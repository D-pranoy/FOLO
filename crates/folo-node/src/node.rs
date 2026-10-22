//! `folo run` (RB-06 steps 6, 7 and 9): takes the home's lock, reads the
//! settings and documents, makes sure the node has an identity, imports a
//! legacy `studio.json` on a first run, serves the node's port, supervises
//! the effect engine, and stops cleanly on a signal.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use folo_identity::{Identity, fingerprint};
use folo_settings::{Launch, Layered, changes};
use folo_store::{Doc, Store};
use folo_supervisor::config::local_config;
use folo_supervisor::{EngineCommand, LogSink, Supervisor};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::cli::{
    EXIT_FAILED, EXIT_NOT_HERE, EXIT_OK, EXIT_USAGE, Failure, LocationArgs, Place, RunArgs,
    env_var, locate,
};
use crate::{import, lock, logs, server};

/// How long the server has to finish its connections when the node stops.
const SERVER_STOP: Duration = Duration::from_secs(3);

/// The running node, as the server's routes see it.
pub struct Node {
    pub started: SystemTime,
    pub place: Place,
    /// The node id.
    pub id: String,
    /// The role and the address it listens on, as started: they change
    /// only with a restart.
    pub role: String,
    pub listen: SocketAddr,
    flags: Vec<(&'static str, String)>,
    settings: Mutex<Layered>,
    pub store: Store,
    pub supervisor: Supervisor,
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

    /// The engine's configuration: built from the documents in the controller
    /// role; none in the source role until its controller sends one (RB-16).
    fn engine_config(&self) -> Option<Value> {
        (self.role == "controller").then(|| {
            local_config(
                &self.store.get(Doc::Zones),
                &self.store.get(Doc::Show),
                &self.store.get(Doc::Sources),
            )
        })
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
            *settings = fresh.clone();
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
        if !documents.is_empty() {
            self.supervisor.set_config(self.engine_config());
        }
        let mut applied: Vec<String> = found.applied.iter().map(|s| (*s).to_owned()).collect();
        applied.extend(documents.iter().map(|d| d.file().to_owned()));
        let result = json!({"applied": applied, "restart_required": found.restart_required});
        log::info!("reloaded: {result}");
        Ok(result)
    }
}

/// The engine's output goes into the node's log.
struct EngineLog;

impl LogSink for EngineLog {
    fn engine_line(&self, line: &str) {
        logs::engine_line(line);
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
        .filter(|name| name.to_string_lossy().starts_with("FOLO_"))
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
    match start(location, flags, launch, engine).await {
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
    let node = Arc::new(Node {
        started: SystemTime::now(),
        id: identity.node_id(),
        role,
        listen,
        flags,
        settings: Mutex::new(place.layered.clone()),
        place,
        store,
        supervisor,
    });
    node.supervisor.set_config(node.engine_config());
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

/// Binds the node's port: loopback only while remote access is off
/// (ADR-0034 D6); otherwise `listen.address`, plus loopback for the local
/// link when that address isn't loopback or every address.
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
        if !first.ip().is_unspecified() && !first.ip().is_loopback() {
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

/// Serves until `stop`, then stops the engine, the server and the store.
async fn serve_until(
    node: Arc<Node>,
    listeners: Vec<TcpListener>,
    stop: impl Future<Output = ()> + Send,
) -> u8 {
    let (halt, halted) = tokio::sync::watch::channel(false);
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
    node.supervisor.shutdown().await;
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
