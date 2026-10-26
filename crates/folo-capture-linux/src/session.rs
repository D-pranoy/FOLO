//! The portal half of a capture, on a thread of its own with a small tokio
//! runtime: connect to the session bus, tell the portal the agent's id, ask
//! for a monitor with a restore token, and keep the session open while the
//! PipeWire stream runs. Closing the session (or this thread ending) ends the
//! share.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions, Streams,
};
use ashpd::enumflags2::BitFlags;
use ashpd::zbus::Connection;
use ashpd::zbus::connection::Builder;
use folo_capture_core::types::CaptureState;
use futures_util::StreamExt as _;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::appid::{self, Registration, UNKNOWN_HINT};
use crate::notify;
use crate::shared::Shared;
use crate::stream::{self, StreamEvent};

/// The oldest ScreenCast interface that keeps restore tokens.
pub const MINIMUM_PORTAL_VERSION: u32 = 4;

/// Where the portal is, and how soon an answer counts as a dialog.
#[derive(Debug, Clone)]
pub struct PortalSettings {
    /// The session bus to use; `None` takes the user's (`DBUS_SESSION_BUS_ADDRESS`).
    pub bus_address: Option<String>,
    /// How long `Start` may take before the agent counts the consent dialog as
    /// shown: restoring a saved choice answers at once.
    pub dialog_delay: Duration,
    /// The file that names the agent's systemd unit, `/proc/self/cgroup`.
    pub cgroup_file: Option<std::path::PathBuf>,
}

impl Default for PortalSettings {
    fn default() -> Self {
        Self {
            bus_address: None,
            dialog_delay: Duration::from_millis(1500),
            cgroup_file: Some("/proc/self/cgroup".into()),
        }
    }
}

/// What the backend can ask of the thread.
#[derive(Debug)]
pub enum Command {
    Stop,
}

/// The portal thread, to stop and join.
pub struct Worker {
    commands: UnboundedSender<Command>,
    thread: JoinHandle<()>,
}

impl Worker {
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Asks the thread to close the session and waits for it.
    pub fn stop(self) {
        // A thread that has ended has nothing left to close.
        let _ = self.commands.send(Command::Stop);
        let _ = self.thread.join();
    }
}

/// Starts the portal thread. It records progress and failures in `shared`.
///
/// # Errors
///
/// A message when the thread can't be started.
pub fn spawn(
    settings: PortalSettings,
    shared: Arc<Shared>,
    restore_token: Option<String>,
) -> Result<Worker, String> {
    let (commands, receiver) = unbounded_channel();
    let thread = thread::Builder::new()
        .name("folo-capture-portal".to_owned())
        .spawn(move || work(&settings, &shared, restore_token, receiver))
        .map_err(|error| format!("can't start the portal thread: {error}"))?;
    Ok(Worker { commands, thread })
}

fn work(
    settings: &PortalSettings,
    shared: &Arc<Shared>,
    restore_token: Option<String>,
    mut commands: UnboundedReceiver<Command>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            shared.fail(format!("can't start the portal runtime: {error}"));
            return;
        }
    };
    let outcome = runtime.block_on(capture(settings, shared, restore_token, &mut commands));
    match outcome {
        Ok(()) => shared.set_state(CaptureState::Off),
        Err(message) => shared.fail(message),
    }
}

async fn connect(settings: &PortalSettings) -> Result<Connection, String> {
    let builder = match &settings.bus_address {
        Some(address) => Builder::address(address.as_str()),
        None => Builder::session(),
    }
    .map_err(|error| format!("the desktop's session bus isn't reachable: {error}"))?;
    builder
        .build()
        .await
        .map_err(|error| format!("can't connect to the desktop's session bus: {error}"))
}

/// A portal answer as the message the status shows.
fn describe(what: &str, error: &ashpd::Error) -> String {
    match error {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            format!("{what}: the screen share was declined in the desktop's dialog")
        }
        other => format!("{what}: {other}"),
    }
}

