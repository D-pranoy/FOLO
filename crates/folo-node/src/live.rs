//! The live channel (RB-07 step 4, docs/specs/client-api.md#live-websocket,
//! ADR-0035 D4 to D6): topic subscriptions, `status` and `levels` messages,
//! `config_changed`, and the engine's frames re-indexed per connection.
//! Each connection gets at most 30 updates a second, each with the newest
//! data, so nothing queues up for a slow client.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use folo_controller::ZoneLive;
use folo_store::Doc;
use folo_supervisor::config::{LOCAL_SOURCE, Wanted};
use folo_supervisor::{Received, WorkerState};
use serde_json::{Map, Value, json};
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use crate::node::Node;
use crate::server::not_a_websocket;
use crate::zones;

/// Updates per second per connection.
const RATE_HZ: u64 = 30;
/// Topics a connection may hold.
const MAX_TOPICS: usize = 64;
/// The largest message a client may send.
const MAX_MESSAGE: usize = 64 * 1024;
/// The longest name a topic may carry (live.json's `Topic`).
const MAX_NAME: usize = 64;
/// Closing a connection: a client message that isn't one of the three
/// (policy violation), and the node stopping (going away).
const CLOSE_POLICY: u16 = 1008;
const CLOSE_GOING_AWAY: u16 = 1001;
/// How long a client may take to accept a message before it's dropped.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// Frame types on the link (docs/specs/source-link.md#messages-inside-the-noise-channel).
const SURFACE_FRAME: u8 = 0x02;
const SIGNAL_FRAME: u8 = 0x03;
/// Where a frame's index is, without its type byte: surface and signal frames.
const SURFACE_INDEX_AT: usize = 8;
const SIGNAL_INDEX_AT: usize = 6;
/// Signal frames: the header's size, and where the count of values is.
const SIGNAL_HEADER: usize = 24;
const SIGNAL_COUNT_AT: usize = 8;
/// The documents `config_changed` names.
const DOCUMENTS: [(Doc, &str); 5] = [
    (Doc::Zones, "zones"),
    (Doc::Show, "show"),
    (Doc::Providers, "providers"),
    (Doc::Sources, "sources"),
    (Doc::Clients, "clients"),
];

/// Every connection's wants from the local engine, so the node asks it
/// only for what some client watches.
pub struct Hub {
    clients: Mutex<HashMap<u64, Wanted>>,
    next: AtomicU64,
    stopping: watch::Sender<bool>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            stopping: watch::channel(false).0,
        }
    }
}

impl Hub {
    fn clients(&self) -> MutexGuard<'_, HashMap<u64, Wanted>> {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What some connection wants from the local engine.
    #[must_use]
    pub fn wanted(&self) -> Wanted {
        self.clients()
            .values()
            .fold(Wanted::default(), |all, one| Wanted {
                beat: all.beat || one.beat,
                spectrum: all.spectrum || one.spectrum,
                levels: all.levels || one.levels,
            })
    }

    fn join(&self) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.clients().insert(id, Wanted::default());
        id
    }

    /// Sets a connection's wants; true when what all of them want changed.
    fn set(&self, client: u64, wanted: Wanted) -> bool {
        let before = self.wanted();
        self.clients().insert(client, wanted);
        self.wanted() != before
    }

    /// Forgets a connection; true when what all of them want changed.
    fn leave(&self, client: u64) -> bool {
        let before = self.wanted();
        self.clients().remove(&client);
        self.wanted() != before
    }

    /// Closes every connection: the node is stopping.
    pub fn stop(&self) {
        self.stopping.send_replace(true);
    }
}

/// `GET /api/v1/live`.
pub async fn live(
    State(node): State<Arc<Node>>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Ok(upgrade) = upgrade else {
        return not_a_websocket();
    };
    upgrade
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| session(node, socket))
}

