//! Supervises the node's effect engine (RB-06 step 6,
//! docs/architecture/control-plane.md#supervision, ADR-0032, ADR-0034 D9):
//! starts `folo-source` with the local link's URL and a launch token in its
//! environment, serves its end of the link, restarts it with backoff when
//! it exits or hangs, and stops it with a `restart` request.

pub mod config;
pub mod link;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

pub use link::{LinkSocket, Message, Request, RpcError};

/// The engine's environment variables for the link (ADR-0032 D2).
pub const ENV_URL: &str = "FOLO_SOURCE_LINK";
pub const ENV_TOKEN: &str = "FOLO_SOURCE_TOKEN";
/// The worker's name in `folo status`.
pub const WORKER: &str = "folo-source";

/// The first restart delay, the longest one, and how much of each is cut
/// at random (ADR-0034 D9, as the source link's backoff).
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const JITTER: f64 = 0.25;
/// Linked this long, the engine's next restart starts the backoff over.
const HEALTHY_AFTER: Duration = Duration::from_secs(60);
/// How long a stopping engine has before it is killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a request to the engine may take before it counts as unanswered
/// (docs/specs/client-api.md#requests-to-sources).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a started or unlinked engine has to link before it counts as
/// hung: the engine itself gives up after 30 s without a link (ADR-0032).
const LINK_TIMEOUT: Duration = Duration::from_secs(30);
/// Starts a process with a console of its own that has no window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where the engine's output goes: the node's log (ADR-0034 D8).
pub trait LogSink: Send + Sync {
    /// One line the engine wrote to stderr.
    fn engine_line(&self, line: &str);
}

/// How to start the engine.
#[derive(Debug, Clone)]
pub struct EngineCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl EngineCommand {
    /// `folo-source` beside the node's executable, or else from the `PATH`.
    #[must_use]
    pub fn find(exe_dir: &Path) -> Self {
        let name = if cfg!(windows) {
            "folo-source.exe"
        } else {
            "folo-source"
        };
        let beside = exe_dir.join(name);
        let program = if beside.is_file() {
            beside
        } else {
            PathBuf::from("folo-source")
        };
        Self {
            program,
            args: Vec::new(),
        }
    }
}

/// The worker's state, as `folo status` reports it (api.json's `WorkerStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerStatus {
    pub state: WorkerState,
    /// Its process id; 0 while it isn't running.
    pub pid: u32,
    /// How often it was started again since the node started.
    pub restarts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Starting,
    Running,
    Stopped,
    Failed,
}

impl WorkerState {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

/// A value the engine sent, with the update it came in: the count of
/// everything the engine sent before it, so a reader can tell what's new.
#[derive(Debug, Clone)]
pub struct Stamped<T> {
    pub update: u64,
    pub value: T,
}

/// The newest of what the linked engine sent; empty until it sends.
#[derive(Debug, Default)]
pub struct Received {
    /// Counts what the engine sent since the node started.
    pub updates: u64,
    /// The newest notification of each method, but `levels`.
    pub notifications: HashMap<String, Stamped<Value>>,
    /// The newest `levels` notification of each surface index.
    pub levels: HashMap<u16, Stamped<Value>>,
    /// The newest surface (type 2) and signal (type 3) frame by index,
    /// without the type byte.
    pub frames: HashMap<(u8, u16), Stamped<Arc<Vec<u8>>>>,
}

impl Received {
    pub(crate) fn next(&mut self) -> u64 {
        self.updates += 1;
        self.updates
    }
}

/// The type byte of surface frames.
const SURFACE_FRAME: u8 = 2;

/// The surface ids of a configuration, in index order.
fn surfaces_of(config: Option<&Value>) -> Vec<&Value> {
    config
        .and_then(|c| c["surfaces"].as_array())
        .into_iter()
        .flatten()
        .map(|s| &s["surface"]["id"])
        .collect()
}

/// Why a request to the engine got no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// No engine is linked, or its link ended before it answered.
    Offline,
    /// It didn't answer within [`REQUEST_TIMEOUT`].
    Timeout,
    /// It answered with a JSON-RPC error.
    Rpc(RpcError),
}

