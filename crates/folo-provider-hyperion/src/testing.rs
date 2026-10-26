//! A fake Hyperion JSON server for tests (RB-10 step 1): newline-delimited JSON over TCP on
//! loopback. It records every command it receives, keeps the state the administration commands
//! change (priorities, components, brightness, source selection, the selected instance) and
//! answers `serverinfo`, `color`, `clear`, `sourceselect`, `componentstate`, `adjustment`,
//! `instance` and `authorize` the way Hyperion.NG does, with the `tan` of the request. It can
//! require a token, drop its connections and refuse new ones. Time is Tokio's, so paused tests
//! see exact instants.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::serverinfo::LedRect;

/// `count` LEDs side by side along the bottom edge of the screen.
#[must_use]
pub fn edge_leds(count: usize) -> Vec<LedRect> {
    #[allow(clippy::cast_precision_loss)]
    let width = 1.0 / count.max(1) as f64;
    (0..count)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let hmin = i as f64 * width;
            LedRect {
                hmin,
                hmax: hmin + width,
                vmin: 0.9,
                vmax: 1.0,
            }
        })
        .collect()
}

/// A command the fake received.
#[derive(Clone, Debug)]
pub struct Received {
    /// Which connection (counted from 1, in the order they were accepted).
    pub connection: u32,
    /// The Hyperion instance that connection was on.
    pub instance: u32,
    pub at: Instant,
    /// The line as it arrived, without the newline: the bytes the sender wrote.
    pub line: String,
    pub command: Value,
}