/// What a topic is about (live.json's `Topic`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Topic {
    Status,
    Zone(String),
    Input(String),
    Beat(String),
    Media(String),
    Levels(String),
    Spectrum(String),
    Surface(String),
    Target(String),
}

impl Topic {
    fn parse(text: &str) -> Option<Self> {
        if text == "status" {
            return Some(Self::Status);
        }
        let (kind, name) = text.split_once(':')?;
        let valid = !name.is_empty()
            && name.chars().count() <= MAX_NAME
            && !name.chars().any(|c| c == ':' || c.is_whitespace());
        if !valid {
            return None;
        }
        let name = name.to_owned();
        Some(match kind {
            "zone" => Self::Zone(name),
            "input" => Self::Input(name),
            "beat" => Self::Beat(name),
            "media" => Self::Media(name),
            "levels" => Self::Levels(name),
            "spectrum" => Self::Spectrum(name),
            "surface" => Self::Surface(name),
            "target" => Self::Target(name),
            _ => return None,
        })
    }

    /// Whether its data arrives as binary frames, with an index.
    const fn binary(&self) -> bool {
        matches!(self, Self::Spectrum(_) | Self::Surface(_) | Self::Target(_))
    }
}

/// What a client sends (live.json's `LiveClientMessage`).
#[derive(Debug, PartialEq, Eq)]
enum ClientMessage {
    Subscribe(Vec<(String, Topic)>),
    Unsubscribe(Vec<(String, Topic)>),
    Auth,
}

impl ClientMessage {
    /// A client message, or `None` for anything else.
    fn parse(text: &str) -> Option<Self> {
        let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else {
            return None;
        };
        let (key, value) = message.iter().next().filter(|_| message.len() == 1)?;
        let topics = || -> Option<Vec<(String, Topic)>> {
            let list = value.as_array().filter(|l| l.len() <= MAX_TOPICS)?;
            list.iter()
                .map(|t| {
                    let text = t.as_str()?;
                    Some((text.to_owned(), Topic::parse(text)?))
                })
                .collect()
        };
        match key.as_str() {
            "subscribe" => topics().map(Self::Subscribe),
            "unsubscribe" => topics().map(Self::Unsubscribe),
            "auth" => {
                let auth = value.as_object().filter(|a| a.len() == 1)?;
                let key = auth.get("key")?.as_str()?;
                (key.chars().count() <= 128).then_some(Self::Auth)
            }
            _ => None,
        }
    }
}

/// One connection's subscriptions and what it was last sent.
#[derive(Default)]
struct Client {
    /// Topic text: the topic, and its index when it's binary.
    topics: BTreeMap<String, (Topic, Option<u16>)>,
    /// The last `status` message sent.
    status: Option<Value>,
    /// The update of the last frame or `levels` message sent, by topic
    /// (and surface index for `levels`).
    sent: HashMap<(String, u16), u64>,
    /// The revision of each document as last told.
    revisions: HashMap<&'static str, u64>,
}

impl Client {
    fn new(node: &Node) -> Self {
        let mut client = Self::default();
        for (doc, name) in DOCUMENTS {
            client.revisions.insert(name, revision(node, doc));
        }
        client
    }

    /// Adds topics; answers with `subscribed`.
    fn subscribe(&mut self, topics: Vec<(String, Topic)>) -> Message {
        let mut added = Map::new();
        for (text, topic) in topics {
            if self.topics.contains_key(&text) || self.topics.len() >= MAX_TOPICS {
                continue;
            }
            let index = topic.binary().then(|| self.free_index());
            let entry = index.map_or_else(|| json!({}), |i| json!({"index": i}));
            added.insert(text.clone(), entry);
            self.topics.insert(text, (topic, index));
        }
        text(&json!({"type": "subscribed", "data": {"topics": added}}))
    }

    fn unsubscribe(&mut self, topics: Vec<(String, Topic)>) {
        for (text, _) in topics {
            self.topics.remove(&text);
            self.sent.retain(|(topic, _), _| *topic != text);
        }
    }

