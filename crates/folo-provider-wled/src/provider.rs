//! The WLED kind and its instances.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use folo_protocol::generated::{
    DiscoveredDevice, ProviderInstance, ProviderInstanceKind, ProviderSecrets,
};
use folo_provider::{
    AdminError, AdminMethod, BoxFuture, DeliveryPolicy, DescribedTarget, ElementFrame,
    ElementOutput, Provider, ProviderError, ProviderKind, TargetOutput,
};
use serde_json::Value;

use crate::admin::Admin;
use crate::api::{DeviceApi, HttpApi};
use crate::ddp::{DdpEncoder, PixelFormat, PixelRun};
use crate::describe::{Description, WledTarget};
use crate::device::Device;
use crate::discovery::{Browse, MdnsBrowser, device_from_answer};
use crate::dnrgb::{DNRGB_PORT, DnrgbEncoder};
use crate::pixels::pixel_bytes;
use crate::reader::{INFO_PATH, read_device};
use crate::settings::{Config, Transport};
use crate::sink::{DatagramSink, UdpSink};

/// An unchanged frame goes out again after this long at the latest: well inside WLED's
/// realtime timeout (2.5 s by default).
const KEEPALIVE: Duration = Duration::from_secs(1);

/// The WLED kind: discovery and making instances.
pub struct WledKind {
    browser: Arc<dyn Browse>,
    ddp_port: u16,
    dnrgb_port: Option<u16>,
}

impl Default for WledKind {
    fn default() -> Self {
        Self::new()
    }
}

impl WledKind {
    /// With mDNS discovery and the standard ports.
    #[must_use]
    pub fn new() -> Self {
        Self::with_browser(Arc::new(MdnsBrowser::default()))
    }

    /// With another way to browse, for tests.
    #[must_use]
    pub fn with_browser(browser: Arc<dyn Browse>) -> Self {
        Self {
            browser,
            ddp_port: crate::ddp::DDP_PORT,
            dnrgb_port: None,
        }
    }

    /// With other UDP ports than the device's, for tests that listen on loopback.
    #[must_use]
    pub fn with_ports(mut self, ddp: u16, dnrgb: u16) -> Self {
        self.ddp_port = ddp;
        self.dnrgb_port = Some(dnrgb);
        self
    }
}

impl ProviderKind for WledKind {
    fn kind(&self) -> ProviderInstanceKind {
        ProviderInstanceKind::Wled
    }

    fn discover(&self) -> BoxFuture<'_, Result<Vec<DiscoveredDevice>, ProviderError>> {
        Box::pin(async move {
            let answers = self.browser.browse().await?;
            Ok(answers.iter().filter_map(device_from_answer).collect())
        })
    }

    fn create(
        &self,
        instance: &ProviderInstance,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Arc<dyn Provider>, ProviderError> {
        let mut config = Config::from_instance(instance, secrets)?;
        config.ddp_port = self.ddp_port;
        config.dnrgb_port = self.dnrgb_port;
        let api = Arc::new(HttpApi::new(&config.authority));
        let sink = Arc::new(UdpSink::new(&config.host));
        Ok(Arc::new(WledProvider::new(config, api, sink)))
    }
}

struct State {
    description: Option<Description>,
    /// The device's own UDP realtime port (`info.udpport`).
    device_udp_port: Option<u16>,
    /// The targets realtime output was started for. The output is device-wide: it ends when the
    /// last of them is released.
    started: BTreeSet<String>,
    ddp: DdpEncoder,
}

/// One WLED device.
pub struct WledProvider {
    config: Config,
    api: Arc<dyn DeviceApi>,
    sink: Arc<dyn DatagramSink>,
    state: Mutex<State>,
}

/// What a frame turned into: datagrams for one port.
struct Datagrams {
    port: u16,
    packets: Vec<Vec<u8>>,
}

