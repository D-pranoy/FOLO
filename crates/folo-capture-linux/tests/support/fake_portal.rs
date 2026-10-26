//! A fake desktop portal on the tests' private session bus: the ScreenCast
//! interface (with its request and session objects), the host registry and
//! the notification service. It answers as the real portal does, with the
//! questions the tests need to ask of it: what was selected, which app id
//! registered, and what the user would have seen.

// The interface macros fix these signatures.
#![allow(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::unused_self,
    clippy::used_underscore_binding
)]

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, ObjectServer, connection::Builder, interface};

const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const SENDER_PREFIX: &str = "/org/freedesktop/portal/desktop";
/// The portal answers a request after the call returns, never inside it.
const RESPONSE_DELAY: Duration = Duration::from_millis(20);

/// How the fake portal behaves; the tests change it between sessions.
#[derive(Clone)]
pub struct Behavior {
    /// The ScreenCast interface version.
    pub version: u32,
    /// Whether the host registry exists (xdg-desktop-portal 1.19 and newer).
    pub registry: bool,
    /// Whether the registry refuses the application id it is given.
    pub registry_refuses: bool,
    /// How long the user takes over the consent dialog.
    pub dialog_time: Duration,
    /// The restore token that still holds. Any other, or none, shows the dialog.
    pub valid_token: Option<String>,
    /// The user declines the dialog.
    pub decline: bool,
    /// The PipeWire node of the screen and its size.
    pub node_id: u32,
    pub stream_size: (i32, i32),
    /// The PipeWire daemon's socket, which `OpenPipeWireRemote` connects to.
    pub pipewire_socket: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub types: Option<u32>,
    pub cursor_mode: Option<u32>,
    pub persist_mode: Option<u32>,
    pub multiple: Option<bool>,
    pub restore_token: Option<String>,
}

/// What the portal was asked, for the tests to read.
#[derive(Debug, Clone, Default)]
pub struct Log {
    pub registered: Vec<String>,
    pub selections: Vec<Selection>,
    pub starts: u32,
    pub dialogs: u32,
    pub notifications: Vec<String>,
    pub closed_by_client: u32,
    pub remotes_opened: u32,
    pub tokens_issued: u32,
}

struct Shared {
    behavior: Mutex<Behavior>,
    log: Mutex<Log>,
    /// The paths of the sessions that are open.
    sessions: Mutex<Vec<String>>,
}

enum Command {
    /// The compositor ends the share: `Closed` on every open session.
    EndShare,
}

pub struct FakePortal {
    shared: Arc<Shared>,
    commands: UnboundedSender<Command>,
}

impl FakePortal {
    pub fn start(bus_address: &str, behavior: Behavior) -> Self {
        let shared = Arc::new(Shared {
            behavior: Mutex::new(behavior),
            log: Mutex::new(Log::default()),
            sessions: Mutex::new(Vec::new()),
        });
        let (commands, mut received) = unbounded_channel();
        let (ready_sender, ready) = std::sync::mpsc::channel();
        let address = bus_address.to_owned();
        let served = Arc::clone(&shared);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let connection = serve(&address, &served).await;
                ready_sender.send(()).unwrap();
                while let Some(Command::EndShare) = received.recv().await {
                    end_share(&connection, &served).await;
                }
            });
        });
        ready
            .recv_timeout(Duration::from_secs(10))
            .expect("the fake portal didn't start");
        Self { shared, commands }
    }

    pub fn log(&self) -> Log {
        self.shared.log.lock().unwrap().clone()
    }

    pub fn change(&self, change: impl FnOnce(&mut Behavior)) {
        change(&mut self.shared.behavior.lock().unwrap());
    }

    /// The compositor or the user ends the share from the desktop's own controls.
    pub fn end_share(&self) {
        self.commands.send(Command::EndShare).unwrap();
    }
}