    /// The lowest index none of the connection's binary topics uses.
    fn free_index(&self) -> u16 {
        let used: Vec<u16> = self.topics.values().filter_map(|(_, i)| *i).collect();
        (0..=u16::MAX)
            .find(|i| !used.contains(i))
            .unwrap_or(u16::MAX)
    }

    /// What it wants from the local engine.
    fn wanted(&self) -> Wanted {
        let local = |name: &String| name == LOCAL_SOURCE;
        let mut wanted = Wanted::default();
        for (topic, _) in self.topics.values() {
            match topic {
                Topic::Beat(name) if local(name) => wanted.beat = true,
                Topic::Spectrum(name) if local(name) => wanted.spectrum = true,
                Topic::Levels(name) if local(name) => wanted.levels = true,
                _ => {}
            }
        }
        wanted
    }

    /// Everything that changed since the last update, newest data only.
    fn update(&mut self, node: &Node) -> Vec<Message> {
        let mut out = Vec::new();
        for (doc, name) in DOCUMENTS {
            let revision = revision(node, doc);
            if self.revisions.insert(name, revision) != Some(revision) {
                let data = json!({"document": name, "revision": revision});
                out.push(text(&json!({"type": "config_changed", "data": data})));
            }
        }
        if let Some(status) = self.status_of(node)
            && self.status.as_ref() != Some(&status)
        {
            out.push(text(&json!({"type": "status", "data": status})));
            self.status = Some(status);
        }
        if node.supervisor.linked() {
            let config = ConfigSummary::now(node);
            node.supervisor
                .received(|received| self.engine_data(&config, received, &mut out));
        }
        self.target_frames(node, &mut out);
        out
    }

    /// The `target:<id>` frames: the targets' projected colours, or their test patterns
    /// (ADR-0036 D19), whether or not an engine is linked.
    fn target_frames(&mut self, node: &Node, out: &mut Vec<Message>) {
        node.output.published(|published| {
            for (text, (topic, index)) in &self.topics {
                let (Topic::Target(id), Some(index)) = (topic, index) else {
                    continue;
                };
                let Some(frame) = published.targets.get(id) else {
                    continue;
                };
                let key = (text.clone(), 0);
                if self.sent.insert(key, frame.update) != Some(frame.update) {
                    out.push(reindexed(&frame.value, SURFACE_INDEX_AT, *index));
                }
            }
        });
    }

    /// `levels` messages and frames from the local engine.
    fn engine_data(&mut self, config: &ConfigSummary, received: &Received, out: &mut Vec<Message>) {
        for (text, (topic, index)) in &self.topics {
            match topic {
                Topic::Levels(name) if name == LOCAL_SOURCE => {
                    for (surface, levels) in &received.levels {
                        let key = (text.clone(), *surface);
                        let Some(id) = config.surfaces.get(usize::from(*surface)) else {
                            continue;
                        };
                        if self.sent.get(&key) == Some(&levels.update) {
                            continue;
                        }
                        self.sent.insert(key, levels.update);
                        let data = json!({
                            "source": LOCAL_SOURCE,
                            "surface": id,
                            "layers": levels.value["layers"],
                        });
                        out.push(self::text(&json!({"type": "levels", "data": data})));
                    }
                }
                Topic::Spectrum(name) if name == LOCAL_SOURCE => {
                    let Some(signal) = config.signal("spectrum") else {
                        continue;
                    };
                    let frame = received.frames.get(&(SIGNAL_FRAME, signal));
                    if let (Some(frame), Some(index)) = (frame, index) {
                        let key = (text.clone(), 0);
                        if self.sent.insert(key, frame.update) != Some(frame.update) {
                            out.push(reindexed(&frame.value, SIGNAL_INDEX_AT, *index));
                        }
                    }
                }
                Topic::Surface(id) => {
                    let Some(surface) = config.surfaces.iter().position(|s| s == id) else {
                        continue;
                    };
                    let surface = u16::try_from(surface).unwrap_or(u16::MAX);
                    let frame = received.frames.get(&(SURFACE_FRAME, surface));
                    if let (Some(frame), Some(index)) = (frame, index) {
                        let key = (text.clone(), 0);
                        if self.sent.insert(key, frame.update) != Some(frame.update) {
                            out.push(reindexed(&frame.value, SURFACE_INDEX_AT, *index));
                        }
                    }
                }
                // Targets' colours come from the output (`target_frames`)
                _ => {}
            }
        }
    }