/// One capture, from the connection to the end of the session. `Ok` after a
/// stop request; an error message for everything that went wrong.
async fn capture(
    settings: &PortalSettings,
    shared: &Arc<Shared>,
    restore_token: Option<String>,
    commands: &mut UnboundedReceiver<Command>,
) -> Result<(), String> {
    let connection = connect(settings).await?;
    let cgroup = settings
        .cgroup_file
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok());
    if let Registration::Unknown(why) = appid::register(&connection, cgroup.as_deref()).await {
        shared.warn(format!("{UNKNOWN_HINT}: {why}"));
    }
    let screencast = Screencast::with_connection(connection.clone())
        .await
        .map_err(|error| describe("the desktop has no ScreenCast portal", &error))?;
    if screencast.version() < MINIMUM_PORTAL_VERSION {
        return Err(format!(
            "the desktop's ScreenCast portal is version {}, and screen sync needs version {MINIMUM_PORTAL_VERSION} or newer for restore tokens",
            screencast.version()
        ));
    }
    // ashpd doesn't export the options type to name it.
    #[allow(clippy::default_trait_access)]
    let session = screencast
        .create_session(Default::default())
        .await
        .map_err(|error| describe("can't create a screen cast session", &error))?;
    let outcome = share(
        settings,
        shared,
        &connection,
        &screencast,
        &session,
        restore_token,
        commands,
    )
    .await;
    // Closing is best effort: the session may be gone already.
    let _ = session.close().await;
    outcome
}

type PortalSession = ashpd::desktop::Session<Screencast>;

async fn share(
    settings: &PortalSettings,
    shared: &Arc<Shared>,
    connection: &Connection,
    screencast: &Screencast,
    session: &PortalSession,
    restore_token: Option<String>,
    commands: &mut UnboundedReceiver<Command>,
) -> Result<(), String> {
    let mut closed = session
        .receive_closed()
        .await
        .map_err(|error| describe("can't watch the session", &error))?;
    let options = SelectSourcesOptions::default()
        .set_sources(BitFlags::from(SourceType::Monitor))
        .set_multiple(false)
        .set_cursor_mode(CursorMode::Hidden)
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(restore_token.as_deref());
    screencast
        .select_sources(session, options)
        .await
        .and_then(|request| request.response())
        .map_err(|error| describe("can't choose what to share", &error))?;
    let Some(streams) = start(
        settings,
        shared,
        connection,
        screencast,
        session,
        restore_token.is_some(),
        commands,
    )
    .await?
    else {
        return Ok(());
    };
    // Saved after every start: a token works once (D14).
    if let Some(token) = streams.restore_token() {
        shared.set_restore_token(token.to_owned());
    }
    let Some(chosen) = streams.streams().first() else {
        return Err("the desktop shared no screen".to_owned());
    };
    let node_id = chosen.pipe_wire_node_id();
    if let Some((width, height)) = chosen.size() {
        let size = (
            u32::try_from(width).unwrap_or(0),
            u32::try_from(height).unwrap_or(0),
        );
        shared.set_screen_size(size);
    }
    let remote = screencast
        .open_pipe_wire_remote(
            session,
            ashpd::desktop::screencast::OpenPipeWireRemoteOptions::default(),
        )
        .await
        .map_err(|error| describe("can't open the PipeWire remote", &error))?;
    let (events, mut stream_events) = unbounded_channel();
    let handle =
        stream::spawn(remote, node_id, shared, &events).map_err(|error| error.to_string())?;
    let outcome = loop {
        tokio::select! {
            command = commands.recv() => {
                // A dropped sender is a stop too.
                let _ = command;
                break Ok(());
            }
            ended = closed.next() => {
                let _ = ended;
                break Err("the desktop ended the screen share".to_owned());
            }
            event = stream_events.recv() => match event {
                Some(StreamEvent::Streaming) => shared.set_state(CaptureState::Capturing),
                Some(StreamEvent::Ended(why)) => break Err(why),
                None => break Err("the PipeWire stream ended".to_owned()),
            },
        }
    };
    handle.stop();
    outcome
}

/// Calls `Start`, which returns when the user has chosen (or at once when the
/// saved choice still holds). `None` when a stop request came first.
async fn start(
    settings: &PortalSettings,
    shared: &Arc<Shared>,
    connection: &Connection,
    screencast: &Screencast,
    session: &PortalSession,
    had_token: bool,
    commands: &mut UnboundedReceiver<Command>,
) -> Result<Option<Streams>, String> {
    let pending = screencast.start(session, None, StartCastOptions::default());
    tokio::pin!(pending);
    let dialog = tokio::time::sleep(settings.dialog_delay);
    tokio::pin!(dialog);
    let mut waiting = false;
    loop {
        tokio::select! {
            answer = &mut pending => {
                return answer
                    .and_then(|request| request.response())
                    .map(Some)
                    .map_err(|error| describe("can't start the screen share", &error));
            }
            () = &mut dialog, if !waiting => {
                waiting = true;
                shared.set_state(CaptureState::WaitingConsent);
                // A saved choice that no longer holds is why a dialog shows (D21).
                // Not awaited here: a desktop with no notification service
                // would hold up the answer to `Start` and a stop request.
                if had_token {
                    tokio::spawn(notify::reprompt(connection.clone()));
                }
            }
            _ = commands.recv() => return Ok(None),
        }
    }
}
