//! The element output: starting an area's stream, sending frames into it, and ending it
//! (`docs/integrations/hue.md`, FOLO provider design; `docs/architecture/providers.md`).
//!
//! A bridge has one stream, so starts and releases are counted: the stream begins with the first
//! target started and ends with the last one released. Every packet carries every channel of the
//! area, so the provider keeps the last colour of each channel and a frame updates the channels
//! of its targets only. Channels no started target drives stay black.

use std::collections::{BTreeMap, BTreeSet};

use folo_provider::{
    BoxFuture, ElementColors, ElementFrame, ElementOutput, ProviderError, TargetOutput,
};
use serde_json::json;

use crate::dtls::{DtlsError, DtlsSession};
use crate::encoder::{EncodeError, Encoder, MAX_CHANNELS};
use crate::model::{EntertainmentConfiguration, HueTarget};
use crate::provider::{Connection, HueProvider};
use crate::rest::{BridgeClient, RestError};

/// What a start is refused with when another application streams.
pub const IN_USE_BY_ANOTHER_APP: &str = "in use by another app";

/// A level on the 0–255 scale as the byte the bridge takes: truncated toward zero, saturating.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn byte(level: f32) -> u8 {
    level as u8
}

/// The stream of one entertainment area.
pub(crate) struct ActiveStream {
    /// The area's id.
    pub configuration: String,
    pub name: String,
    channels: Vec<u8>,
    colors: BTreeMap<u8, [u8; 3]>,
    encoder: Encoder,
    session: Option<DtlsSession>,
    /// The targets started on it.
    pub started: BTreeSet<String>,
}

impl ActiveStream {
    pub(crate) fn new(
        configuration: &EntertainmentConfiguration,
        encoder: Encoder,
        session: DtlsSession,
    ) -> Self {
        let channels = configuration.channel_ids();
        Self {
            configuration: configuration.id.clone(),
            name: configuration.metadata.name.clone(),
            colors: channels.iter().map(|id| (*id, [0; 3])).collect(),
            channels,
            encoder,
            session: Some(session),
            started: BTreeSet::new(),
        }
    }

    /// Sets the colours of a target's channels: element `i` goes to the target's `i`-th channel.
    /// Elements beyond the channels, and channels beyond the elements, are left alone.
    pub(crate) fn apply(&mut self, target: &HueTarget, colors: &[[f32; 3]]) {
        for (channel, rgb) in target.channels.iter().zip(colors) {
            if let Some(slot) = self.colors.get_mut(channel) {
                *slot = rgb.map(byte);
            }
        }
    }

    /// Turns a target's channels off.
    pub(crate) fn silence(&mut self, target: &HueTarget) {
        for channel in &target.channels {
            if let Some(slot) = self.colors.get_mut(channel) {
                *slot = [0; 3];
            }
        }
    }

    /// The packet of the current colours: every channel of the area.
    pub(crate) fn packet(&mut self) -> Result<Vec<u8>, EncodeError> {
        let records: Vec<(u8, [u8; 3])> = self
            .channels
            .iter()
            .map(|id| (*id, self.colors.get(id).copied().unwrap_or([0; 3])))
            .collect();
        self.encoder.encode(&records)
    }

    fn send(&mut self) -> Result<(), ProviderError> {
        let packet = self
            .packet()
            .map_err(|e| ProviderError::Failed(e.to_string()))?;
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| ProviderError::Unreachable("the stream is closed".into()))?;
        session.send(&packet).map_err(|e| match e {
            DtlsError::Handshake(m) | DtlsError::Closed(m) | DtlsError::Io(m) => {
                ProviderError::Unreachable(m)
            }
        })
    }

    /// Sends `close_notify`.
    pub(crate) fn close(&mut self) {
        if let Some(session) = self.session.take() {
            session.close();
        }
    }
}

impl HueProvider {
    fn target(&self, id: &str) -> Option<HueTarget> {
        let targets = self
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        targets.get(id).cloned()
    }

    fn set_pending_stop(&self, configuration: Option<String>) {
        *self
            .pending_stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = configuration;
    }