    /// The `status` message's data for this connection's topics, or `None`
    /// when it has none of the topics sent as `status` messages.
    fn status_of(&self, node: &Node) -> Option<Value> {
        let all = self.topics.values().any(|(t, _)| *t == Topic::Status);
        let mut sources = Map::new();
        let mut zones = Map::new();
        let mut providers = Map::new();
        let mut any = all;
        let known = known_sources(node);
        let zones_doc = node.store.get(Doc::Zones);
        let zone_ids = ids(&zones_doc, "zones");
        // The effective state, not the stored flag (ADR-0036 D3)
        let active = zones::check(&zones_doc);
        let reactive = node.output.published(|p| p.zones.clone());
        let zone_live = |id: &str| zone_live(active.is_active(id), reactive.get(id));
        if all {
            for id in &known {
                sources.insert(id.clone(), json!({"state": source_state(node, id)}));
            }
            for id in &zone_ids {
                zones.insert(id.clone(), zone_live(id));
            }
            for id in ids(&node.store.get(Doc::Providers), "providers") {
                let status = node.output.provider_status(&id);
                let mut live = json!({"state": status.state.live()});
                if let Some(message) = status.message {
                    live["message"] = json!(message.chars().take(500).collect::<String>());
                }
                providers.insert(id, live);
            }
        }
        for (topic, _) in self.topics.values() {
            match topic {
                Topic::Zone(id) => {
                    any = true;
                    if zone_ids.contains(id) {
                        zones.insert(id.clone(), zone_live(id));
                    }
                }
                Topic::Input(id) | Topic::Beat(id) | Topic::Media(id) => {
                    any = true;
                    if !known.contains(id) {
                        continue;
                    }
                    let entry = sources
                        .entry(id.clone())
                        .or_insert_with(|| json!({"state": source_state(node, id)}));
                    if let Some(part) = source_part(node, topic) {
                        let name = match topic {
                            Topic::Input(_) => "input",
                            Topic::Beat(_) => "beat",
                            _ => "media",
                        };
                        entry[name] = part;
                    }
                }
                _ => {}
            }
        }
        any.then(|| json!({"sources": sources, "zones": zones, "providers": providers}))
    }
}

/// A zone's live state: whether it is effectively active, and its reactive state as the output
/// last stepped it (an inactive zone has no ambient light and counts no silence).
fn zone_live(active: bool, reactive: Option<&ZoneLive>) -> Value {
    match reactive.filter(|r| active && r.active) {
        Some(r) => {
            json!({"active": true, "ambient": r.ambient.clamp(0.0, 1.0), "quiet_s": r.quiet_s.max(0.0)})
        }
        None => json!({"active": active, "ambient": 0, "quiet_s": 0}),
    }
}

/// The ids of a list document's items.
fn ids(document: &Value, key: &str) -> Vec<String> {
    document[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect()
}

fn revision(node: &Node, doc: Doc) -> u64 {
    node.store.get(doc)["revision"].as_u64().unwrap_or(0)
}

/// The sources there are: those in `sources.json`, and the local engine
/// while the node runs it.
fn known_sources(node: &Node) -> Vec<String> {
    let mut known = ids(&node.store.get(Doc::Sources), "sources");
    if node.engine_runs() && !known.iter().any(|id| id == LOCAL_SOURCE) {
        known.push(LOCAL_SOURCE.to_owned());
    }
    known
}

/// A source's `state` (docs/specs/client-api.md#topics-and-delivery): only
/// the local engine links before RB-16.
fn source_state(node: &Node, id: &str) -> &'static str {
    if id != LOCAL_SOURCE || !node.engine_runs() {
        return "offline";
    }
    if node.supervisor.linked() {
        return "connected";
    }
    match node.supervisor.status().state {
        WorkerState::Starting | WorkerState::Running => "connecting",
        WorkerState::Stopped | WorkerState::Failed => "offline",
    }
}

