//! `folo run` (RB-06 steps 6, 7 and 9): takes the home's lock, reads the
//! settings and documents, makes sure the node has an identity, imports a
//! legacy `studio.json` on a first run, serves the node's port (RB-07),
//! supervises the effect engine, and stops cleanly on a signal.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use folo_identity::{Identity, fingerprint};
use folo_provider::Registry;
use folo_settings::{Launch, Layered, changes};
use folo_store::{Doc, Store};
use folo_supervisor::capture::{CaptureCommand, CaptureSink, CaptureSupervisor};
use folo_supervisor::config::{capture_request, local_config};
use folo_supervisor::{EngineCommand, LogSink, Supervisor};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::cli::{
    EXIT_FAILED, EXIT_NOT_HERE, EXIT_OK, EXIT_USAGE, Failure, LocationArgs, Place, RunArgs,
    env_var, locate,
};
use crate::{import, live, lock, logs, output, server, zones};

/// How long the server has to finish its connections when the node stops.
const SERVER_STOP: Duration = Duration::from_secs(3);

/// The running node, as the server's routes see it.
pub struct Node {
    pub started: SystemTime,
    pub place: Place,
    /// The node id.
    pub id: String,
    /// Its name in `/api/v1/info`: the computer's host name (ADR-0035 D9).
    pub name: String,
    /// The role and the address it listens on, as started: they change
    /// only with a restart.
    pub role: String,
    pub listen: SocketAddr,
    flags: Vec<(&'static str, String)>,
    settings: Mutex<Layered>,
    pub store: Store,
    pub supervisor: Supervisor,
    /// The display capture agent (RB-14, ADR-0037).
    pub capture: CaptureSupervisor,
    /// The live channel's connections.
    pub live: live::Hub,
    /// The controller's output: the zones' targets, the provider senders (RB-08).
    pub output: output::Output,
}

impl Node {
    /// The settings as last read.
    pub fn settings(&self) -> MutexGuard<'_, Layered> {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the engine should run: always in the source role, while the
    /// local source is on in the controller role (ADR-0034 D9).
    fn engine_wanted(&self, settings: &Layered) -> bool {
        self.role == "source" || settings.settings.local_source.enabled
    }

    /// Whether the node runs its engine, the source `local`, now.
    #[must_use]
    pub fn engine_runs(&self) -> bool {
        self.engine_wanted(&self.settings())
    }

    /// The engine's configuration: built from the documents and what live
    /// clients watch in the controller role; none in the source role until
    /// its controller sends one (RB-16).
    fn engine_config(&self) -> Option<Value> {
        (self.role == "controller").then(|| {
            // The engine gets the effective activation, not the stored flags
            local_config(
                &zones::with_effective(&self.store.get(Doc::Zones)),
                &self.store.get(Doc::Show),
                &self.store.get(Doc::Sources),
                self.live.wanted(),
            )
        })
    }

    /// Logs the zones the activation check suppressed: the check that runs
    /// on every load and write of the zones (ADR-0036 D3).
    pub fn check_zones(&self) {
        for (zone, holder) in zones::check(&self.store.get(Doc::Zones)).suppressed {
            log::warn!("zone {zone} is inactive: zone {holder} drives a target it shares");
        }
    }

    /// Sends the engine its configuration again when it changed: after a
    /// document it comes from changed, or what live clients watch.
    pub fn refresh_engine(&self) {
        self.supervisor.set_config(self.engine_config());
        self.capture.set_wanted(self.capture_wanted());
    }

    /// What the capture agent should capture (ADR-0037 D3): something only
    /// while the node's own engine runs, which draws the Screen effect from
    /// the agent's frames, in the controller role (a source gets its
    /// configuration from its controller with RB-16).
    fn capture_wanted(&self) -> Option<Value> {
        if self.role != "controller" || !self.engine_runs() {
            return None;
        }
        capture_request(
            &self.store.get(Doc::Zones),
            &self.store.get(Doc::Show),
            &self.store.get(Doc::Sources),
        )
    }