impl Received {
    /// The command's name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.command["command"].as_str().unwrap_or_default()
    }

    /// The levels of a `color` command.
    #[must_use]
    pub fn levels(&self) -> Vec<u8> {
        self.command["color"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64().and_then(|v| u8::try_from(v).ok()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

struct Entry {
    priority: i64,
    origin: String,
    owner: String,
    component: String,
}

struct InstanceData {
    number: u32,
    name: String,
    running: bool,
    leds: Vec<LedRect>,
    priorities: Vec<Entry>,
    components: Vec<(String, bool)>,
    brightness: u8,
    auto: bool,
    pinned: Option<i64>,
}

impl InstanceData {
    fn new(number: u32, name: &str, running: bool, leds: Vec<LedRect>) -> Self {
        let components = [
            "ALL",
            "SMOOTHING",
            "BLACKBORDER",
            "FORWARDER",
            "LEDDEVICE",
            "AUDIO",
            "V4L",
        ]
        .iter()
        .map(|name| ((*name).to_owned(), true))
        .collect();
        Self {
            number,
            name: name.to_owned(),
            running,
            leds,
            priorities: Vec::new(),
            components,
            brightness: 100,
            auto: true,
            pinned: None,
        }
    }

    fn visible(&self, priority: i64) -> bool {
        match self.pinned {
            Some(pinned) => pinned == priority,
            None => self.priorities.iter().all(|e| e.priority >= priority),
        }
    }

    fn priorities_json(&self) -> Vec<Value> {
        let mut entries: Vec<&Entry> = self.priorities.iter().collect();
        entries.sort_by_key(|e| e.priority);
        entries
            .iter()
            .map(|e| {
                json!({
                    "priority": e.priority,
                    "origin": e.origin,
                    "owner": e.owner,
                    "componentId": e.component,
                    "active": true,
                    "visible": self.visible(e.priority),
                })
            })
            .collect()
    }
}

struct Inner {
    instances: Vec<InstanceData>,
    token: Option<String>,
    accepting: bool,
    silent: bool,
    connections: u32,
    log: Vec<Received>,
}

struct Shared {
    inner: Mutex<Inner>,
    drop_all: watch::Sender<u64>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A Hyperion server on loopback with one running instance (number 0, 10 LEDs along the bottom
/// edge) until a test says otherwise.
pub struct FakeHyperion {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: JoinHandle<()>,
}

impl Drop for FakeHyperion {
    fn drop(&mut self) {
        self.accept.abort();
        self.shared.drop_all.send_modify(|g| *g += 1);
    }
}

impl FakeHyperion {
    /// Starts listening on a free port of 127.0.0.1.
    ///
    /// # Panics
    /// When loopback can't be bound.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local address");
        let (drop_all, _) = watch::channel(0u64);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                instances: vec![InstanceData::new(0, "FOLO test", true, edge_leds(10))],
                token: None,
                accepting: true,
                silent: false,
                connections: 0,
                log: Vec::new(),
            }),
            drop_all,
        });
        let accept = tokio::spawn(accept_loop(listener, Arc::clone(&shared)));
        Self {
            addr,
            shared,
            accept,
        }
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    #[must_use]
    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }

    /// Requires this token: every command but `authorize` fails with "No Authorization" on a
    /// connection that hasn't logged in with it.
    pub fn require_token(&self, token: &str) {
        self.shared.lock().token = Some(token.to_owned());
    }

    /// Adds an instance, or replaces the one with this number.
    pub fn set_instance(&self, number: u32, name: &str, running: bool, leds: Vec<LedRect>) {
        let mut inner = self.shared.lock();
        inner.instances.retain(|i| i.number != number);
        inner
            .instances
            .push(InstanceData::new(number, name, running, leds));
        inner.instances.sort_by_key(|i| i.number);
    }

    /// Changes the LED layout of an instance, as when its owner changes the LED count.
    pub fn set_leds(&self, number: u32, leds: Vec<LedRect>) {
        let mut inner = self.shared.lock();
        if let Some(i) = inner.instances.iter_mut().find(|i| i.number == number) {
            i.leds = leds;
        }
    }

    /// Starts or stops an instance.
    pub fn set_running(&self, number: u32, running: bool) {
        let mut inner = self.shared.lock();
        if let Some(i) = inner.instances.iter_mut().find(|i| i.number == number) {
            i.running = running;
        }
    }

    /// Puts a priority of somebody else in an instance's list, as another app's colours.
    pub fn add_priority(&self, number: u32, priority: i64, origin: &str, owner: &str) {
        let mut inner = self.shared.lock();
        if let Some(i) = inner.instances.iter_mut().find(|i| i.number == number) {
            i.priorities.push(Entry {
                priority,
                origin: origin.to_owned(),
                owner: owner.to_owned(),
                component: "COLOR".to_owned(),
            });
        }
    }

    /// Closes every open connection; new ones are still accepted.
    pub fn drop_connections(&self) {
        self.shared.drop_all.send_modify(|g| *g += 1);
    }

    /// Whether new connections are accepted: when not, they are closed at once, as when nothing
    /// listens.
    pub fn set_accepting(&self, accepting: bool) {
        self.shared.lock().accepting = accepting;
    }

    /// Whether the fake stays silent: it records commands and answers none, as a Hyperion that
    /// hangs.
    pub fn set_silent(&self, silent: bool) {
        self.shared.lock().silent = silent;
    }

    /// How many connections were accepted so far.
    #[must_use]
    pub fn connections(&self) -> u32 {
        self.shared.lock().connections
    }

    /// Every command received so far, in order.
    #[must_use]
    pub fn received(&self) -> Vec<Received> {
        self.shared.lock().log.clone()
    }

    /// The commands of one name, in order.
    #[must_use]
    pub fn commands(&self, name: &str) -> Vec<Received> {
        self.received()
            .into_iter()
            .filter(|r| r.name() == name)
            .collect()
    }

    /// The `color` commands, in order.
    #[must_use]
    pub fn color_frames(&self) -> Vec<Received> {
        self.commands("color")
    }

    /// The priorities of an instance now: `(priority, origin)`, lowest number first.
    #[must_use]
    pub fn priorities(&self, number: u32) -> Vec<(i64, String)> {
        let inner = self.shared.lock();
        let mut list: Vec<_> = inner
            .instances
            .iter()
            .find(|i| i.number == number)
            .map(|i| {
                i.priorities
                    .iter()
                    .map(|e| (e.priority, e.origin.clone()))
                    .collect()
            })
            .unwrap_or_default();
        list.sort();
        list
    }

    /// Whether a component of an instance is on.
    #[must_use]
    pub fn component(&self, number: u32, name: &str) -> Option<bool> {
        let inner = self.shared.lock();
        let instance = inner.instances.iter().find(|i| i.number == number)?;
        instance
            .components
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, on)| *on)
    }

    /// The brightness of an instance, in percent.
    #[must_use]
    pub fn brightness(&self, number: u32) -> Option<u8> {
        let inner = self.shared.lock();
        inner
            .instances
            .iter()
            .find(|i| i.number == number)
            .map(|i| i.brightness)
    }

    /// Whether an instance selects its source automatically, and the priority it is pinned to.
    #[must_use]
    pub fn source(&self, number: u32) -> Option<(bool, Option<i64>)> {
        let inner = self.shared.lock();
        inner
            .instances
            .iter()
            .find(|i| i.number == number)
            .map(|i| (i.auto, i.pinned))
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let connection = {
            let mut inner = shared.lock();
            if !inner.accepting {
                continue;
            }
            inner.connections += 1;
            inner.connections
        };
        tokio::spawn(serve(stream, connection, Arc::clone(&shared)));
    }
}