/// The input, beat or now playing of a source, while its engine sends them.
fn source_part(node: &Node, topic: &Topic) -> Option<Value> {
    let (Topic::Input(id) | Topic::Beat(id) | Topic::Media(id)) = topic else {
        return None;
    };
    if id != LOCAL_SOURCE || !node.supervisor.linked() {
        return None;
    }
    match topic {
        Topic::Input(_) => node
            .supervisor
            .received(|r| r.notifications.get("input").map(|n| n.value.clone())),
        Topic::Media(_) => node
            .supervisor
            .received(|r| r.notifications.get("media").map(|n| n.value.clone())),
        _ => {
            let signal = ConfigSummary::now(node).signal("beat")?;
            let frame = node.supervisor.received(|r| {
                r.frames
                    .get(&(SIGNAL_FRAME, signal))
                    .map(|f| Arc::clone(&f.value))
            })?;
            beat_status(&signal_values(&frame))
        }
    }
}

/// A signal frame's values (without its type byte).
fn signal_values(frame: &[u8]) -> Vec<f32> {
    let count = frame
        .get(SIGNAL_COUNT_AT..SIGNAL_COUNT_AT + 2)
        .map_or(0, |c| usize::from(u16::from_le_bytes([c[0], c[1]])));
    frame
        .get(SIGNAL_HEADER..SIGNAL_HEADER + 4 * count)
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect()
}

/// live.json's `BeatStatus` from the beat signal's
/// `[bpm, confidence, locked, phase, bar_beat]`.
fn beat_status(values: &[f32]) -> Option<Value> {
    let [bpm, confidence, locked, phase, bar_beat] = values else {
        return None;
    };
    let unit = |v: f32| f64::from(v).clamp(0.0, 1.0);
    // Rounded and clamped to 0-3 first, so the cast loses nothing
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bar_beat = f64::from(*bar_beat).round().clamp(0.0, 3.0) as u8;
    Some(json!({
        "bpm": f64::from(*bpm).max(0.0),
        "confidence": unit(*confidence),
        "locked": *locked >= 0.5,
        "phase": unit(*phase),
        "bar_beat": bar_beat,
    }))
}

/// A frame with its index replaced, as a WebSocket message.
fn reindexed(frame: &[u8], index_at: usize, index: u16) -> Message {
    let mut bytes = frame.to_vec();
    if let Some(slot) = bytes.get_mut(index_at..index_at + 2) {
        slot.copy_from_slice(&index.to_le_bytes());
    }
    Message::Binary(bytes.into())
}

fn text(value: &Value) -> Message {
    Message::Text(value.to_string().into())
}

/// What the live channel needs of the engine's configuration: its surfaces'
/// ids by index, and its signals' indices by name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConfigSummary {
    pub surfaces: Vec<String>,
    pub signals: Vec<(String, u16)>,
}

impl ConfigSummary {
    /// From `config.apply`'s params.
    #[must_use]
    pub fn of(config: &Value) -> Self {
        let surfaces = config["surfaces"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| s["surface"]["id"].as_str().unwrap_or_default().to_owned())
            .collect();
        let signals = config["publish"]["signals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let index = u16::try_from(s["index"].as_u64()?).ok()?;
                Some((s["name"].as_str()?.to_owned(), index))
            })
            .collect();
        Self { surfaces, signals }
    }

    /// The configuration the engine has now.
    fn now(node: &Node) -> Self {
        node.supervisor
            .with_config(|config| config.map(Self::of).unwrap_or_default())
    }

    fn signal(&self, name: &str) -> Option<u16> {
        self.signals
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, i)| *i)
    }
}