    /// Asks the bridge to stop streaming to an area, trying a few times.
    pub(crate) async fn try_stop_area(
        &self,
        client: &BridgeClient,
        configuration: &str,
    ) -> Result<(), RestError> {
        let path = format!("entertainment_configuration/{configuration}");
        client
            .put_retrying(
                &path,
                &json!({"action": "stop"}),
                self.options.action_tries,
                self.options.action_pause,
            )
            .await
            .map(|_| ())
    }

    /// Stops an area that FOLO itself started, and only that: a stream another application has
    /// started in the meantime is theirs to stop (ADR-0013 D5). Returns whether nothing is left
    /// to stop, so the caller can forget the area.
    async fn stop_if_ours(&self, connection: &Connection, configuration: &str) -> bool {
        let areas = match connection
            .client
            .resources("entertainment_configuration")
            .await
        {
            Ok(areas) => areas,
            Err(e) => {
                log::warn!("reading the entertainment areas to stop one failed: {e}");
                return false;
            }
        };
        let area: Option<EntertainmentConfiguration> = areas
            .into_iter()
            .filter_map(|v| serde_json::from_value::<EntertainmentConfiguration>(v).ok())
            .find(|a| a.id == configuration);
        let Some(area) = area.filter(EntertainmentConfiguration::is_active) else {
            return true;
        };
        // Without an application id our own stream can't be told from another's: leave it to
        // the bridge's ten second timeout
        let ours = connection
            .application_id
            .as_deref()
            .is_some_and(|me| !area.streamed_by_other(me));
        if !ours {
            log::info!("the entertainment area is streamed by another app now: left alone");
            return true;
        }
        match self.try_stop_area(&connection.client, configuration).await {
            Ok(()) => true,
            Err(e) => {
                log::warn!("stopping the entertainment area failed: {e}");
                false
            }
        }
    }

    /// Stops an area, remembering it until that is done: a stop cut short by the sender's time
    /// limit is finished by the next release or connection.
    async fn release_area(&self, connection: &Connection, configuration: &str) {
        self.set_pending_stop(Some(configuration.to_owned()));
        if self.stop_if_ours(connection, configuration).await {
            self.set_pending_stop(None);
        }
    }

    /// Stops an area whose stream `start` or a failure left behind.
    pub(crate) async fn stop_pending(&self, connection: &Connection) {
        let pending = self
            .pending_stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(configuration) = pending {
            self.release_area(connection, &configuration).await;
        }
    }

    /// Closes the stream without the bridge: the connection to it is gone. The area is stopped
    /// when the connection is back.
    pub(crate) async fn discard_stream(&self) {
        let taken = self.stream.lock().await.take();
        if let Some(mut stream) = taken {
            stream.close();
            self.set_pending_stop(Some(stream.configuration.clone()));
        }
    }

    /// Ends the stream: `close_notify`, then the area's stop.
    pub(crate) async fn end_stream(&self, connection: &Connection) {
        let taken = self.stream.lock().await.take();
        if let Some(mut stream) = taken {
            stream.close();
            self.release_area(connection, &stream.configuration).await;
        }
        self.stop_pending(connection).await;
    }