    /// `folo reload` (ADR-0034 D7): reads `node.toml` and the documents
    /// again, applies what can change while running, and reports the rest.
    ///
    /// # Errors
    ///
    /// When `node.toml` or a document can't be read; nothing changes then.
    pub fn reload(&self) -> Result<Value, String> {
        let file_path = self.place.folders.settings_file();
        let file = match std::fs::read_to_string(&file_path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("{}: {error}", file_path.display())),
        };
        let fresh = folo_settings::layer(file.as_deref(), &env_var, &self.flags, self.place.launch)
            .map_err(|e| format!("{}: {e}", file_path.display()))?;
        let documents = self.store.reload().map_err(|e| e.to_string())?;
        let found = {
            let mut settings = self.settings();
            let found = changes(&settings.json, &fresh.json);
            // Settings that need a restart keep their running values until then
            let mut kept = fresh.clone();
            for path in &found.restart_required {
                set_at(&mut kept.json, path, at(&settings.json, path).clone());
            }
            kept.settings = serde_json::from_value(kept.json.clone())
                .map_err(|e| format!("{}: {e}", file_path.display()))?;
            *settings = kept;
            found
        };
        for warning in &fresh.warnings {
            log::warn!("{warning}");
        }
        if found.applied.contains(&"log_level") {
            logs::set_level(logs::level_of(
                fresh.json["log_level"].as_str().unwrap_or("info"),
            ));
        }
        if found.applied.contains(&"local_source.enabled") {
            self.supervisor.set_wanted(self.engine_wanted(&fresh));
        }
        if documents.contains(&Doc::Zones) {
            self.check_zones();
        }
        if !documents.is_empty() || found.applied.contains(&"local_source.enabled") {
            self.refresh_engine();
        }
        let mut applied: Vec<String> = found.applied.iter().map(|s| (*s).to_owned()).collect();
        applied.extend(documents.iter().map(|d| d.file().to_owned()));
        let result = json!({"applied": applied, "restart_required": found.restart_required});
        log::info!("reloaded: {result}");
        Ok(result)
    }
}

/// A node for the server's unit tests: its documents in `dir`, its
/// settings from `node_toml`, and no engine.
#[cfg(test)]
pub(crate) fn for_tests(dir: &std::path::Path, node_toml: &str) -> Arc<Node> {
    use folo_settings::folders::{Folders, Mode};
    let layered = folo_settings::layer(Some(node_toml), &|_| None, &[], Launch::Foreground)
        .expect("the test's node.toml reads");
    let folders = Folders {
        mode: Mode::Custom,
        home: dir.to_path_buf(),
        data: dir.to_path_buf(),
        logs: dir.join("logs"),
    };
    let place = Place {
        folders,
        layered: layered.clone(),
        launch: Launch::Foreground,
        service: false,
        exe_dir: dir.to_path_buf(),
        cwd: dir.to_path_buf(),
    };
    let engine = EngineCommand {
        program: dir.join("no-engine"),
        args: Vec::new(),
    };
    let supervisor = Supervisor::start(engine, String::new(), Arc::new(EngineLog), false);
    let capture = CaptureSupervisor::start(
        CaptureCommand::find(dir),
        String::new(),
        Arc::new(CaptureLog::new(dir, "test")),
        supervisor.screen_frames(),
        None,
        None,
    );
    Arc::new(Node {
        started: SystemTime::now(),
        place,
        id: "0123456789abcdef".into(),
        name: "test".into(),
        role: "controller".into(),
        listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8777),
        flags: Vec::new(),
        settings: Mutex::new(layered),
        store: Store::open(dir).expect("the test's data folder opens"),
        supervisor,
        capture,
        live: live::Hub::default(),
        output: output::Output::new(Registry::new()),
    })
}