async fn session(node: Arc<Node>, mut socket: WebSocket) {
    let id = node.live.join();
    let mut client = Client::new(&node);
    let mut stopping = node.live.stopping.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(1000 / RATE_HZ));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    'session: loop {
        tokio::select! {
            message = socket.recv() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(Message::Binary(_))) => {
                        close(&mut socket, CLOSE_POLICY, "binary messages aren't taken").await;
                        break;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                };
                match ClientMessage::parse(text.as_str()) {
                    Some(ClientMessage::Subscribe(topics)) => {
                        let answer = client.subscribe(topics);
                        if !deliver(&mut socket, answer).await {
                            break;
                        }
                    }
                    Some(ClientMessage::Unsubscribe(topics)) => client.unsubscribe(topics),
                    // Loopback clients need no key; keys arrive with RB-15
                    Some(ClientMessage::Auth) => {}
                    None => {
                        close(&mut socket, CLOSE_POLICY, "not a subscribe, unsubscribe or auth message").await;
                        break;
                    }
                }
                if node.live.set(id, client.wanted()) {
                    node.refresh_engine();
                }
            }
            _ = tick.tick() => {
                for message in client.update(&node) {
                    if !deliver(&mut socket, message).await {
                        break 'session;
                    }
                }
            }
            () = async { let _ = stopping.wait_for(|stop| *stop).await; } => {
                close(&mut socket, CLOSE_GOING_AWAY, "the node is stopping").await;
                break;
            }
        }
    }
    if node.live.leave(id) {
        node.refresh_engine();
    }
}