async fn serve(address: &str, shared: &Arc<Shared>) -> Connection {
    let registry = shared.behavior.lock().unwrap().registry;
    let mut builder = Builder::address(address)
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .serve_at(
            DESKTOP_PATH,
            ScreenCast {
                shared: Arc::clone(shared),
            },
        )
        .unwrap()
        .serve_at(
            DESKTOP_PATH,
            Notification {
                shared: Arc::clone(shared),
            },
        )
        .unwrap();
    if registry {
        builder = builder
            .serve_at(
                DESKTOP_PATH,
                Registry {
                    shared: Arc::clone(shared),
                },
            )
            .unwrap();
    }
    builder.build().await.unwrap()
}

/// `Closed` on the sessions that are open.
async fn end_share(connection: &Connection, shared: &Arc<Shared>) {
    let open = shared.sessions.lock().unwrap().clone();
    for path in open {
        connection
            .emit_signal(
                None::<&str>,
                path.as_str(),
                "org.freedesktop.portal.Session",
                "Closed",
                &HashMap::<String, OwnedValue>::new(),
            )
            .await
            .unwrap();
    }
}

/// The tail of a request or session path for the caller: `1_42/token`.
fn caller_path(header: &Header<'_>, token: &str) -> String {
    let sender = header
        .sender()
        .map(|name| name.as_str().trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default();
    format!("{sender}/{token}")
}

fn text_of(options: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|value| value.downcast_ref::<&str>().ok().map(str::to_owned))
}

fn number_of(options: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    options
        .get(key)
        .and_then(|value| value.downcast_ref::<u32>().ok())
}

/// A request object plus the `Response` signal after a delay.
async fn respond(
    connection: &Connection,
    server: &ObjectServer,
    request_path: &str,
    code: u32,
    results: HashMap<String, OwnedValue>,
    delay: Duration,
) {
    let path = OwnedObjectPath::try_from(request_path).unwrap();
    server.at(&path, Request).await.unwrap();
    let connection = connection.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        connection
            .emit_signal(
                None::<&str>,
                &path,
                "org.freedesktop.portal.Request",
                "Response",
                &(code, results),
            )
            .await
            .unwrap();
    });
}

struct Request;

#[interface(name = "org.freedesktop.portal.Request")]
impl Request {
    async fn close(&self) {}
}

struct Session {
    shared: Arc<Shared>,
}