/// The value at a dotted settings path (`listen.port`), or null.
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |value, key| &value[key])
}

/// Sets the value at a dotted settings path, making the objects on the way.
fn set_at(value: &mut Value, path: &str, new: Value) {
    let mut place = value;
    for key in path.split('.') {
        if !place.is_object() {
            *place = json!({});
        }
        place = &mut place[key];
    }
    *place = new;
}

/// The engine's output goes into the node's log.
struct EngineLog;

impl LogSink for EngineLog {
    fn engine_line(&self, line: &str) {
        logs::engine_line(line);
    }
}

/// The capture agent's output goes into the node's log, and the restore
/// token it returns into `secrets.json` (ADR-0037 D14).
struct CaptureLog {
    data: std::path::PathBuf,
    server_id: String,
    /// The newest token, and one write of `secrets.json` at a time: a write
    /// that runs late still stores the newest.
    newest: Arc<Mutex<Option<String>>>,
}

impl CaptureLog {
    fn new(data: &std::path::Path, server_id: &str) -> Self {
        Self {
            data: data.to_path_buf(),
            server_id: server_id.to_owned(),
            newest: Arc::new(Mutex::new(None)),
        }
    }

    fn store(data: &std::path::Path, server_id: &str, token: &str) {
        let stored = folo_store::read_secrets(data).and_then(|mut secrets| {
            secrets["capture"] = json!({"restore_token": token});
            folo_store::write_secrets(data, &secrets, server_id)
        });
        // The token is never in the message (ADR-0037 D9, D14)
        match stored {
            Ok(_) => log::debug!("kept the capture agent's new restore token"),
            Err(error) => log::error!("secrets.json can't take the restore token: {error}"),
        }
    }
}

impl CaptureSink for CaptureLog {
    fn agent_line(&self, line: &str) {
        logs::capture_line(line);
    }

    fn restore_token(&self, token: &str) {
        *self.newest.lock().unwrap_or_else(PoisonError::into_inner) = Some(token.to_owned());
        let (data, server_id, newest) = (
            self.data.clone(),
            self.server_id.clone(),
            Arc::clone(&self.newest),
        );
        // A file write with a sync, off the link's task
        let write = move || {
            let mut newest = newest.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(token) = newest.take() {
                Self::store(&data, &server_id, &token);
            }
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(write);
        } else {
            write();
        }
    }
}

/// `folo run` from the command line: a Tokio runtime, stopped by signals.
#[must_use]
pub fn main(location: &LocationArgs, args: &RunArgs) -> u8 {
    if args.managed {
        drop_folo_variables();
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("folo")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("the node can't start its runtime: {error}");
            return EXIT_FAILED;
        }
    };
    runtime.block_on(run(location, args, stop_signal()))
}

/// A managed launch removes the `FOLO_*` variables it inherited from its
/// environment (ADR-0010 D1, ADR-0034 D2), before the runtime starts any
/// thread.
#[allow(unsafe_code)] // see the SAFETY note
fn drop_folo_variables() {
    let names: Vec<std::ffi::OsString> = std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| folo_supervisor::is_folo_variable(name))
        .collect();
    for name in names {
        // SAFETY: the process has only this thread yet, so nothing reads
        // the environment while it changes.
        unsafe { std::env::remove_var(name) };
    }
}

/// Runs the node until `stop` completes; returns its exit status.
pub async fn run(
    location: &LocationArgs,
    args: &RunArgs,
    stop: impl Future<Output = ()> + Send,
) -> u8 {
    run_with(location, args, None, stop).await
}

/// [`run`] with the engine started by `engine` instead of `folo-source`,
/// as the tests do.
pub async fn run_with(
    location: &LocationArgs,
    args: &RunArgs,
    engine: Option<EngineCommand>,
    stop: impl Future<Output = ()> + Send,
) -> u8 {
    run_with_workers(location, args, engine, None, stop).await
}