/// What the link session tells the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Linked,
    Unlinked,
    Hung,
}

pub(crate) struct State {
    /// The launch token of the engine the node expects, if any.
    pub token: Option<String>,
    /// Whether an engine is linked (one at a time).
    pub linked: bool,
    /// Where the supervisor sends requests while an engine is linked.
    pub requests: Option<mpsc::UnboundedSender<Request>>,
    pub status: WorkerStatus,
    /// What the linked engine sent.
    pub received: Received,
    /// The surfaces moved and the engine hasn't been sent its new `config.apply` yet.
    pub apply_pending: bool,
    /// The id of the `config.apply` sent last, until the engine answers it. While either is
    /// set, surface frames are dropped: they may be numbered for the surfaces before
    /// (ADR-0036 D6).
    pub apply_id: Option<u64>,
}

pub(crate) struct Shared {
    state: Mutex<State>,
    /// The configuration the engine should have, without its revision.
    pub config: watch::Sender<Option<Value>>,
    /// Counts the configurations sent since the node started.
    pub revision: AtomicU64,
    /// Changes when the supervisor is done with an engine, which ends its link.
    pub generation: watch::Sender<u64>,
    /// Events from link sessions, with the generation of the engine they're about.
    pub events: mpsc::UnboundedSender<(u64, Event)>,
    /// Counts the surface frames taken, and the links made and ended, so the output task
    /// steps when one arrives.
    pub arrivals: watch::Sender<u64>,
    /// Counts the links made and ended.
    pub links: watch::Sender<u64>,
    events_in: Mutex<Option<mpsc::UnboundedReceiver<(u64, Event)>>>,
}

impl Shared {
    fn new() -> Self {
        let (events, events_in) = mpsc::unbounded_channel();
        Self {
            state: Mutex::new(State {
                token: None,
                linked: false,
                requests: None,
                status: WorkerStatus {
                    state: WorkerState::Stopped,
                    pid: 0,
                    restarts: 0,
                },
                received: Received::default(),
                apply_pending: false,
                apply_id: None,
            }),
            config: watch::channel(None).0,
            revision: AtomicU64::new(0),
            generation: watch::channel(0).0,
            events,
            events_in: Mutex::new(Some(events_in)),
            arrivals: watch::channel(0).0,
            links: watch::channel(0).0,
        }
    }