impl WledProvider {
    #[must_use]
    pub fn new(config: Config, api: Arc<dyn DeviceApi>, sink: Arc<dyn DatagramSink>) -> Self {
        Self {
            config,
            api,
            sink,
            state: Mutex::new(State {
                description: None,
                device_udp_port: None,
                started: BTreeSet::new(),
                ddp: DdpEncoder::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn dnrgb_port(&self, state: &State) -> u16 {
        self.config
            .dnrgb_port
            .or(state.device_udp_port)
            .unwrap_or(DNRGB_PORT)
    }

    fn remember(&self, device: &Device) -> Description {
        let description = Description::of(device);
        let mut state = self.lock();
        state.device_udp_port = device.udp_port;
        state.description = Some(description.clone());
        description
    }

    /// The datagrams of a frame, and their port.
    fn datagrams(&self, frame: &ElementFrame) -> Result<Datagrams, ProviderError> {
        let mut state = self.lock();
        let description = state
            .description
            .clone()
            .ok_or_else(|| ProviderError::Failed("the device hasn't been described".into()))?;
        let mut runs: Vec<(PixelFormat, u32, Vec<u8>)> = Vec::new();
        for output in &frame.targets {
            let target = description.target(&output.target).ok_or_else(|| {
                ProviderError::Failed(format!("no such target: {}", output.target))
            })?;
            let (format, bytes) = pixel_bytes(&output.colors, target)?;
            let first = target.first_pixel.saturating_sub(description.base_pixel);
            runs.push((format, first, bytes));
        }
        Ok(match self.config.transport {
            Transport::Ddp => {
                let borrowed: Vec<PixelRun<'_>> = runs
                    .iter()
                    .map(|(format, first_pixel, data)| PixelRun {
                        format: *format,
                        first_pixel: *first_pixel,
                        data,
                    })
                    .collect();
                Datagrams {
                    port: self.config.ddp_port,
                    packets: state.ddp.frame(&borrowed),
                }
            }
            Transport::Dnrgb => {
                let encoder = DnrgbEncoder::new(self.config.timeout_s);
                Datagrams {
                    port: self.dnrgb_port(&state),
                    packets: runs
                        .iter()
                        .flat_map(|(format, first, data)| encoder.pixels(*format, *first, data))
                        .collect(),
                }
            }
        })
    }

    async fn transmit(&self, datagrams: Datagrams) -> Result<(), ProviderError> {
        for packet in datagrams.packets {
            self.sink
                .send(datagrams.port, packet)
                .await
                .map_err(|e| ProviderError::Unreachable(format!("sending to the device: {e}")))?;
        }
        Ok(())
    }
}

impl Provider for WledProvider {
    fn connect(&self) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(async move {
            self.sink.reset().await;
            self.api.get(INFO_PATH).await.map(|_| ())
        })
    }

    fn describe(&self) -> BoxFuture<'_, Result<Vec<DescribedTarget>, ProviderError>> {
        Box::pin(async move {
            let device = read_device(self.api.as_ref(), self.config.pin.as_deref()).await?;
            let description = self.remember(&device);
            for warning in &description.warnings {
                log::warn!("WLED {}: {warning}", self.config.authority);
            }
            Ok(description.described())
        })
    }

    fn disconnect(&self) -> BoxFuture<'_, ()> {
        // Every request has its own connection
        Box::pin(async {})
    }

    fn admin(
        &self,
        action: &str,
        method: AdminMethod,
        body: Value,
    ) -> BoxFuture<'_, Result<Value, AdminError>> {
        let action = action.to_owned();
        Box::pin(async move {
            let admin = Admin {
                api: self.api.as_ref(),
                config: &self.config,
            };
            let outcome = admin.run(&action, method, &body).await?;
            if let Some(device) = &outcome.device {
                // Segments and settings change what the targets are
                self.remember(device);
            }
            Ok(outcome.answer)
        })
    }

    fn delivery_policy(&self) -> DeliveryPolicy {
        DeliveryPolicy {
            keepalive: KEEPALIVE,
            ..DeliveryPolicy::default()
        }
    }

    fn element_output(&self) -> Option<&dyn ElementOutput> {
        Some(self)
    }
}

impl ElementOutput for WledProvider {
    fn start(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move {
            let mut state = self.lock();
            let known = state
                .description
                .as_ref()
                .is_some_and(|d| d.target(&target).is_some());
            if !known {
                return Err(ProviderError::Refused(format!(
                    "the device has no target {target}"
                )));
            }
            // WLED starts its realtime mode with the first packet
            state.started.insert(target);
            Ok(())
        })
    }

    fn send<'a>(&'a self, frame: &'a ElementFrame) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move {
            let datagrams = self.datagrams(frame)?;
            self.transmit(datagrams).await
        })
    }

    fn wire_key(&self, output: &TargetOutput) -> Vec<u8> {
        let state = self.lock();
        let target: Option<&WledTarget> = state
            .description
            .as_ref()
            .and_then(|d| d.target(&output.target));
        match target.map(|t| pixel_bytes(&output.colors, t)) {
            Some(Ok((_, bytes))) => bytes,
            _ => output.colors.to_bytes(),
        }
    }

    fn release(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move {
            let release = {
                let mut state = self.lock();
                let was_started = state.started.remove(&target);
                (was_started && state.started.is_empty()).then(|| Datagrams {
                    port: self.dnrgb_port(&state),
                    packets: vec![DnrgbEncoder::new(self.config.timeout_s).release()],
                })
            };
            match release {
                Some(datagrams) => self.transmit(datagrams).await,
                None => Ok(()),
            }
        })
    }
}