/// What one connection knows.
struct Session {
    connection: u32,
    instance: u32,
    authorized: bool,
}

async fn serve(stream: TcpStream, connection: u32, shared: Arc<Shared>) {
    let mut dropped = shared.drop_all.subscribe();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut session = {
        let inner = shared.lock();
        Session {
            connection,
            instance: inner
                .instances
                .iter()
                .find(|i| i.running)
                .map_or(0, |i| i.number),
            authorized: inner.token.is_none(),
        }
    };
    loop {
        let line = tokio::select! {
            _ = dropped.changed() => return,
            line = lines.next_line() => match line {
                Ok(Some(line)) => line,
                _ => return,
            },
        };
        let Some(reply) = handle(&shared, &mut session, &line) else {
            continue;
        };
        let mut bytes = reply.to_string().into_bytes();
        bytes.push(b'\n');
        if write.write_all(&bytes).await.is_err() {
            return;
        }
    }
}

fn failure(command: &str, tan: &Value, error: &str) -> Value {
    json!({"command": command, "error": error, "success": false, "tan": tan})
}

fn success(command: &str, tan: &Value) -> Value {
    json!({"command": command, "success": true, "tan": tan})
}

/// Records a line and answers it. `None` for a line that isn't JSON.
fn handle(shared: &Shared, session: &mut Session, line: &str) -> Option<Value> {
    let command: Value = serde_json::from_str(line).ok()?;
    let name = command["command"].as_str().unwrap_or_default().to_owned();
    let tan = command.get("tan").cloned().unwrap_or(json!(0));
    let mut inner = shared.lock();
    inner.log.push(Received {
        connection: session.connection,
        instance: session.instance,
        at: Instant::now(),
        line: line.to_owned(),
        command: command.clone(),
    });
    if inner.silent {
        return None;
    }
    if name == "authorize" {
        return Some(authorize(&inner, session, &command, &tan));
    }
    if !session.authorized {
        return Some(failure(&name, &tan, "No Authorization"));
    }
    if name == "instance" {
        return Some(switch(&inner, session, &command, &tan));
    }
    let number = session.instance;
    let Some(instance) = inner.instances.iter_mut().find(|i| i.number == number) else {
        return Some(failure(&name, &tan, "Instance not found"));
    };
    Some(match name.as_str() {
        "serverinfo" => server_info(&inner, number, &tan),
        "color" => color(instance, &command, &tan),
        "clear" => clear(instance, &command, &tan),
        "sourceselect" => source_select(instance, &command, &tan),
        "componentstate" => component_state(instance, &command, &tan),
        "adjustment" => {
            instance.brightness = command["adjustment"]["brightness"]
                .as_u64()
                .and_then(|b| u8::try_from(b).ok())
                .unwrap_or(instance.brightness);
            success(&name, &tan)
        }
        _ => failure(&name, &tan, "Unknown command"),
    })
}

