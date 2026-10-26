//! Supervises the node's display capture agent (RB-14, ADR-0037,
//! docs/specs/source-link.md#the-capture-link): starts `folo-capture` with
//! the capture link's URL and a launch token in its environment while capture
//! is wanted, serves the node's end of the link, restarts the agent with the
//! engine's policy, stops it with `capture.stop` and `restart`, and passes
//! its screen frames on to the engine's link unchanged.
//!
//! Raw frames are never read, logged or kept here (ADR-0037 D9): they live
//! as bytes in [`ScreenFrames`] until the engine's link sends them.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use folo_protocol::clamp::clamp;
use serde_json::{Value, json};
use subtle::ConstantTimeEq as _;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep_until, timeout};

use crate::link::{LinkSocket, Message, RpcError, rpc};
use crate::{
    Event, HEALTHY_AFTER, LINK_TIMEOUT, REQUEST_TIMEOUT, RequestError, STOP_TIMEOUT, ScreenFrames,
    WorkerState, WorkerStatus, backoff, is_folo_variable, launch_token,
};

/// The agent's environment variables for the link (ADR-0037 D2).
pub const ENV_URL: &str = "FOLO_CAPTURE_LINK";
pub const ENV_TOKEN: &str = "FOLO_CAPTURE_TOKEN";
/// The worker's name in `folo status`.
pub const WORKER: &str = "folo-capture";
/// The WebSocket subprotocol of the capture link.
pub const SUBPROTOCOL: &str = "folo.capture.v1";
/// The one capture-link version.
pub const VERSION: &str = "1";
/// How long the agent has for its handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Silence after which the agent counts as hung (ADR-0037 D2).
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
/// No message on the link may be longer (a frame is at most 388,836 bytes).
pub const MAX_MESSAGE_BYTES: usize = 400_000;
/// The most output of `folo-capture screens --json` that is read.
const LISTING_MAX_BYTES: usize = 64 * 1024;
/// How long the output of `folo-capture screens --json` is reused.
const LISTING_KEPT: Duration = Duration::from_secs(3);

/// Message type bytes (docs/specs/source-link.md#messages-inside-the-noise-channel).
const TYPE_RPC: u8 = 0x01;
const TYPE_SCREEN: u8 = 0x04;
/// Starts a process with a console of its own that has no window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where the agent's output and discoveries go.
pub trait CaptureSink: Send + Sync {
    /// One line the agent wrote to stderr: its log (ADR-0034 D8).
    fn agent_line(&self, line: &str);
    /// The new restore token the agent returned (ADR-0037 D14), to keep in
    /// `secrets.json`. It is never part of the status clients see.
    fn restore_token(&self, token: &str);
}

/// How to start the agent, and how to ask it for its screens when none is
/// linked.
#[derive(Debug, Clone)]
pub struct CaptureCommand {
    pub program: PathBuf,
    /// The arguments of a run (`--backend ...`).
    pub run_args: Vec<OsString>,
    /// The program that prints `{"screens": [...]}` with `list_args`: the
    /// agent itself.
    pub list_program: PathBuf,
    /// The arguments that print it (`screens --json`).
    pub list_args: Vec<OsString>,
    /// `systemd-run`, where the agent is started in a transient scope so the
    /// desktop portal reads its application id (ADR-0037 D16). `None` starts
    /// the agent directly. Only used on Linux.
    pub scope_launcher: Option<PathBuf>,
}

impl CaptureCommand {
    /// `folo-capture` beside the node's executable, or else from the `PATH`.
    #[must_use]
    pub fn find(exe_dir: &Path) -> Self {
        let name = if cfg!(windows) {
            "folo-capture.exe"
        } else {
            "folo-capture"
        };
        let beside = exe_dir.join(name);
        let program = if beside.is_file() {
            beside
        } else {
            PathBuf::from("folo-capture")
        };
        Self {
            list_program: program.clone(),
            program,
            run_args: Vec::new(),
            list_args: vec!["screens".into(), "--json".into()],
            scope_launcher: find_scope_launcher(),
        }
    }

    /// The program and arguments that start the agent: in a transient scope
    /// named `scope` when a launcher is set, else the agent itself. The link's
    /// URL and token are never among them (ADR-0037 D2).
    #[cfg(target_os = "linux")]
    fn launch(&self, scope: &str) -> (&Path, Vec<OsString>) {
        let Some(launcher) = &self.scope_launcher else {
            return (&self.program, self.run_args.clone());
        };
        let mut args: Vec<OsString> = vec![
            "--user".into(),
            "--scope".into(),
            "--quiet".into(),
            format!("--unit={scope}").into(),
            self.program.clone().into_os_string(),
        ];
        // `--backend ...` comes before the subcommand
        args.extend(self.run_args.iter().cloned());
        args.push("run".into());
        (launcher, args)
    }
}