    async fn open_stream(
        &self,
        connection: &Connection,
        target: &HueTarget,
    ) -> Result<ActiveStream, ProviderError> {
        let client = &connection.client;
        let areas = client.resources("entertainment_configuration").await?;
        let areas: Vec<EntertainmentConfiguration> = areas
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        // One stream per bridge, so anyone's stream in the way ends the start, never theirs
        let me = connection.application_id.as_deref().unwrap_or_default();
        if let Some(other) = areas.iter().find(|a| a.streamed_by_other(me)) {
            return Err(ProviderError::Refused(format!(
                "{IN_USE_BY_ANOTHER_APP}: {}",
                other.metadata.name
            )));
        }
        let area = areas
            .iter()
            .find(|a| a.id == target.configuration)
            .ok_or_else(|| ProviderError::Refused("the entertainment area is gone".into()))?;
        if area.channel_ids().len() > MAX_CHANNELS {
            return Err(ProviderError::Refused(format!(
                "the area has more than {MAX_CHANNELS} channels"
            )));
        }
        let encoder = Encoder::new(&area.id).map_err(|e| ProviderError::Failed(e.to_string()))?;
        let dtls = self.dtls_settings(&connection.identity)?;
        let address = self.endpoint.dtls_address().await?;

        self.set_pending_stop(Some(area.id.clone()));
        if area.is_active() {
            // A stream of ours that outlived the last run: starting doesn't change an owner
            if let Err(e) = self.try_stop_area(client, &area.id).await {
                log::warn!("stopping the old stream failed: {e}");
            }
        }
        let path = format!("entertainment_configuration/{}", area.id);
        client
            .put_retrying(
                &path,
                &json!({"action": "start"}),
                self.options.action_tries,
                self.options.action_pause,
            )
            .await?;

        // The handshake follows the start at once: the bridge waits a short while for it
        let session = tokio::task::spawn_blocking(move || DtlsSession::connect(address, &dtls))
            .await
            .map_err(|e| ProviderError::Failed(format!("the handshake stopped: {e}")))?;
        match session {
            Ok(session) => {
                self.set_pending_stop(None);
                Ok(ActiveStream::new(area, encoder, session))
            }
            Err(e) => {
                self.release_area(connection, &area.id).await;
                Err(ProviderError::Failed(e.to_string()))
            }
        }
    }

    async fn start_target(&self, id: &str) -> Result<(), ProviderError> {
        let target = self
            .target(id)
            .ok_or_else(|| ProviderError::Refused(format!("the bridge has no light {id}")))?;
        let connection = self.connected()?;
        let mut guard = self.stream.lock().await;
        if let Some(active) = guard.as_mut() {
            if active.configuration != target.configuration {
                return Err(ProviderError::Refused(format!(
                    "the bridge is streaming to {:?} and allows one stream",
                    active.name
                )));
            }
            active.started.insert(id.to_owned());
            return Ok(());
        }
        let mut active = self.open_stream(&connection, &target).await?;
        active.started.insert(id.to_owned());
        *guard = Some(active);
        Ok(())
    }

    async fn release_target(&self, id: &str) -> Result<(), ProviderError> {
        let target = self.target(id);
        let mut guard = self.stream.lock().await;
        let Some(active) = guard.as_mut() else {
            drop(guard);
            // A start that was cancelled may have left the area started
            if let Ok(connection) = self.connected() {
                self.stop_pending(&connection).await;
            }
            return Ok(());
        };
        active.started.remove(id);
        if let Some(target) = target {
            // The stream stays open for the others, and these lights go dark
            active.silence(&target);
        }
        if !active.started.is_empty() {
            return Ok(());
        }
        let last = guard.take();
        drop(guard);
        if let Some(mut stream) = last {
            stream.close();
            self.set_pending_stop(Some(stream.configuration.clone()));
            if let Ok(connection) = self.connected() {
                self.release_area(&connection, &stream.configuration).await;
            }
        }
        Ok(())
    }

    async fn send_frame(&self, frame: &ElementFrame) -> Result<(), ProviderError> {
        let mut guard = self.stream.lock().await;
        let Some(active) = guard.as_mut() else {
            return Ok(());
        };
        for TargetOutput { target, colors } in &frame.targets {
            let (Some(info), ElementColors::Rgb(rgb)) = (self.target(target), colors) else {
                continue;
            };
            active.apply(&info, rgb);
        }
        let sent = active.send();
        if let Err(e) = &sent {
            // The stream is gone: the connection starts over, and the area is stopped then
            if let Some(mut dead) = guard.take() {
                dead.close();
                self.set_pending_stop(Some(dead.configuration.clone()));
            }
            log::warn!("the entertainment stream failed: {e}");
        }
        sent
    }
}

impl ElementOutput for HueProvider {
    fn start(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move { self.start_target(&target).await })
    }

    fn send<'a>(&'a self, frame: &'a ElementFrame) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move { self.send_frame(frame).await })
    }

    fn wire_key(&self, output: &TargetOutput) -> Vec<u8> {
        output.colors.to_bytes()
    }

    fn release(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move { self.release_target(&target).await })
    }
}