/// [`run_with`] with the provider kinds in `registry`. The built-in kinds arrive with their run
/// books (RB-10, RB-12, RB-13); tests register a recording provider.
pub async fn run_with_providers(
    location: &LocationArgs,
    args: &RunArgs,
    engine: Option<EngineCommand>,
    registry: Registry,
    stop: impl Future<Output = ()> + Send,
) -> u8 {
    run_inner(location, args, engine, None, registry, stop).await
}

/// [`run_with`], and the capture agent started by `capture` instead of
/// `folo-capture`.
pub async fn run_with_workers(
    location: &LocationArgs,
    args: &RunArgs,
    engine: Option<EngineCommand>,
    capture: Option<CaptureCommand>,
    stop: impl Future<Output = ()> + Send,
) -> u8 {
    run_inner(location, args, engine, capture, Registry::new(), stop).await
}

async fn run_inner(
    location: &LocationArgs,
    args: &RunArgs,
    engine: Option<EngineCommand>,
    capture: Option<CaptureCommand>,
    registry: Registry,
    stop: impl Future<Output = ()> + Send,
) -> u8 {
    let launch = if args.managed {
        Launch::Managed
    } else {
        Launch::Foreground
    };
    let flags = args.flags();
    if launch == Launch::Managed
        && (!flags.is_empty() || location.home.is_some() || location.service)
    {
        eprintln!("--managed takes no other flags (ADR-0034 D2)");
        return EXIT_USAGE;
    }
    match start(location, flags, launch, engine, registry, capture).await {
        Ok((node, listeners, _lock)) => serve_until(node, listeners, stop).await,
        Err(failure) => {
            if log::max_level() == log::LevelFilter::Off {
                eprintln!("{}", failure.message);
            } else {
                log::error!("{}", failure.message);
            }
            logs::close_file();
            failure.status
        }
    }
}