/// `systemd-run` on the `PATH` on Linux, if it can make a scope for this user
/// (no reachable user manager makes it fail, and a direct start still works).
fn find_scope_launcher() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let launcher = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|folder| folder.join("systemd-run"))
        .find(|candidate| candidate.is_file())?;
    let works = std::process::Command::new(&launcher)
        .args(["--user", "--scope", "--quiet", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !works {
        log::warn!("systemd-run can't make a user scope here");
    }
    works.then_some(launcher)
}

/// Why `screens()` has no list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    /// The agent answered with a JSON-RPC error.
    Rpc(RpcError),
    /// The listing command failed or printed something else.
    Failed(String),
}

/// What the node asks of the linked agent.
enum Call {
    /// `capture.stop` if it runs, then `restart {component: capture}`.
    Restart,
    /// A JSON-RPC request whose answer goes to `reply`.
    Request {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, RpcError>>,
    },
}

/// Who waits for the answer to a request.
enum Waiting {
    Caller(oneshot::Sender<Result<Value, RpcError>>),
    /// The answer to `capture.start`: a refusal is shown as the screen's error.
    Start,
}

type Pending = HashMap<u64, Waiting>;

/// What the supervisor wants of the agent now.
#[derive(Debug, Clone, PartialEq)]
struct Control {
    /// The `capture.start` params (without the restore token) while capture
    /// is wanted; `None` while it isn't.
    params: Option<Value>,
    shutdown: bool,
}

struct State {
    /// The launch token of the agent the node expects, if any.
    token: Option<String>,
    linked: bool,
    requests: Option<mpsc::UnboundedSender<Call>>,
    status: WorkerStatus,
    /// Whether capture is wanted, also when no agent can run.
    wanted: bool,
    /// The agent's newest `capture.status`, without its restore token.
    report: Option<Value>,
    /// Why the agent can't capture: a start it refused, or a launch that failed.
    problem: Option<String>,
    /// The portal's restore token (ADR-0037 D14); never shown to clients.
    restore_token: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    control: watch::Sender<Control>,
    /// Changes when the supervisor is done with an agent, which ends its link.
    generation: watch::Sender<u64>,
    events: mpsc::UnboundedSender<(u64, Event)>,
    events_in: Mutex<Option<mpsc::UnboundedReceiver<(u64, Event)>>>,
    frames: ScreenFrames,
    sink: Arc<dyn CaptureSink>,
    /// Why no agent can run here, when that is so (ADR-0037 D3).
    unavailable: Option<String>,
}