#[interface(name = "org.freedesktop.portal.Session")]
impl Session {
    async fn close(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(header)] header: Header<'_>,
    ) {
        self.shared.log.lock().unwrap().closed_by_client += 1;
        if let Some(path) = header.path() {
            self.shared
                .sessions
                .lock()
                .unwrap()
                .retain(|open| open != path.as_str());
            let _ = server.remove::<Session, _>(path).await;
        }
    }

    #[zbus(signal)]
    async fn closed(
        emitter: &SignalEmitter<'_>,
        details: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

struct ScreenCast {
    shared: Arc<Shared>,
}

#[interface(name = "org.freedesktop.portal.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(connection)] connection: &Connection,
    ) -> OwnedObjectPath {
        let request_token = text_of(&options, "handle_token").unwrap();
        let session_token = text_of(&options, "session_handle_token").unwrap();
        let session_path = format!(
            "{SENDER_PREFIX}/session/{}",
            caller_path(&header, &session_token)
        );
        self.shared
            .sessions
            .lock()
            .unwrap()
            .push(session_path.clone());
        server
            .at(
                OwnedObjectPath::try_from(session_path.as_str()).unwrap(),
                Session {
                    shared: Arc::clone(&self.shared),
                },
            )
            .await
            .unwrap();
        let request_path = format!(
            "{SENDER_PREFIX}/request/{}",
            caller_path(&header, &request_token)
        );
        let mut results = HashMap::new();
        results.insert(
            "session_handle".to_owned(),
            OwnedValue::try_from(Value::from(session_path)).unwrap(),
        );
        respond(
            connection,
            server,
            &request_path,
            0,
            results,
            RESPONSE_DELAY,
        )
        .await;
        OwnedObjectPath::try_from(request_path).unwrap()
    }

    async fn select_sources(
        &self,
        _session: ObjectPath<'_>,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(connection)] connection: &Connection,
    ) -> OwnedObjectPath {
        let selection = Selection {
            types: number_of(&options, "types"),
            cursor_mode: number_of(&options, "cursor_mode"),
            persist_mode: number_of(&options, "persist_mode"),
            multiple: options
                .get("multiple")
                .and_then(|value| value.downcast_ref::<bool>().ok()),
            restore_token: text_of(&options, "restore_token"),
        };
        self.shared.log.lock().unwrap().selections.push(selection);
        let token = text_of(&options, "handle_token").unwrap();
        let request_path = format!("{SENDER_PREFIX}/request/{}", caller_path(&header, &token));
        respond(
            connection,
            server,
            &request_path,
            0,
            HashMap::new(),
            RESPONSE_DELAY,
        )
        .await;
        OwnedObjectPath::try_from(request_path).unwrap()
    }

    async fn start(
        &self,
        _session: ObjectPath<'_>,
        _parent_window: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(connection)] connection: &Connection,
    ) -> OwnedObjectPath {
        let behavior = self.shared.behavior.lock().unwrap().clone();
        let asked_with = self
            .shared
            .log
            .lock()
            .unwrap()
            .selections
            .last()
            .and_then(|selection| selection.restore_token.clone());
        let restored = asked_with.is_some() && asked_with == behavior.valid_token;
        let (code, delay, results) = if behavior.decline {
            (1, behavior.dialog_time, HashMap::new())
        } else {
            let mut log = self.shared.log.lock().unwrap();
            log.tokens_issued += 1;
            let delay = if restored {
                RESPONSE_DELAY
            } else {
                log.dialogs += 1;
                behavior.dialog_time
            };
            let token = format!("token-{}", log.tokens_issued);
            drop(log);
            (0, delay, start_results(&behavior, &token))
        };
        self.shared.log.lock().unwrap().starts += 1;
        let handle = text_of(&options, "handle_token").unwrap();
        let request_path = format!("{SENDER_PREFIX}/request/{}", caller_path(&header, &handle));
        respond(connection, server, &request_path, code, results, delay).await;
        OwnedObjectPath::try_from(request_path).unwrap()
    }

    async fn open_pipe_wire_remote(
        &self,
        _session: ObjectPath<'_>,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        self.shared.log.lock().unwrap().remotes_opened += 1;
        let socket = self.shared.behavior.lock().unwrap().pipewire_socket.clone();
        let stream = UnixStream::connect(socket)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        Ok(zbus::zvariant::OwnedFd::from(OwnedFd::from(stream)))
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.shared.behavior.lock().unwrap().version
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        7
    }
}

fn start_results(behavior: &Behavior, token: &str) -> HashMap<String, OwnedValue> {
    let mut properties: HashMap<String, OwnedValue> = HashMap::new();
    properties.insert(
        "size".to_owned(),
        OwnedValue::try_from(Value::from(behavior.stream_size)).unwrap(),
    );
    properties.insert(
        "source_type".to_owned(),
        OwnedValue::try_from(Value::from(1u32)).unwrap(),
    );
    let streams = vec![(behavior.node_id, properties)];
    let mut results = HashMap::new();
    results.insert(
        "streams".to_owned(),
        OwnedValue::try_from(Value::from(streams)).unwrap(),
    );
    results.insert(
        "restore_token".to_owned(),
        OwnedValue::try_from(Value::from(token.to_owned())).unwrap(),
    );
    results.insert(
        "persist_mode".to_owned(),
        OwnedValue::try_from(Value::from(2u32)).unwrap(),
    );
    results
}

struct Registry {
    shared: Arc<Shared>,
}

#[interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    async fn register(
        &self,
        app_id: &str,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.shared
            .log
            .lock()
            .unwrap()
            .registered
            .push(app_id.to_owned());
        if self.shared.behavior.lock().unwrap().registry_refuses {
            return Err(zbus::fdo::Error::InvalidArgs("Invalid app id".to_owned()));
        }
        Ok(())
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

struct Notification {
    shared: Arc<Shared>,
}

#[interface(name = "org.freedesktop.portal.Notification")]
impl Notification {
    async fn add_notification(&self, id: &str, _notification: HashMap<String, OwnedValue>) {
        self.shared
            .log
            .lock()
            .unwrap()
            .notifications
            .push(id.to_owned());
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}