fn authorize(inner: &Inner, session: &mut Session, command: &Value, tan: &Value) -> Value {
    match command["subcommand"].as_str() {
        Some("tokenRequired") => {
            let required = inner.token.is_some();
            json!({"command": "authorize-tokenRequired", "info": {"required": required},
                   "success": true, "tan": tan})
        }
        Some("login") => {
            let given = command["token"].as_str();
            if inner.token.is_none() || given == inner.token.as_deref() {
                session.authorized = true;
                success("authorize-login", tan)
            } else {
                failure("authorize-login", tan, "No Authorization")
            }
        }
        _ => failure("authorize", tan, "Unknown subcommand"),
    }
}

fn switch(inner: &Inner, session: &mut Session, command: &Value, tan: &Value) -> Value {
    let wanted = command["instance"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok());
    let found = wanted.filter(|n| inner.instances.iter().any(|i| i.number == *n && i.running));
    match found {
        Some(n) => {
            session.instance = n;
            json!({"command": "instance-switchTo", "info": {"instance": n},
                   "success": true, "tan": tan})
        }
        None => failure(
            "instance-switchTo",
            tan,
            "Selecting Hyperion instance failed",
        ),
    }
}

fn server_info(inner: &Inner, number: u32, tan: &Value) -> Value {
    let Some(me) = inner.instances.iter().find(|i| i.number == number) else {
        return failure("serverinfo", tan, "Instance not found");
    };
    let leds: Vec<Value> = me
        .leds
        .iter()
        .map(|l| json!({"hmin": l.hmin, "hmax": l.hmax, "vmin": l.vmin, "vmax": l.vmax}))
        .collect();
    let instances: Vec<Value> = inner
        .instances
        .iter()
        .map(|i| json!({"instance": i.number, "friendly_name": i.name, "running": i.running}))
        .collect();
    let components: Vec<Value> = me
        .components
        .iter()
        .map(|(name, enabled)| json!({"name": name, "enabled": enabled}))
        .collect();
    json!({
        "command": "serverinfo",
        "info": {
            "leds": leds,
            "instance": instances,
            "priorities": me.priorities_json(),
            "priorities_autoselect": me.auto,
            "components": components,
            "adjustment": [{"id": "A", "brightness": me.brightness}],
        },
        "success": true,
        "tan": tan,
    })
}

fn color(instance: &mut InstanceData, command: &Value, tan: &Value) -> Value {
    let Some(priority) = command["priority"].as_i64() else {
        return failure("color", tan, "priority is required");
    };
    let origin = command["origin"].as_str().unwrap_or("JSON API").to_owned();
    if origin.chars().count() > 20 {
        return failure("color", tan, "origin is too long");
    }
    instance.priorities.retain(|e| e.priority != priority);
    instance.priorities.push(Entry {
        priority,
        origin,
        owner: "JSON API".to_owned(),
        component: "COLOR".to_owned(),
    });
    success("color", tan)
}

fn clear(instance: &mut InstanceData, command: &Value, tan: &Value) -> Value {
    match command["priority"].as_i64() {
        Some(-1) => instance.priorities.clear(),
        Some(priority) => instance.priorities.retain(|e| e.priority != priority),
        None => return failure("clear", tan, "priority is required"),
    }
    success("clear", tan)
}

fn source_select(instance: &mut InstanceData, command: &Value, tan: &Value) -> Value {
    if command["auto"].as_bool() == Some(true) {
        instance.auto = true;
        instance.pinned = None;
        return success("sourceselect", tan);
    }
    let Some(priority) = command["priority"].as_i64() else {
        return failure("sourceselect", tan, "Priority request is invalid");
    };
    // Hyperion answers success either way, and ignores a priority it doesn't have
    if instance.priorities.iter().any(|e| e.priority == priority) {
        instance.auto = false;
        instance.pinned = Some(priority);
    }
    success("sourceselect", tan)
}

fn component_state(instance: &mut InstanceData, command: &Value, tan: &Value) -> Value {
    let state = &command["componentstate"];
    let (Some(name), Some(on)) = (state["component"].as_str(), state["state"].as_bool()) else {
        return failure("componentstate", tan, "Invalid componentstate");
    };
    match instance.components.iter_mut().find(|(n, _)| n == name) {
        Some(component) => {
            component.1 = on;
            success("componentstate", tan)
        }
        None => failure("componentstate", tan, "Unknown component"),
    }
}