/// Everything up to serving: the lock, the settings, the log, the first
/// run, the documents, the identity, the listeners and the supervisor.
async fn start(
    location: &LocationArgs,
    flags: Vec<(&'static str, String)>,
    launch: Launch,
    engine: Option<EngineCommand>,
    registry: Registry,
    capture: Option<CaptureCommand>,
) -> Result<(Arc<Node>, Vec<TcpListener>, lock::HomeLock), Failure> {
    let place = locate(location, &flags, launch)?;
    let folders = &place.folders;
    std::fs::create_dir_all(&folders.home)
        .map_err(|e| Failure::new(EXIT_FAILED, format!("{}: {e}", folders.home.display())))?;
    let home_lock = lock::acquire(&folders.lock_file()).map_err(|error| match error {
        lock::LockError::Held(_) => Failure::new(
            EXIT_NOT_HERE,
            format!("{error}: {}", folders.home.display()),
        ),
        lock::LockError::Io(_) => Failure::new(EXIT_FAILED, error.to_string()),
    })?;
    logs::init(logs::level_of(
        place.layered.json["log_level"].as_str().unwrap_or("info"),
    ));
    if let Err(error) = logs::open_file(&folders.log_file()) {
        log::warn!(
            "the log file {} can't be written: {error}",
            folders.log_file().display()
        );
    }
    for warning in &place.layered.warnings {
        log::warn!("{warning}");
    }
    log::info!(
        "FOLO node {} starting ({} launch, {} folders): home {}, data {}",
        env!("CARGO_PKG_VERSION"),
        launch.name(),
        folders.mode.name(),
        folders.home.display(),
        folders.data.display()
    );
    let engine = engine.unwrap_or_else(|| EngineCommand::find(&place.exe_dir));
    std::fs::create_dir_all(&folders.data)
        .map_err(|e| Failure::new(EXIT_FAILED, format!("{}: {e}", folders.data.display())))?;
    if !folders.data.join(Doc::Zones.file()).exists() {
        import::first_run(&engine, &folders.home, &place.cwd, &folders.data);
    }
    let store = Store::open(&folders.data).map_err(|e| Failure::new(EXIT_FAILED, e.to_string()))?;
    let identity = ensure_identity(&store)?;
    let role = place.layered.json["role"]
        .as_str()
        .unwrap_or("controller")
        .to_owned();
    let (listeners, listen) = bind(&place.layered).await?;
    let url = format!("ws://127.0.0.1:{}/api/v1/source-link", listen.port());
    let wanted = role == "source" || place.layered.settings.local_source.enabled;
    let supervisor = Supervisor::start(engine, url, Arc::new(EngineLog), wanted);
    let capture = capture.unwrap_or_else(|| CaptureCommand::find(&place.exe_dir));
    let capture_url = format!("ws://127.0.0.1:{}/api/v1/capture-link", listen.port());
    let restore_token = ensure_secrets(&store)?["capture"]["restore_token"]
        .as_str()
        .map(str::to_owned);
    // The agent runs in the user's session (ADR-0037 D3): a service has none
    let no_session = place
        .service
        .then(|| "the node runs as a service, outside the user's session; the desktop app starts the capture agent".to_owned());
    let capture = CaptureSupervisor::start(
        capture,
        capture_url,
        Arc::new(CaptureLog::new(store.dir(), store.server_id())),
        supervisor.screen_frames(),
        restore_token,
        no_session,
    );
    let node = Arc::new(Node {
        started: SystemTime::now(),
        id: identity.node_id(),
        name: gethostname::gethostname().to_string_lossy().into_owned(),
        role,
        listen,
        flags,
        settings: Mutex::new(place.layered.clone()),
        place,
        store,
        supervisor,
        capture,
        live: live::Hub::default(),
        output: output::Output::new(registry),
    });
    node.check_zones();
    node.refresh_engine();
    log::info!(
        "node {} ({}) listening on {listen}",
        fingerprint(&node.id),
        node.role
    );
    Ok((node, listeners, home_lock))
}

/// The node's identity from `secrets.json`, made on the first run. Keys
/// that don't fit together stop the node rather than changing its id.
fn ensure_identity(store: &Store) -> Result<Identity, Failure> {
    let failed = |message: String| Failure::new(EXIT_FAILED, message);
    let dir = store.dir();
    let mut secrets = folo_store::read_secrets(dir).map_err(|e| failed(e.to_string()))?;
    let stored = &secrets["identity"];
    let private = stored["private_key"].as_str().unwrap_or_default();
    if !private.is_empty() {
        let public = stored["public_key"].as_str().unwrap_or_default();
        return Identity::from_stored(private, public)
            .map_err(|e| failed(format!("secrets.json: the node's identity: {e}")));
    }
    let identity = Identity::generate().map_err(|e| failed(e.to_string()))?;
    secrets["identity"] = json!({
        "public_key": identity.public_key(),
        "private_key": identity.private_key(),
    });
    folo_store::write_secrets(dir, &secrets, store.server_id())
        .map_err(|e| failed(e.to_string()))?;
    log::info!(
        "made the node's identity: {}",
        fingerprint(&identity.node_id())
    );
    Ok(identity)
}

/// `secrets.json` as the node last wrote it.
fn ensure_secrets(store: &Store) -> Result<Value, Failure> {
    folo_store::read_secrets(store.dir()).map_err(|e| Failure::new(EXIT_FAILED, e.to_string()))
}

/// Binds the node's port: loopback only while remote access is off
/// (ADR-0034 D6); otherwise `listen.address`, plus `127.0.0.1` for the
/// local link and the command line when that address doesn't take it.
async fn bind(layered: &Layered) -> Result<(Vec<TcpListener>, SocketAddr), Failure> {
    let port = u16::try_from(layered.settings.listen.port.get()).unwrap_or(u16::MAX);
    let remote = layered.settings.remote_access.enabled;
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut addresses = Vec::new();
    if remote {
        let host = layered.settings.listen.address.as_str();
        let resolved = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| Failure::new(EXIT_FAILED, format!("listen.address {host}: {e}")))?;
        let first = resolved.into_iter().next().ok_or_else(|| {
            Failure::new(EXIT_FAILED, format!("listen.address {host} has no address"))
        })?;
        addresses.push(first);
        // 0.0.0.0 and 127.0.0.1 take it; so does :: on Linux, where it's
        // dual-stack, but not on Windows, where it's IPv6 only
        let takes_loopback = match first.ip() {
            IpAddr::V4(ip) => ip.is_unspecified() || ip.is_loopback(),
            IpAddr::V6(ip) => ip.is_unspecified() && !cfg!(windows),
        };
        if !takes_loopback {
            addresses.push(loopback);
        }
    } else {
        addresses.push(loopback);
    }
    let mut listeners = Vec::with_capacity(addresses.len());
    for address in &addresses {
        let listener = TcpListener::bind(address)
            .await
            .map_err(|e| Failure::new(EXIT_FAILED, format!("can't listen on {address}: {e}")))?;
        listeners.push(listener);
    }
    Ok((listeners, addresses[0]))
}