/// Sends a message, or gives up on a client that hasn't taken it within
/// [`SEND_TIMEOUT`]: a vanished client mustn't keep its topics wanted.
async fn deliver(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, socket.send(message)).await,
        Ok(Ok(()))
    )
}

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    deliver(socket, Message::Close(Some(frame))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_follow_the_contract() {
        assert_eq!(Topic::parse("status"), Some(Topic::Status));
        assert_eq!(
            Topic::parse("beat:local"),
            Some(Topic::Beat("local".into()))
        );
        assert_eq!(
            Topic::parse("surface:main"),
            Some(Topic::Surface("main".into()))
        );
        for bad in [
            "", "beat", "beat:", "beat:a b", "beat:a:b", "colour:x", "Status",
        ] {
            assert_eq!(Topic::parse(bad), None, "{bad}");
        }
        assert!(Topic::parse(&format!("zone:{}", "z".repeat(64))).is_some());
        assert!(Topic::parse(&format!("zone:{}", "z".repeat(65))).is_none());
    }

    #[test]
    fn client_messages_are_the_three_kinds() {
        let subscribe = ClientMessage::parse(r#"{"subscribe": ["status", "spectrum:local"]}"#);
        assert_eq!(
            subscribe,
            Some(ClientMessage::Subscribe(vec![
                ("status".into(), Topic::Status),
                ("spectrum:local".into(), Topic::Spectrum("local".into())),
            ]))
        );
        assert_eq!(
            ClientMessage::parse(r#"{"auth": {"key": "folo_c_x"}}"#),
            Some(ClientMessage::Auth)
        );
        for bad in [
            "hello",
            "[]",
            r#"{"subscribe": "status"}"#,
            r#"{"subscribe": ["nope"]}"#,
            r#"{"subscribe": [], "unsubscribe": []}"#,
            r#"{"auth": {"key": 1}}"#,
            r#"{"auth": {"key": "k", "more": 1}}"#,
            r#"{"ping": true}"#,
        ] {
            assert_eq!(ClientMessage::parse(bad), None, "{bad}");
        }
        let many: Vec<String> = (0..65).map(|i| format!("\"zone:z{i}\"")).collect();
        let too_many = format!("{{\"subscribe\": [{}]}}", many.join(","));
        assert_eq!(ClientMessage::parse(&too_many), None);
    }

    fn subscribed(message: &Message) -> Value {
        let Message::Text(text) = message else {
            panic!("not text: {message:?}");
        };
        let value: Value = serde_json::from_str(text.as_str()).unwrap();
        assert_eq!(value["type"], "subscribed");
        value["data"]["topics"].clone()
    }

    fn topics(texts: &[&str]) -> Vec<(String, Topic)> {
        texts
            .iter()
            .map(|t| ((*t).to_owned(), Topic::parse(t).unwrap()))
            .collect()
    }

    #[test]
    fn binary_topics_take_the_lowest_free_index() {
        let mut client = Client::default();
        let answer = client.subscribe(topics(&["status", "spectrum:local", "surface:main"]));
        assert_eq!(
            subscribed(&answer),
            json!({"status": {}, "spectrum:local": {"index": 0}, "surface:main": {"index": 1}})
        );
        client.unsubscribe(topics(&["spectrum:local"]));
        let answer = client.subscribe(topics(&["surface:wall", "status"]));
        assert_eq!(
            subscribed(&answer),
            json!({"surface:wall": {"index": 0}}),
            "already subscribed topics aren't added again"
        );
        assert_eq!(
            client.wanted(),
            Wanted::default(),
            "nothing asked of the engine"
        );
        client.subscribe(topics(&["beat:local", "levels:local", "beat:remote"]));
        let wanted = client.wanted();
        assert!(wanted.beat && wanted.levels && !wanted.spectrum);
    }

    #[test]
    fn a_connection_holds_at_most_64_topics() {
        let mut client = Client::default();
        let first: Vec<String> = (0..60).map(|i| format!("zone:z{i}")).collect();
        let first: Vec<&str> = first.iter().map(String::as_str).collect();
        client.subscribe(topics(&first));
        let more: Vec<String> = (60..70).map(|i| format!("zone:z{i}")).collect();
        let more: Vec<&str> = more.iter().map(String::as_str).collect();
        let answer = subscribed(&client.subscribe(topics(&more)));
        assert_eq!(answer.as_object().unwrap().len(), 4);
        assert_eq!(client.topics.len(), MAX_TOPICS);
    }

    #[test]
    fn frames_are_reindexed_and_beats_read_from_their_signal() {
        let mut frame = b"FOLG".to_vec();
        frame.extend_from_slice(&[1, 0, 7, 0, 5, 0, 0, 0]);
        frame.extend_from_slice(&[0; 12]);
        for value in [120.0_f32, 0.7, 1.0, 0.25, 2.0] {
            frame.extend_from_slice(&value.to_le_bytes());
        }
        let Message::Binary(bytes) = reindexed(&frame, SIGNAL_INDEX_AT, 3) else {
            panic!("binary");
        };
        assert_eq!(&bytes[6..8], &[3, 0]);
        assert_eq!(&bytes[..6], &frame[..6]);
        let beat = beat_status(&signal_values(&frame)).unwrap();
        assert_eq!(beat["locked"], true);
        assert_eq!(beat["bar_beat"], 2);
        assert!((beat["bpm"].as_f64().unwrap() - 120.0).abs() < 1e-6);
        assert_eq!(beat_status(&[1.0, 2.0]), None);
    }

    #[test]
    fn wants_add_up_across_connections() {
        let hub = Hub::default();
        let (a, b) = (hub.join(), hub.join());
        let beat = Wanted {
            beat: true,
            ..Wanted::default()
        };
        assert!(hub.set(a, beat));
        assert!(!hub.set(b, beat), "already wanted");
        assert!(!hub.leave(a), "b still wants it");
        assert!(hub.leave(b));
        assert_eq!(hub.wanted(), Wanted::default());
    }
}