impl Shared {
    fn new(
        frames: ScreenFrames,
        sink: Arc<dyn CaptureSink>,
        restore_token: Option<String>,
        unavailable: Option<String>,
    ) -> Self {
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
                wanted: false,
                report: None,
                problem: None,
                restore_token,
            }),
            control: watch::channel(Control {
                params: None,
                shutdown: false,
            })
            .0,
            generation: watch::channel(0).0,
            events,
            events_in: Mutex::new(Some(events_in)),
            frames,
            sink,
            unavailable,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The agent `generation` linked is gone.
    fn release(&self, generation: u64) {
        let mut state = self.lock();
        if *self.generation.borrow() == generation {
            state.linked = false;
            state.requests = None;
            state.report = None;
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
}

/// A handle the node's server uses to serve the capture link's endpoint.
#[derive(Clone)]
pub struct CaptureEndpoint {
    shared: Arc<Shared>,
}

impl CaptureEndpoint {
    /// Serves one connection to `/api/v1/capture-link` until it ends. Only a
    /// `loopback` client may link as the agent (ADR-0037 D1).
    pub async fn serve<S: LinkSocket>(&self, socket: S, loopback: bool) {
        serve(&self.shared, socket, loopback).await;
    }
}

/// The capture agent's supervisor.
pub struct CaptureSupervisor {
    shared: Arc<Shared>,
    command: CaptureCommand,
    /// The newest output of the listing command, and when it came: one
    /// listing runs at a time, and a new one only after [`LISTING_KEPT`].
    listing: tokio::sync::Mutex<Option<(Instant, Value)>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl CaptureSupervisor {
    /// Starts supervising: the agent runs while capture is wanted
    /// ([`set_wanted`](Self::set_wanted)). `url` is the capture link's URL it
    /// dials; `frames` is where its screen frames go; `restore_token` is
    /// the one `secrets.json` holds. With `unavailable`, no agent is ever
    /// started here, and the reason is what clients are told (ADR-0037 D3).
    #[must_use]
    pub fn start(
        command: CaptureCommand,
        url: String,
        sink: Arc<dyn CaptureSink>,
        frames: ScreenFrames,
        restore_token: Option<String>,
        unavailable: Option<String>,
    ) -> Self {
        let shared = Arc::new(Shared::new(frames, sink, restore_token, unavailable));
        let events = shared.take_events();
        let task = tokio::spawn(supervise(
            Arc::clone(&shared),
            command.clone(),
            url,
            shared.control.subscribe(),
            events,
        ));
        Self {
            shared,
            command,
            listing: tokio::sync::Mutex::new(None),
            task: Mutex::new(Some(task)),
        }
    }

    /// The handle the server serves the link with.
    #[must_use]
    pub fn endpoint(&self) -> CaptureEndpoint {
        CaptureEndpoint {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Sets what capture is wanted for: the `capture.start` params without
    /// the restore token, or `None` when it isn't wanted. A linked agent gets
    /// a changed request in place.
    pub fn set_wanted(&self, params: Option<Value>) {
        self.shared.lock().wanted = params.is_some();
        if self.shared.unavailable.is_some() {
            return;
        }
        self.shared.control.send_if_modified(|control| {
            if control.params == params {
                false
            } else {
                control.params = params;
                true
            }
        });
    }

    /// The worker's state.
    #[must_use]
    pub fn status(&self) -> WorkerStatus {
        self.shared.lock().status
    }

    /// Whether an agent is linked now.
    #[must_use]
    pub fn linked(&self) -> bool {
        self.shared.lock().linked
    }

    /// `SourceLive.screen` (specs/schema/live.json's `ScreenStatus`): `off`
    /// while capture isn't wanted, `unavailable` while it is but no agent can
    /// run, otherwise the agent's own status without its restore token.
    #[must_use]
    pub fn screen_live(&self) -> Value {
        let state = self.shared.lock();
        if !state.wanted {
            return screen_state("off", None);
        }
        if let Some(why) = &self.shared.unavailable {
            return screen_state("unavailable", Some(why));
        }
        if state.linked {
            return match (&state.report, &state.problem) {
                (Some(report), problem) => with_problem(report, problem.as_deref()),
                (None, Some(problem)) => screen_state("error", Some(problem)),
                (None, None) => screen_state("starting", None),
            };
        }
        match state.status.state {
            WorkerState::Failed => {
                let why = state
                    .problem
                    .as_deref()
                    .unwrap_or("the capture agent isn't running; the node starts it again");
                screen_state("unavailable", Some(why))
            }
            WorkerState::Starting | WorkerState::Running | WorkerState::Stopped => {
                screen_state("starting", None)
            }
        }
    }

    /// The screens the agent can capture, as `screens.list` answers them:
    /// the linked agent's answer, or the output of `folo-capture screens
    /// --json` when none is linked. Empty where no agent can run (ADR-0037 D15).
    ///
    /// # Errors
    ///
    /// [`ListError`] when the agent refused, or the command failed.
    pub async fn screens(&self) -> Result<Value, ListError> {
        if self.shared.unavailable.is_some() {
            return Ok(json!({"screens": []}));
        }
        let listed = match self.request("screens.list", json!({})).await {
            Ok(answer) => answer,
            Err(RequestError::Rpc(error)) => return Err(ListError::Rpc(error)),
            Err(RequestError::Offline | RequestError::Timeout) => self.listed_by_command().await?,
        };
        // What an agent says goes to clients: at the contract's limits
        clamp("ScreensListResult", &listed, &mut String::new)
            .map_err(|_| ListError::Failed("the screen list isn't a list of screens".into()))
    }

    /// The listing command's output, the same one for [`LISTING_KEPT`]: a page
    /// that polls doesn't start a process each time, and none run at once.
    async fn listed_by_command(&self) -> Result<Value, ListError> {
        let mut kept = self.listing.lock().await;
        if let Some((at, listed)) = &*kept
            && at.elapsed() < LISTING_KEPT
        {
            return Ok(listed.clone());
        }
        let listed = list_with_command(&self.command).await?;
        *kept = Some((Instant::now(), listed.clone()));
        Ok(listed)
    }

    /// Sends the linked agent a JSON-RPC request and waits for its result,
    /// at most [`REQUEST_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// [`RequestError`]: no agent linked, no answer in time, or an error answer.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RequestError> {
        let requests = self.shared.lock().requests.clone();
        let Some(requests) = requests else {
            return Err(RequestError::Offline);
        };
        let (reply, answer) = oneshot::channel();
        let call = Call::Request {
            method: method.to_owned(),
            params,
            reply,
        };
        if requests.send(call).is_err() {
            return Err(RequestError::Offline);
        }
        match timeout(REQUEST_TIMEOUT, answer).await {
            Err(_) => Err(RequestError::Timeout),
            Ok(Err(_)) => Err(RequestError::Offline),
            Ok(Ok(result)) => result.map_err(RequestError::Rpc),
        }
    }

    /// Stops the agent and the supervision.
    pub async fn shutdown(&self) {
        self.shared
            .control
            .send_modify(|control| control.shutdown = true);
        let task = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    /// A supervisor that starts no process: a test acts as the agent.
    #[cfg(test)]
    fn without_process(
        frames: ScreenFrames,
        sink: Arc<dyn CaptureSink>,
        restore_token: Option<String>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared::new(frames, sink, restore_token, None)),
            command: CaptureCommand::find(Path::new("no such folder")),
            listing: tokio::sync::Mutex::new(None),
            task: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn shared_for_tests(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }
}

/// A reason as `ScreenStatus.message` takes it: at most 500 characters.
fn shorten(message: &str) -> String {
    message.chars().take(500).collect()
}

/// The status of an agent that doesn't capture, in `ScreenStatus`'s shape.
fn screen_state(state: &str, message: Option<&str>) -> Value {
    let mut status = json!({
        "state": state,
        "backend": "",
        "screen": "",
        "fps": 0,
        "hdr": false,
        "protected": false,
        "bars": {"top": 0, "bottom": 0, "left": 0, "right": 0},
    });
    if let Some(message) = message {
        status["message"] = json!(message);
    }
    status
}

/// The agent's status; its refused start is what an `off` agent says.
fn with_problem(report: &Value, problem: Option<&str>) -> Value {
    match problem {
        Some(problem) if report["state"] == "off" => screen_state("error", Some(problem)),
        _ => report.clone(),
    }
}

/// Runs `folo-capture screens --json` (ADR-0037 D15). It lists without
/// capturing and asks for no consent.
async fn list_with_command(command: &CaptureCommand) -> Result<Value, ListError> {
    let mut process = Command::new(&command.list_program);
    process
        .args(&command.list_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(CREATE_NO_WINDOW);
    remove_folo_variables(&mut process);
    let failed = |why: String| ListError::Failed(why);
    let run = async {
        let mut child = process.spawn()?;
        let mut stdout = Vec::new();
        if let Some(out) = child.stdout.take() {
            // One byte over the limit tells it is too long, without reading the rest
            out.take(LISTING_MAX_BYTES as u64 + 1)
                .read_to_end(&mut stdout)
                .await?;
        }
        let status = child.wait().await?;
        std::io::Result::Ok((status, stdout))
    };
    let (status, stdout) = match timeout(REQUEST_TIMEOUT, run).await {
        Err(_) => return Err(failed("listing the screens took too long".into())),
        Ok(Err(error)) => return Err(failed(format!("the capture agent can't run: {error}"))),
        Ok(Ok(done)) => done,
    };
    if !status.success() {
        return Err(failed(format!("listing the screens failed ({status})")));
    }
    if stdout.len() > LISTING_MAX_BYTES {
        return Err(failed("the screen list is too long".into()));
    }
    let listing: Value =
        serde_json::from_slice(&stdout).map_err(|_| failed("the screen list isn't JSON".into()))?;
    if listing["screens"].is_array() {
        Ok(listing)
    } else {
        Err(failed("the screen list has no screens".into()))
    }
}

/// The node's environment without its own variables (ADR-0034 D9).
fn remove_folo_variables(process: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if is_folo_variable(&name) {
            process.env_remove(name);
        }
    }
}

// ---------------------------------------------------------------- the link

/// Why a connection was turned away.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    /// Not a hello with the current launch token, or no common version.
    Rejected(String),
    /// Another agent is linked already; the connection is closed at once.
    Busy,
}

/// Serves one connection to the capture link until it ends.
async fn serve<S: LinkSocket>(shared: &Shared, mut socket: S, loopback: bool) {
    let Ok(Some(Message::Text(first))) = timeout(HANDSHAKE_TIMEOUT, socket.recv()).await else {
        log::warn!("a connection to the capture link sent no handshake");
        return;
    };
    let mut generation = shared.generation.subscribe();
    let agent = *generation.borrow_and_update();
    match check_hello(shared, &first, loopback) {
        Ok(()) => {}
        Err(Refusal::Busy) => {
            log::warn!("refused a second connection to the capture link");
            return;
        }
        Err(Refusal::Rejected(why)) => {
            log::warn!("refused a connection to the capture link: {why}");
            let welcome = json!({"version": VERSION, "result": "rejected"});
            let _ = socket.send(Message::Text(welcome.to_string())).await;
            return;
        }
    }
    let welcome = json!({"version": VERSION, "result": "ok"});
    if !socket.send(Message::Text(welcome.to_string())).await {
        shared.release(agent);
        return;
    }
    log::info!("the capture agent linked (capture link {VERSION})");
    let ended = session(shared, &mut socket, &mut generation, agent).await;
    shared.release(agent);
    shared.frames.clear();
    let _ = shared.events.send((agent, ended));
}

/// Checks the agent's hello: the token first, in constant time, so a rogue
/// local connection can't take the slot; then whether an agent is linked.
/// Accepting marks the link as taken.
fn check_hello(shared: &Shared, first: &str, loopback: bool) -> Result<(), Refusal> {
    let hello: Value = serde_json::from_str(first)
        .map_err(|_| Refusal::Rejected("the hello isn't JSON".into()))?;
    if !loopback {
        return Err(Refusal::Rejected(
            "the capture link is taken from loopback clients only".into(),
        ));
    }
    let offered = hello["token"].as_str().unwrap_or_default();
    let mut state = shared.lock();
    let Some(token) = state.token.as_deref() else {
        return Err(Refusal::Rejected("no agent is expected".into()));
    };
    if !bool::from(token.as_bytes().ct_eq(offered.as_bytes())) {
        return Err(Refusal::Rejected("wrong launch token".into()));
    }
    if state.linked {
        return Err(Refusal::Busy);
    }
    let speaks = hello["version"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|v| v == VERSION);
    if !speaks {
        return Err(Refusal::Rejected(format!(
            "no common version in {}",
            hello["version"]
        )));
    }
    state.linked = true;
    // A new link starts with nothing known: what an earlier agent said is stale
    state.report = None;
    state.problem = None;
    Ok(())
}

/// The linked session: sends `capture.start` and `capture.stop` as the
/// wish changes, requests, watches the heartbeat, and keeps what the agent
/// sends. Returns why it ended.
async fn session<S: LinkSocket>(
    shared: &Shared,
    socket: &mut S,
    generation: &mut watch::Receiver<u64>,
    agent: u64,
) -> Event {
    let mut control = shared.control.subscribe();
    let mut requests = {
        let (sender, receiver) = mpsc::unbounded_channel();
        shared.lock().requests = Some(sender);
        receiver
    };
    let _ = shared.events.send((agent, Event::Linked));
    let mut last_seen = Instant::now();
    let mut next_id: u64 = 1;
    let mut pending = Pending::new();
    // The params the agent was last told to capture with; `None` while it is off
    let mut sent: Option<Value> = None;
    control.mark_changed();
    loop {
        let message: Option<Vec<u8>> = tokio::select! {
            message = socket.recv() => {
                let Some(message) = message else {
                    log::info!("the capture agent closed the link");
                    return Event::Unlinked;
                };
                last_seen = Instant::now();
                match receive(shared, message, &mut pending) {
                    Some(reply) => Some(reply),
                    None => continue,
                }
            }
            changed = control.changed() => {
                if changed.is_err() {
                    return Event::Unlinked;
                }
                let wanted = control.borrow_and_update().params.clone();
                if wanted == sent {
                    continue;
                }
                let message = match &wanted {
                    Some(params) => start_message(shared, &mut next_id, &mut pending, sent.is_none(), params),
                    None => rpc(take_id(&mut next_id), "capture.stop", &json!({})),
                };
                sent = wanted;
                Some(message)
            }
            request = requests.recv() => match request {
                // Its caller gave up (504): the agent mustn't act on it late
                Some(Call::Request { reply, .. }) if reply.is_closed() => continue,
                Some(Call::Request { method, params, reply }) => {
                    pending.retain(|_, waiting| !matches!(waiting, Waiting::Caller(r) if r.is_closed()));
                    let id = take_id(&mut next_id);
                    pending.insert(id, Waiting::Caller(reply));
                    Some(rpc(id, &method, &params))
                }
                Some(Call::Restart) => {
                    let stop = sent
                        .take()
                        .map(|_| rpc(take_id(&mut next_id), "capture.stop", &json!({})));
                    if let Some(stop) = stop
                        && let Some(ended) = send(socket, Message::Binary(stop)).await
                    {
                        return ended;
                    }
                    let restart = rpc(
                        take_id(&mut next_id),
                        "restart",
                        &json!({"component": "capture"}),
                    );
                    Some(restart)
                }
                None => continue,
            },
            _ = generation.changed() => return Event::Unlinked,
            () = sleep_until(last_seen + HEARTBEAT_TIMEOUT) => {
                log::error!(
                    "the capture agent sent nothing for {} s",
                    HEARTBEAT_TIMEOUT.as_secs()
                );
                return Event::Hung;
            }
        };
        if let Some(message) = message
            && let Some(ended) = send(socket, Message::Binary(message)).await
        {
            return ended;
        }
    }
}

fn take_id(next_id: &mut u64) -> u64 {
    let id = *next_id;
    *next_id += 1;
    id
}

/// `capture.start` for `params`: the restore token goes only with a capture
/// that starts from off (ADR-0037 D3), never again by itself.
fn start_message(
    shared: &Shared,
    next_id: &mut u64,
    pending: &mut Pending,
    from_off: bool,
    params: &Value,
) -> Vec<u8> {
    let mut start = params.clone();
    {
        let mut state = shared.lock();
        state.problem = None;
        if from_off && let Some(token) = &state.restore_token {
            start["restore_token"] = json!(token);
        }
    }
    let id = take_id(next_id);
    pending.insert(id, Waiting::Start);
    rpc(id, "capture.start", &start)
}

/// Sends a message in the session: `None` once sent, or why the session
/// ends. An agent that stops reading counts as hung, as one that stops
/// sending does.
async fn send<S: LinkSocket>(socket: &mut S, message: Message) -> Option<Event> {
    match timeout(HEARTBEAT_TIMEOUT, socket.send(message)).await {
        Ok(true) => None,
        Ok(false) => Some(Event::Unlinked),
        Err(_) => {
            log::error!(
                "the capture agent read nothing for {} s",
                HEARTBEAT_TIMEOUT.as_secs()
            );
            Some(Event::Hung)
        }
    }
}

/// Keeps what the agent sent: its newest status, answers to requests, and
/// screen frames, which are passed on without being read (ADR-0037 D9).
fn receive(shared: &Shared, message: Message, pending: &mut Pending) -> Option<Vec<u8>> {
    let Message::Binary(bytes) = message else {
        log::debug!("ignored a text message after the handshake");
        return None;
    };
    if bytes.len() > MAX_MESSAGE_BYTES {
        log::warn!("dropped a capture message of {} bytes", bytes.len());
        return None;
    }
    match bytes.first().copied() {
        Some(TYPE_RPC) => match serde_json::from_slice::<Value>(&bytes[1..]) {
            Ok(rpc) => return receive_rpc(shared, rpc, pending),
            Err(_) => log::warn!("ignored a JSON-RPC message that isn't JSON"),
        },
        Some(TYPE_SCREEN) => shared.frames.publish(bytes),
        other => log::debug!("ignored a capture message of type {other:?}"),
    }
    None
}

fn receive_rpc(shared: &Shared, mut rpc: Value, pending: &mut Pending) -> Option<Vec<u8>> {
    if let Some(method) = rpc["method"].as_str() {
        if let Some(id) = rpc.get("id") {
            // The agent asks nothing of the node
            log::debug!("refused a request for {method} from the capture agent");
            let error = json!({"code": -32601, "message": "the node answers no requests"});
            let body = json!({"jsonrpc": "2.0", "id": id, "error": error});
            let mut out = vec![TYPE_RPC];
            out.extend_from_slice(body.to_string().as_bytes());
            return Some(out);
        } else if method == "capture.status" {
            let params = rpc["params"].take();
            keep_status(shared, params);
        } else {
            log::debug!("ignored a {method} notification from the capture agent");
        }
        return None;
    }
    let waiting = rpc["id"].as_u64().and_then(|id| pending.remove(&id));
    let error = rpc.get("error").map(|error| RpcError {
        code: error["code"].as_i64().unwrap_or(0),
        message: error["message"].as_str().unwrap_or_default().to_owned(),
    });
    match (waiting, error) {
        (Some(Waiting::Caller(reply)), Some(error)) => {
            let _ = reply.send(Err(error));
        }
        (Some(Waiting::Caller(reply)), None) => {
            let _ = reply.send(Ok(rpc["result"].take()));
        }
        (Some(Waiting::Start), Some(error)) => {
            log::warn!("the capture agent refused capture.start: {}", error.message);
            shared.lock().problem = Some(shorten(&error.message));
        }
        (Some(Waiting::Start), None) => shared.lock().problem = None,
        (None, Some(error)) => log::warn!(
            "the capture agent answered request {} with {} ({})",
            rpc["id"],
            error.message,
            error.code
        ),
        (None, None) => {}
    }
    None
}

/// The longest restore token (specs/schema/source-link.json).
const MAX_TOKEN_CHARS: usize = 256;

/// Keeps a restore token the agent reported: for the next capture that starts
/// from off, and with the node through the sink. An unchanged token (the
/// agent may repeat it with every status) and one the contract doesn't take
/// are dropped; the message never holds the token.
fn keep_token(shared: &Shared, token: &str) {
    if token.chars().count() > MAX_TOKEN_CHARS || token.chars().any(char::is_control) {
        log::warn!("ignored a restore token the contract doesn't take");
        return;
    }
    {
        let mut state = shared.lock();
        if state.restore_token.as_deref() == Some(token) {
            return;
        }
        state.restore_token = Some(token.to_owned());
    }
    shared.sink.restore_token(token);
}

/// Keeps the agent's `capture.status` as clients may see it: clamped to
/// the contract, without the restore token, which goes to the sink.
fn keep_status(shared: &Shared, mut params: Value) {
    let token = params
        .as_object_mut()
        .and_then(|status| status.remove("restore_token"))
        .and_then(|token| token.as_str().map(str::to_owned))
        .filter(|token| !token.is_empty());
    let Ok(mut report) = clamp("CaptureStatusParams", &params, &mut String::new) else {
        log::warn!("ignored a capture.status that isn't a status");
        return;
    };
    // The clamper keeps the field it was not given out; but never trust that
    if let Some(report) = report.as_object_mut() {
        report.remove("restore_token");
    }
    if let Some(token) = token {
        keep_token(shared, &token);
    }
    shared.lock().report = Some(report);
}

// ---------------------------------------------------------------- the process

/// How an agent's run ended.
enum Ending {
    Exited(std::io::Result<ExitStatus>),
    Hung,
    /// Stopping: capture isn't wanted any more, or the node is shutting down.
    Stop {
        shutdown: bool,
    },
}

async fn supervise(
    shared: Arc<Shared>,
    command: CaptureCommand,
    url: String,
    mut control: watch::Receiver<Control>,
    mut events: mpsc::UnboundedReceiver<(u64, Event)>,
) {
    let mut attempt = 0u32;
    loop {
        // Wait until an agent is wanted.
        loop {
            let now = control.borrow_and_update().clone();
            if now.shutdown {
                return;
            }
            if now.params.is_some() {
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
        let mut child = match spawn(&command, &url, &token, &shared.sink) {
            Ok(child) => child,
            Err(error) => {
                log::error!(
                    "the capture agent ({}) can't start: {error}",
                    command.program.display()
                );
                shared.lock().problem =
                    Some(shorten(&format!("the capture agent can't start: {error}")));
                end_agent(&shared);
                if !wait_backoff(&shared, &mut control, &mut attempt, false).await {
                    return;
                }
                continue;
            }
        };
        let pid = child.id().unwrap_or(0);
        shared.set_status(WorkerState::Starting, pid);
        log::info!("started the capture agent (process {pid})");
        let (ending, healthy) =
            watch_run(&shared, &mut child, pid, &mut control, &mut events).await;
        match ending {
            Ending::Stop { shutdown } => {
                stop(&shared, &mut child).await;
                end_agent(&shared);
                shared.set_status(WorkerState::Stopped, 0);
                if shutdown {
                    return;
                }
                continue;
            }
            Ending::Exited(status) => {
                match status {
                    Ok(status) => log::warn!("the capture agent exited ({status})"),
                    Err(error) => log::error!("waiting for the capture agent failed: {error}"),
                }
                end_agent(&shared);
            }
            Ending::Hung => {
                log::error!("the capture agent is hung; killing it");
                end_agent(&shared);
                let _ = child.kill().await;
            }
        }
        if !wait_backoff(&shared, &mut control, &mut attempt, healthy).await {
            return;
        }
    }
}

/// Watches one run of the agent until it ends: whether it exited, hung
/// or is to stop, and whether a link of it lasted [`HEALTHY_AFTER`].
async fn watch_run(
    shared: &Shared,
    child: &mut Child,
    pid: u32,
    control: &mut watch::Receiver<Control>,
    events: &mut mpsc::UnboundedReceiver<(u64, Event)>,
) -> (Ending, bool) {
    let mut linked_at: Option<Instant> = None;
    let mut healthy = false;
    let mut link_by = Instant::now() + LINK_TIMEOUT;
    let ending = loop {
        tokio::select! {
            status = child.wait() => break Ending::Exited(status),
            () = sleep_until(link_by), if linked_at.is_none() => {
                log::error!(
                    "the capture agent didn't link within {} s",
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
                        log::warn!("the capture agent's link ended after {:.1} s", since.elapsed().as_secs_f64());
                    }
                    link_by = Instant::now() + LINK_TIMEOUT;
                    shared.set_status(WorkerState::Starting, pid);
                }
                Some((_, Event::Hung)) => break Ending::Hung,
                None => break Ending::Stop { shutdown: true },
            },
            changed = control.changed() => {
                let now = control.borrow_and_update().clone();
                if changed.is_err() || now.shutdown || now.params.is_none() {
                    break Ending::Stop { shutdown: changed.is_err() || now.shutdown };
                }
            }
        }
    };
    healthy |= linked_at.is_some_and(|since| since.elapsed() >= HEALTHY_AFTER);
    (ending, healthy)
}

/// The agent is gone: its token stops working and its link ends.
fn end_agent(shared: &Shared) {
    {
        let mut state = shared.lock();
        state.token = None;
        state.linked = false;
        state.requests = None;
        state.report = None;
    }
    shared.generation.send_modify(|g| *g += 1);
    shared.frames.clear();
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
        "starting the capture agent again in {:.1} s",
        delay.as_secs_f64()
    );
    let deadline = Instant::now() + delay;
    loop {
        tokio::select! {
            () = sleep_until(deadline) => return true,
            changed = control.changed() => {
                let now = control.borrow_and_update().clone();
                if changed.is_err() || now.shutdown {
                    return false;
                }
                if now.params.is_none() {
                    return true; // the outer loop waits until it's wanted again
                }
            }
        }
    }
}

/// Asks a linked agent to stop capturing and end, and kills it after
/// [`STOP_TIMEOUT`]; an agent not linked yet can't be asked, and is killed
/// at once.
async fn stop(shared: &Shared, child: &mut Child) {
    let requests = shared.lock().requests.clone();
    let asked = requests.is_some_and(|r| r.send(Call::Restart).is_ok());
    if !asked {
        let _ = child.kill().await;
        log::info!("stopped the capture agent before it linked");
        return;
    }
    match timeout(STOP_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => log::info!("the capture agent stopped ({status})"),
        Ok(Err(error)) => log::error!("waiting for the capture agent failed: {error}"),
        Err(_) => {
            log::warn!(
                "the capture agent didn't stop in {} s; killing it",
                STOP_TIMEOUT.as_secs()
            );
            let _ = child.kill().await;
        }
    }
}

/// The missing-launcher warning is logged once, not at every restart.
#[cfg(target_os = "linux")]
static WARNED: std::sync::Once = std::sync::Once::new();

/// How the agent is started on this platform. `systemd-run --scope` replaces
/// itself with the agent once the scope exists, so the child is the agent and
/// it inherits the environment that carries the link's token.
fn launch_plan(command: &CaptureCommand) -> (&Path, Vec<OsString>) {
    #[cfg(target_os = "linux")]
    {
        if command.scope_launcher.is_some() {
            let random = launch_token();
            return command.launch(&folo_capture_linux::appid::transient_scope_name(&random));
        }
        WARNED.call_once(|| {
            log::warn!(
                "systemd-run isn't available; starting the capture agent outside a scope, so the \
             desktop portal may not know it (ADR-0037 D16)"
            );
        });
    }
    (&command.program, command.run_args.clone())
}

fn spawn(
    command: &CaptureCommand,
    url: &str,
    token: &str,
    sink: &Arc<dyn CaptureSink>,
) -> std::io::Result<Child> {
    let (program, args) = launch_plan(command);
    let mut process = Command::new(program);
    // Its logs are its stderr (ADR-0034 D8); it prints nothing on stdout.
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Keep the console's Ctrl+C and the terminal's signals for the node,
    // which stops the agent over the link.
    #[cfg(windows)]
    process.creation_flags(CREATE_NO_WINDOW);
    #[cfg(unix)]
    process.process_group(0);
    remove_folo_variables(&mut process);
    // On the environment only, never the command line (ADR-0037 D2)
    process.env(ENV_URL, url).env(ENV_TOKEN, token);
    let mut child = process.spawn()?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(pump(stderr, Arc::clone(sink)));
    }
    Ok(child)
}

/// Forwards the agent's stderr, line by line, to the node's log.
async fn pump<R: AsyncRead + Unpin>(output: R, sink: Arc<dyn CaptureSink>) {
    let mut output = BufReader::new(output);
    let mut line = Vec::new();
    loop {
        line.clear();
        match output.read_until(b'\n', &mut line).await {
            Ok(0) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                sink.agent_line(text.trim_end_matches(['\r', '\n']));
            }
            Err(error) => {
                log::debug!("reading the capture agent's output failed: {error}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