    /// A link was made or ended.
    pub(crate) fn link_changed(&self) {
        self.links.send_modify(|n| *n += 1);
        self.arrivals.send_modify(|n| *n += 1);
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The engine `generation` linked is gone. A session that ends after
    /// the supervisor moved on to a newer engine leaves that one's link alone.
    pub(crate) fn release(&self, generation: u64) {
        let mut state = self.lock();
        if *self.generation.borrow() == generation && state.linked {
            state.linked = false;
            state.requests = None;
            drop(state);
            self.link_changed();
        }
    }

    fn take_events(&self) -> mpsc::UnboundedReceiver<(u64, Event)> {
        self.events_in
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("the events are taken once")
    }

    fn set_status(&self, state: WorkerState, pid: u32) {
        let mut guard = self.lock();
        guard.status.state = state;
        guard.status.pid = pid;
    }

    #[cfg(test)]
    pub(crate) fn take_events_for_tests(&self) -> mpsc::UnboundedReceiver<(u64, Event)> {
        self.take_events()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Control {
    wanted: bool,
    shutdown: bool,
}

/// A handle the node's server uses to serve the link's endpoint.
#[derive(Clone)]
pub struct LinkEndpoint {
    shared: Arc<Shared>,
}

impl LinkEndpoint {
    /// Serves one connection to `/api/v1/source-link` until it ends. Only a
    /// `loopback` client may link as the local engine (ADR-0035 D1).
    pub async fn serve<S: LinkSocket>(&self, socket: S, loopback: bool) {
        link::serve(&self.shared, socket, loopback).await;
    }
}

/// The effect engine's supervisor.
pub struct Supervisor {
    shared: Arc<Shared>,
    control: watch::Sender<Control>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Supervisor {
    /// Starts supervising: the engine runs while `wanted`. `url` is the
    /// local link's URL the engine dials; `log` takes its output.
    #[must_use]
    pub fn start(command: EngineCommand, url: String, log: Arc<dyn LogSink>, wanted: bool) -> Self {
        let shared = Arc::new(Shared::new());
        let (control, control_rx) = watch::channel(Control {
            wanted,
            shutdown: false,
        });
        let events = shared.take_events();
        let task = tokio::spawn(supervise(
            Arc::clone(&shared),
            command,
            url,
            log,
            control_rx,
            events,
        ));
        Self {
            shared,
            control,
            task: Mutex::new(Some(task)),
        }
    }

    /// The handle the server serves the link with.
    #[must_use]
    pub fn endpoint(&self) -> LinkEndpoint {
        LinkEndpoint {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Starts or stops the engine (`local_source.enabled`, the role).
    pub fn set_wanted(&self, wanted: bool) {
        self.control.send_modify(|c| c.wanted = wanted);
    }

    /// The configuration the engine gets, without its revision; `None`
    /// sends nothing, as in the source role before its controller sends one.
    pub fn set_config(&self, config: Option<Value>) {
        // Frames and levels are kept by surface index: once the surfaces
        // move, what was received under an index may be another surface's.
        // They are dropped, and surface frames refused until the engine
        // answers the new configuration, before it is published, so the link
        // can't send and clear that before the flag is set
        {
            let current = self.shared.config.borrow();
            if *current == config {
                return;
            }
            if surfaces_of(current.as_ref()) != surfaces_of(config.as_ref()) {
                drop(current);
                let mut state = self.shared.lock();
                state
                    .received
                    .frames
                    .retain(|(kind, _), _| *kind != SURFACE_FRAME);
                state.received.levels.clear();
                state.apply_pending = true;
            }
        }
        self.shared.config.send_replace(config);
    }

    /// The worker's state.
    #[must_use]
    pub fn status(&self) -> WorkerStatus {
        self.shared.lock().status
    }

    /// Reads the configuration the engine gets, without its revision.
    pub fn with_config<R>(&self, read: impl FnOnce(Option<&Value>) -> R) -> R {
        read(self.shared.config.borrow().as_ref())
    }

    /// Whether an engine is linked now.
    #[must_use]
    pub fn linked(&self) -> bool {
        self.shared.lock().linked
    }

    /// Changes when a surface frame arrives from the engine, and when its link is made or
    /// ends: what the output task steps on (ADR-0036 D2).
    #[must_use]
    pub fn arrivals(&self) -> watch::Receiver<u64> {
        self.shared.arrivals.subscribe()
    }

    /// Changes when the engine's link is made or ends: what it sent is gone then, and the
    /// compositor forgets it.
    #[must_use]
    pub fn links(&self) -> watch::Receiver<u64> {
        self.shared.links.subscribe()
    }

    /// Reads what the linked engine sent, under the supervisor's lock: keep
    /// `read` short.
    pub fn received<R>(&self, read: impl FnOnce(&Received) -> R) -> R {
        read(&self.shared.lock().received)
    }

    /// Sends the linked engine a JSON-RPC request and waits for its result,
    /// at most [`REQUEST_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// [`RequestError`]: no engine linked, no answer in time, or an error answer.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RequestError> {
        let requests = self.shared.lock().requests.clone();
        let Some(requests) = requests else {
            return Err(RequestError::Offline);
        };
        let (reply, answer) = tokio::sync::oneshot::channel();
        let call = Request::Call {
            method: method.to_owned(),
            params,
            reply,
        };
        if requests.send(call).is_err() {
            return Err(RequestError::Offline);
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, answer).await {
            Err(_) => Err(RequestError::Timeout),
            Ok(Err(_)) => Err(RequestError::Offline),
            Ok(Ok(result)) => result.map_err(RequestError::Rpc),
        }
    }

    /// Stops the engine and the supervision.
    pub async fn shutdown(&self) {
        self.control.send_modify(|c| c.shutdown = true);
        let task = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn shared_for_tests() -> Arc<Shared> {
        Arc::new(Shared::new())
    }
}

/// Whether an environment variable is one of the node's `FOLO_*`
/// variables. Windows ignores case in variable names, so there `folo_x`
/// counts too.
#[must_use]
pub fn is_folo_variable(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    let prefix = name.get(..5).unwrap_or_default();
    if cfg!(windows) {
        prefix.eq_ignore_ascii_case("FOLO_")
    } else {
        prefix == "FOLO_"
    }
}

/// A new launch token: 256 random bits, base64url (ADR-0032 D2).
fn launch_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system has a random source");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The delay before restart number `attempt` (0 first): 1 s doubling up to
/// 60 s, cut by up to 25% at random.
fn backoff(attempt: u32) -> Duration {
    let full = BACKOFF_FIRST
        .saturating_mul(1u32 << attempt.min(16))
        .min(BACKOFF_MAX);
    let mut byte = [0u8; 1];
    let random = if getrandom::fill(&mut byte).is_ok() {
        f64::from(byte[0]) / 255.0
    } else {
        0.0
    };
    full.mul_f64(1.0 - JITTER * random)
}

/// How an engine's run ended.
enum Ending {
    Exited(std::io::Result<ExitStatus>),
    Hung,
    /// Stopping: no longer wanted, or the node is shutting down.
    Stop {
        shutdown: bool,
    },
}

/// Watches one run of the engine until it ends: whether it exited, hung
/// or is to stop, and whether a link of it lasted [`HEALTHY_AFTER`].
async fn watch_run(
    shared: &Shared,
    child: &mut Child,
    pid: u32,
    control: &mut watch::Receiver<Control>,
    events: &mut mpsc::UnboundedReceiver<(u64, Event)>,
) -> (Ending, bool) {
    let mut linked_at: Option<Instant> = None;
    // Whether a link lasted HEALTHY_AFTER, even if it ended before the run did
    let mut healthy = false;
    let mut link_by = Instant::now() + LINK_TIMEOUT;
    let ending = loop {
        tokio::select! {
            status = child.wait() => break Ending::Exited(status),
            () = tokio::time::sleep_until(link_by), if linked_at.is_none() => {
                log::error!(
                    "the effect engine didn't link within {} s",
                    LINK_TIMEOUT.as_secs()
                );
                break Ending::Hung;
            }
            event = events.recv() => match event {
                Some((generation, _)) if generation != *shared.generation.borrow() => {}
                Some((_, Event::Linked)) => {
                    linked_at = Some(Instant::now());
                    shared.set_status(WorkerState::Running, pid);
                }
                Some((_, Event::Unlinked)) => {
                    if let Some(since) = linked_at.take() {
                        healthy |= since.elapsed() >= HEALTHY_AFTER;
                        log::warn!("the effect engine's link ended after {:.1} s", since.elapsed().as_secs_f64());
                    }
                    link_by = Instant::now() + LINK_TIMEOUT;
                    shared.set_status(WorkerState::Starting, pid);
                }
                Some((_, Event::Hung)) => break Ending::Hung,
                None => break Ending::Stop { shutdown: true },
            },
            changed = control.changed() => {
                let now = *control.borrow_and_update();
                if changed.is_err() || now.shutdown || !now.wanted {
                    break Ending::Stop { shutdown: changed.is_err() || now.shutdown };
                }
            }
        }
    };
    healthy |= linked_at.is_some_and(|since| since.elapsed() >= HEALTHY_AFTER);
    (ending, healthy)
}

async fn supervise(
    shared: Arc<Shared>,
    command: EngineCommand,
    url: String,
    sink: Arc<dyn LogSink>,
    mut control: watch::Receiver<Control>,
    mut events: mpsc::UnboundedReceiver<(u64, Event)>,
) {
    let mut attempt = 0u32;
    loop {
        // Wait until the engine is wanted.
        loop {
            let now = *control.borrow_and_update();
            if now.shutdown {
                return;
            }
            if now.wanted {
                break;
            }
            shared.set_status(WorkerState::Stopped, 0);
            if control.changed().await.is_err() {
                return;
            }
        }
        while events.try_recv().is_ok() {}
        let token = launch_token();
        shared.lock().token = Some(token.clone());
        let mut child = match spawn(&command, &url, &token, &sink) {
            Ok(child) => child,
            Err(error) => {
                log::error!(
                    "the effect engine ({}) can't start: {error}",
                    command.program.display()
                );
                end_engine(&shared);
                if !wait_backoff(&shared, &mut control, &mut attempt, false).await {
                    return;
                }
                continue;
            }
        };
        let pid = child.id().unwrap_or(0);
        shared.set_status(WorkerState::Starting, pid);
        log::info!("started the effect engine (process {pid})");
        let (ending, healthy) =
            watch_run(&shared, &mut child, pid, &mut control, &mut events).await;
        match ending {
            Ending::Stop { shutdown } => {
                stop(&shared, &mut child).await;
                end_engine(&shared);
                shared.set_status(WorkerState::Stopped, 0);
                if shutdown {
                    return;
                }
                continue;
            }
            Ending::Exited(status) => {
                match status {
                    Ok(status) => log::warn!("the effect engine exited ({status})"),
                    Err(error) => log::error!("waiting for the effect engine failed: {error}"),
                }
                end_engine(&shared);
            }
            Ending::Hung => {
                log::error!("the effect engine is hung; killing it");
                end_engine(&shared);
                let _ = child.kill().await;
            }
        }
        if !wait_backoff(&shared, &mut control, &mut attempt, healthy).await {
            return;
        }
    }
}

/// The engine is gone: its token stops working and its link ends.
fn end_engine(shared: &Shared) {
    {
        let mut state = shared.lock();
        state.token = None;
        state.linked = false;
        state.requests = None;
    }
    shared.generation.send_modify(|g| *g += 1);
}

/// Waits before the next start, counting it as a restart. False when the
/// node is shutting down.
async fn wait_backoff(
    shared: &Shared,
    control: &mut watch::Receiver<Control>,
    attempt: &mut u32,
    healthy: bool,
) -> bool {
    if healthy {
        *attempt = 0;
    }
    let delay = backoff(*attempt);
    *attempt = attempt.saturating_add(1);
    {
        let mut state = shared.lock();
        state.status = WorkerStatus {
            state: WorkerState::Failed,
            pid: 0,
            restarts: state.status.restarts + 1,
        };
    }
    log::info!(
        "starting the effect engine again in {:.1} s",
        delay.as_secs_f64()
    );
    let deadline = Instant::now() + delay;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => return true,
            changed = control.changed() => {
                let now = *control.borrow_and_update();
                if changed.is_err() || now.shutdown {
                    return false;
                }
                if !now.wanted {
                    return true; // the outer loop waits until it's wanted again
                }
            }
        }
    }
}

/// Asks a linked engine to end, and kills it after [`STOP_TIMEOUT`]; an
/// engine not linked yet can't be asked, and is killed at once.
async fn stop(shared: &Shared, child: &mut Child) {
    let requests = shared.lock().requests.clone();
    let asked = requests.is_some_and(|r| r.send(Request::RestartEngine).is_ok());
    if !asked {
        let _ = child.kill().await;
        log::info!("stopped the effect engine before it linked");
        return;
    }
    match tokio::time::timeout(STOP_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => log::info!("the effect engine stopped ({status})"),
        Ok(Err(error)) => log::error!("waiting for the effect engine failed: {error}"),
        Err(_) => {
            log::warn!(
                "the effect engine didn't stop in {} s; killing it",
                STOP_TIMEOUT.as_secs()
            );
            let _ = child.kill().await;
        }
    }
}

fn spawn(
    command: &EngineCommand,
    url: &str,
    token: &str,
    sink: &Arc<dyn LogSink>,
) -> std::io::Result<Child> {
    let mut process = Command::new(&command.program);
    // Its logs are its stderr (ADR-0034 D8); it prints nothing on stdout.
    process
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Keep the console's Ctrl+C and the terminal's signals for the node,
    // which stops the engine over the link.
    #[cfg(windows)]
    process.creation_flags(CREATE_NO_WINDOW);
    #[cfg(unix)]
    process.process_group(0);
    // The node's environment without its own variables (ADR-0034 D9).
    for (name, _) in std::env::vars_os() {
        if is_folo_variable(&name) {
            process.env_remove(name);
        }
    }
    process.env(ENV_URL, url).env(ENV_TOKEN, token);
    let mut child = process.spawn()?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(pump(stderr, Arc::clone(sink)));
    }
    Ok(child)
}

/// Forwards the engine's stderr, line by line, to the node's log. Bytes
/// that aren't UTF-8 (a traceback in the ANSI code page) are replaced, so
/// one such line doesn't end the log.
async fn pump<R: AsyncRead + Unpin>(output: R, sink: Arc<dyn LogSink>) {
    let mut output = BufReader::new(output);
    let mut line = Vec::new();
    loop {
        line.clear();
        match output.read_until(b'\n', &mut line).await {
            Ok(0) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                sink.engine_line(text.trim_end_matches(['\r', '\n']));
            }
            Err(error) => {
                log::debug!("reading the effect engine's output failed: {error}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_a_minute_with_jitter() {
        for attempt in 0..10 {
            let full = BACKOFF_FIRST.saturating_mul(1 << attempt).min(BACKOFF_MAX);
            for _ in 0..20 {
                let delay = backoff(attempt);
                assert!(delay <= full, "{attempt}: {delay:?}");
                assert!(delay >= full.mul_f64(1.0 - JITTER), "{attempt}: {delay:?}");
            }
        }
        assert!(backoff(u32::MAX) <= BACKOFF_MAX);
    }

    #[test]
    fn folo_variables_are_found_by_their_prefix() {
        assert!(is_folo_variable(std::ffi::OsStr::new("FOLO_LOG_LEVEL")));
        assert!(!is_folo_variable(std::ffi::OsStr::new("FOLOX")));
        assert!(!is_folo_variable(std::ffi::OsStr::new("PATH")));
        // Windows ignores case in variable names
        assert_eq!(
            is_folo_variable(std::ffi::OsStr::new("folo_log_level")),
            cfg!(windows)
        );
    }

    #[tokio::test]
    async fn output_that_isnt_utf8_doesnt_end_the_log() {
        struct Lines(Mutex<Vec<String>>);
        impl LogSink for Lines {
            fn engine_line(&self, line: &str) {
                self.0.lock().unwrap().push(line.to_owned());
            }
        }
        let lines = Arc::new(Lines(Mutex::new(Vec::new())));
        // A path in the ANSI code page, then a JSON line, then no newline
        let output: &[u8] = b"C:\\Users\\Jos\xe9\r\n{\"msg\": \"after\"}\nlast";
        pump(output, Arc::clone(&lines) as Arc<dyn LogSink>).await;
        let got = lines.0.lock().unwrap().clone();
        assert_eq!(
            got,
            ["C:\\Users\\Jos\u{fffd}", "{\"msg\": \"after\"}", "last"]
        );
    }

    #[test]
    fn launch_tokens_are_256_bits_of_base64url() {
        let token = launch_token();
        assert_eq!(URL_SAFE_NO_PAD.decode(&token).unwrap().len(), 32);
        assert_ne!(token, launch_token());
    }

    #[test]
    fn the_engine_is_found_beside_the_node_or_on_the_path() {
        let missing = EngineCommand::find(Path::new("no such folder"));
        assert_eq!(missing.program, PathBuf::from("folo-source"));
    }
}