/// Serves until `stop`, then stops the output, the engine, the server and the store.
async fn serve_until(
    node: Arc<Node>,
    listeners: Vec<TcpListener>,
    stop: impl Future<Output = ()> + Send,
) -> u8 {
    let (halt, halted) = tokio::sync::watch::channel(false);
    let (output_halt, output_halted) = tokio::sync::watch::channel(false);
    let output_task = tokio::spawn(output::Output::run(Arc::clone(&node), output_halted));
    let mut servers = tokio::task::JoinSet::new();
    for listener in listeners {
        let mut halted = halted.clone();
        servers.spawn(server::serve(listener, Arc::clone(&node), async move {
            let _ = halted.wait_for(|h| *h).await;
        }));
    }
    let mut status = EXIT_OK;
    tokio::select! {
        () = stop => log::info!("stopping"),
        ended = servers.join_next() => {
            log::error!("the server stopped: {ended:?}");
            status = EXIT_FAILED;
        }
    }
    node.live.stop();
    // The output stops stepping, then its senders release their targets while the engine stops
    let _ = output_halt.send(true);
    let _ = output_task.await;
    // The agent first: no frames are left to forward to a stopping engine
    node.capture.shutdown().await;
    tokio::join!(node.supervisor.shutdown(), node.output.shutdown());
    let _ = halt.send(true);
    if tokio::time::timeout(SERVER_STOP, async {
        while servers.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        log::warn!("closing connections the server couldn't finish");
        servers.shutdown().await;
    }
    if let Err(error) = node.store.flush() {
        log::error!("{error}");
        status = EXIT_FAILED;
    }
    log::info!("stopped");
    logs::close_file();
    status
}

/// Completes on SIGINT or SIGTERM, or on the Windows console's Ctrl+C,
/// Ctrl+Break, close and shutdown events (RB-06 step 7). The Windows
/// service's stop arrives with the service in RB-20.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_close, ctrl_shutdown};
        accept_ctrl_c();
        let (Ok(mut brk), Ok(mut close), Ok(mut shutdown)) =
            (ctrl_break(), ctrl_close(), ctrl_shutdown())
        else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = brk.recv() => {}
            _ = close.recv() => {}
            _ = shutdown.recv() => {}
        }
    }
}

/// Takes Ctrl+C back when the node's parent had it turned off, which its
/// children inherit (a new process group starts with it off), as the
/// effect engine does.
#[cfg(windows)]
#[allow(unsafe_code)] // one Win32 call without pointers
fn accept_ctrl_c() {
    // SAFETY: without a handler, SetConsoleCtrlHandler only sets whether
    // this process ignores Ctrl+C.
    let _ = unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0) };
}
