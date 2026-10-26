//! The node's end of the local link (docs/specs/source-link.md#the-local-link,
//! ADR-0032): the handshake with the launch token, `config.apply`, the
//! heartbeat, and requests to the engine. The socket is abstract, so the
//! session runs over the node's HTTP server and over channels in tests.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Value, json};
use subtle::ConstantTimeEq as _;
use tokio::sync::oneshot;
use tokio::time::{Instant, sleep_until, timeout};

use crate::{Event, Received, Shared, Stamped};

/// The WebSocket subprotocol of the source link.
pub const SUBPROTOCOL: &str = "folo.source-link.v1";
/// The source-link versions the node speaks, newest first: this contract
/// and one minor version older (docs/specs/README.md#versioning).
pub const VERSIONS: [&str; 2] = ["1.3", "1.2"];
/// How long the engine has for its handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Silence after which the engine counts as hung (ADR-0032 D4).
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

/// Message type bytes (docs/specs/source-link.md#messages-inside-the-noise-channel).
const TYPE_RPC: u8 = 0x01;
const TYPE_SURFACE: u8 = 0x02;
const TYPE_SIGNAL: u8 = 0x03;

/// A WebSocket message on the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

/// The link's WebSocket, as the node's server hands it over.
pub trait LinkSocket: Send {
    /// The next message; `None` once the connection is closed.
    fn recv(&mut self) -> impl Future<Output = Option<Message>> + Send;
    /// Sends a message; false once the connection is closed.
    fn send(&mut self, message: Message) -> impl Future<Output = bool> + Send;
}

/// What the node asks of the engine.
#[derive(Debug)]
pub enum Request {
    /// `restart {component: engine}`: the engine ends with status 0.
    RestartEngine,
    /// A JSON-RPC request whose answer goes to `reply`.
    Call {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, RpcError>>,
    },
}

/// A JSON-RPC error answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// Where the answers to pending requests go, by request id.
type Pending = HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>;

/// Why a connection was turned away.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    /// Not a `local` hello with the current launch token, or no common version.
    Rejected(String),
    /// Another engine is linked already; the connection is closed at once.
    Busy,
}

/// Serves one connection to the link's endpoint until it ends.
pub(crate) async fn serve<S: LinkSocket>(shared: &Shared, mut socket: S, loopback: bool) {
    let Ok(Some(Message::Text(first))) = timeout(HANDSHAKE_TIMEOUT, socket.recv()).await else {
        log::warn!("a connection to the source link sent no handshake");
        return;
    };
    let mut generation = shared.generation.subscribe();
    let engine = *generation.borrow_and_update();
    let accepted = check_hello(shared, &first, loopback);
    let version = match accepted {
        Ok(version) => version,
        Err(Refusal::Busy) => {
            log::warn!("refused a second connection to the local link");
            return;
        }
        Err(Refusal::Rejected(why)) => {
            log::warn!("refused a connection to the local link: {why}");
            let welcome = json!({"version": VERSIONS[0], "result": "rejected"});
            let _ = socket.send(Message::Text(welcome.to_string())).await;
            return;
        }
    };
    let welcome = json!({"version": version, "result": "ok"});
    if !socket.send(Message::Text(welcome.to_string())).await {
        shared.release(engine);
        return;
    }
    log::info!("the effect engine linked (source link {version})");
    let ended = session(shared, &mut socket, &mut generation, engine).await;
    shared.release(engine);
    let _ = shared.events.send((engine, ended));
}

/// The source-link version both sides speak, or why the hello is refused.
/// Accepting marks the link as taken.
fn check_hello(shared: &Shared, first: &str, loopback: bool) -> Result<&'static str, Refusal> {
    let hello: Value = serde_json::from_str(first)
        .map_err(|_| Refusal::Rejected("the hello isn't JSON".into()))?;
    let purpose = &hello["purpose"];
    if purpose["kind"] != "local" {
        return Err(Refusal::Rejected("only the local purpose is served".into()));
    }
    if !loopback {
        return Err(Refusal::Rejected(
            "the local purpose is taken from loopback clients only".into(),
        ));
    }
    let offered = purpose["token"].as_str().unwrap_or_default();
    let mut state = shared.lock();
    let Some(token) = state.token.as_deref() else {
        return Err(Refusal::Rejected("no engine is expected".into()));
    };
    if !bool::from(token.as_bytes().ct_eq(offered.as_bytes())) {
        return Err(Refusal::Rejected("wrong launch token".into()));
    }
    if state.linked {
        return Err(Refusal::Busy);
    }
    let versions: Vec<&str> = hello["version"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let version = VERSIONS
        .into_iter()
        .find(|v| versions.contains(v))
        .ok_or_else(|| Refusal::Rejected(format!("no common version in {versions:?}")))?;
    state.linked = true;
    // A new link starts with nothing received: what an earlier engine sent is stale
    state.received = Received {
        updates: state.received.updates,
        ..Received::default()
    };
    Ok(version)
}

/// The linked session: sends `config.apply` and requests, watches the
/// heartbeat, and keeps what the engine sends. Returns why it ended.
async fn session<S: LinkSocket>(
    shared: &Shared,
    socket: &mut S,
    generation: &mut tokio::sync::watch::Receiver<u64>,
    engine: u64,
) -> Event {
    let mut config = shared.config.subscribe();
    // Frames come as they arrive: one the capture agent sent before this
    // link is stale
    let mut screen = shared.screen.subscribe();
    let mut requests = {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        shared.lock().requests = Some(sender);
        receiver
    };
    let _ = shared.events.send((engine, Event::Linked));
    let mut last_seen = Instant::now();
    let mut next_id: u64 = 1;
    let mut pending = Pending::new();
    config.mark_changed();
    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(message) = message else {
                    log::info!("the effect engine closed the link");
                    return Event::Unlinked;
                };
                last_seen = Instant::now();
                receive(shared, &message, &mut pending);
            }
            changed = config.changed() => {
                if changed.is_err() {
                    return Event::Unlinked;
                }
                let params = config.borrow_and_update().clone();
                if let Some(mut params) = params {
                    let revision = shared.revision.fetch_add(1, Ordering::Relaxed) + 1;
                    params["revision"] = json!(revision);
                    let request = rpc(next_id, "config.apply", &params);
                    next_id += 1;
                    if let Some(ended) = send(socket, Message::Binary(request)).await {
                        return ended;
                    }
                    log::debug!("sent configuration {revision} to the effect engine");
                }
            }
            changed = screen.changed() => {
                if changed.is_err() {
                    return Event::Unlinked;
                }
                // Forwarded as it came (ADR-0037 D1); the engine checks it
                let frame = screen.borrow_and_update().clone();
                if let Some(frame) = frame
                    && let Some(ended) = send(socket, Message::Binary(frame.to_vec())).await
                {
                    return ended;
                }
            }
            request = requests.recv() => {
                let message = match request {
                    Some(Request::RestartEngine) => {
                        rpc(next_id, "restart", &json!({"component": "engine"}))
                    }
                    // Its caller gave up (504): the engine mustn't act on it late
                    Some(Request::Call { reply, .. }) if reply.is_closed() => continue,
                    Some(Request::Call { method, params, reply }) => {
                        pending.retain(|_, waiting| !waiting.is_closed());
                        pending.insert(next_id, reply);
                        rpc(next_id, &method, &params)
                    }
                    None => continue,
                };
                next_id += 1;
                if let Some(ended) = send(socket, Message::Binary(message)).await {
                    return ended;
                }
            }
            _ = generation.changed() => {
                // The supervisor is done with this engine.
                return Event::Unlinked;
            }
            () = sleep_until(last_seen + HEARTBEAT_TIMEOUT) => {
                log::error!(
                    "the effect engine sent nothing for {} s",
                    HEARTBEAT_TIMEOUT.as_secs()
                );
                return Event::Hung;
            }
        }
    }
}

/// Sends a message in the session: `None` once sent, or why the session
/// ends. An engine that stops reading counts as hung, as one that stops
/// sending does.
async fn send<S: LinkSocket>(socket: &mut S, message: Message) -> Option<Event> {
    match timeout(HEARTBEAT_TIMEOUT, socket.send(message)).await {
        Ok(true) => None,
        Ok(false) => Some(Event::Unlinked),
        Err(_) => {
            log::error!(
                "the effect engine read nothing for {} s",
                HEARTBEAT_TIMEOUT.as_secs()
            );
            Some(Event::Hung)
        }
    }
}

pub(crate) fn rpc(id: u64, method: &str, params: &Value) -> Vec<u8> {
    let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let mut out = vec![TYPE_RPC];
    out.extend_from_slice(body.to_string().as_bytes());
    out
}

/// Keeps what the engine sent (notifications by method, `levels` by
/// surface, frames by index) and hands answers to the requests waiting for them.
fn receive(shared: &Shared, message: &Message, pending: &mut Pending) {
    let Message::Binary(bytes) = message else {
        log::debug!("ignored a text message after the handshake");
        return;
    };
    match bytes.first().copied() {
        Some(TYPE_RPC) => match serde_json::from_slice::<Value>(&bytes[1..]) {
            Ok(rpc) => receive_rpc(shared, rpc, pending),
            Err(_) => log::warn!("ignored a JSON-RPC message that isn't JSON"),
        },
        Some(kind @ (TYPE_SURFACE | TYPE_SIGNAL)) => {
            let index_at = if kind == TYPE_SURFACE { 9 } else { 7 };
            if let Some(index) = bytes.get(index_at..index_at + 2) {
                let index = u16::from_le_bytes([index[0], index[1]]);
                let mut state = shared.lock();
                let update = state.received.next();
                let value = Arc::new(bytes[1..].to_vec());
                state
                    .received
                    .frames
                    .insert((kind, index), Stamped { update, value });
            }
        }
        other => log::debug!("ignored a message of type {other:?}"),
    }
}

fn receive_rpc(shared: &Shared, mut rpc: Value, pending: &mut Pending) {
    if let Some(method) = rpc["method"].as_str() {
        if rpc.get("id").is_some() {
            log::debug!("ignored a request for {method} from the engine");
            return;
        }
        let method = method.to_owned();
        let params = rpc["params"].take();
        let mut state = shared.lock();
        if method == "levels" {
            let Some(surface) = params["surface"]
                .as_u64()
                .and_then(|s| u16::try_from(s).ok())
            else {
                log::debug!("ignored levels without a surface");
                return;
            };
            let update = state.received.next();
            let levels = Stamped {
                update,
                value: params,
            };
            state.received.levels.insert(surface, levels);
        } else {
            let update = state.received.next();
            let notification = Stamped {
                update,
                value: params,
            };
            state.received.notifications.insert(method, notification);
        }
        return;
    }
    let waiting = rpc["id"].as_u64().and_then(|id| pending.remove(&id));
    if let Some(error) = rpc.get("error") {
        let error = RpcError {
            code: error["code"].as_i64().unwrap_or(0),
            message: error["message"].as_str().unwrap_or_default().to_owned(),
        };
        match waiting {
            Some(reply) => {
                let _ = reply.send(Err(error));
            }
            None => log::warn!(
                "the effect engine answered request {} with {} ({})",
                rpc["id"],
                error.message,
                error.code
            ),
        }
    } else if let Some(reply) = waiting {
        let _ = reply.send(Ok(rpc["result"].take()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Supervisor;
    use std::sync::Arc;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    /// One end of an in-memory link.
    struct Pipe {
        incoming: UnboundedReceiver<Message>,
        outgoing: UnboundedSender<Message>,
    }

    impl LinkSocket for Pipe {
        async fn recv(&mut self) -> Option<Message> {
            self.incoming.recv().await
        }

        fn send(&mut self, message: Message) -> impl Future<Output = bool> + Send {
            std::future::ready(self.outgoing.send(message).is_ok())
        }
    }

    fn pipe() -> (Pipe, Pipe) {
        let (a_out, b_in) = unbounded_channel();
        let (b_out, a_in) = unbounded_channel();
        (
            Pipe {
                incoming: a_in,
                outgoing: a_out,
            },
            Pipe {
                incoming: b_in,
                outgoing: b_out,
            },
        )
    }

    fn hello(token: &str) -> Message {
        Message::Text(
            json!({
                "version": ["1.3"],
                "node": {"id": "folo-source", "name": "folo-source", "version": "0", "platform": "test"},
                "purpose": {"kind": "local", "token": token},
            })
            .to_string(),
        )
    }

    fn welcome_of(message: Option<Message>) -> Value {
        match message {
            Some(Message::Text(text)) => serde_json::from_str(&text).unwrap(),
            other => panic!("no welcome: {other:?}"),
        }
    }

    fn rpc_of(message: Option<Message>) -> Value {
        match message {
            Some(Message::Binary(bytes)) => {
                assert_eq!(bytes[0], TYPE_RPC);
                serde_json::from_slice(&bytes[1..]).unwrap()
            }
            other => panic!("no JSON-RPC message: {other:?}"),
        }
    }

    fn status() -> Message {
        let body = json!({"jsonrpc": "2.0", "method": "status", "params": {"state": "running", "workers": {}, "inputs": {}, "errors": []}});
        let mut bytes = vec![TYPE_RPC];
        bytes.extend_from_slice(body.to_string().as_bytes());
        Message::Binary(bytes)
    }

    /// A supervisor's shared state expecting an engine with `token`.
    fn expecting(token: &str, config: Option<Value>) -> Arc<Shared> {
        let shared = Supervisor::shared_for_tests();
        shared.lock().token = Some(token.into());
        shared.config.send_replace(config);
        shared
    }

    #[tokio::test]
    async fn the_engine_gets_a_welcome_and_its_configuration() {
        let shared = expecting(
            "secret",
            Some(
                json!({"inputs": {}, "surfaces": [], "scenes": [], "publish": {"surfaces": [], "signals": []}}),
            ),
        );
        let (node, mut engine) = pipe();
        let server = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move { serve(&shared, node, true).await })
        };
        engine.outgoing.send(hello("secret")).unwrap();
        let welcome = welcome_of(engine.incoming.recv().await);
        assert_eq!(welcome, json!({"version": "1.3", "result": "ok"}));
        let apply = rpc_of(engine.incoming.recv().await);
        assert_eq!(apply["method"], "config.apply");
        assert_eq!(apply["params"]["revision"], 1);

        shared.config.send_replace(Some(json!({"inputs": {}, "surfaces": [], "scenes": [], "publish": {"surfaces": [], "signals": []}})));
        let again = rpc_of(engine.incoming.recv().await);
        assert_eq!(again["params"]["revision"], 2);

        engine.outgoing.send(status()).unwrap();
        tokio::task::yield_now().await;
        let request = shared.lock().requests.clone().unwrap();
        request.send(Request::RestartEngine).unwrap();
        let restart = rpc_of(engine.incoming.recv().await);
        assert_eq!(restart["method"], "restart");
        assert_eq!(restart["params"], json!({"component": "engine"}));
        assert_eq!(
            shared.lock().received.notifications["status"].value["state"],
            "running"
        );

        drop(engine);
        server.await.unwrap();
        assert!(!shared.lock().linked);
    }

    fn from_engine(body: &Value) -> Message {
        let mut bytes = vec![TYPE_RPC];
        bytes.extend_from_slice(body.to_string().as_bytes());
        Message::Binary(bytes)
    }

    #[tokio::test]
    async fn requests_get_their_answers_and_levels_are_kept_per_surface() {
        let shared = expecting("secret", None);
        let (node, mut engine) = pipe();
        let server = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move { serve(&shared, node, true).await })
        };
        engine.outgoing.send(hello("secret")).unwrap();
        welcome_of(engine.incoming.recv().await);
        tokio::task::yield_now().await;
        let requests = shared.lock().requests.clone().unwrap();

        let (reply, answer) = oneshot::channel();
        let params = json!({"art_id": 3});
        let method = "media.art".to_owned();
        requests
            .send(Request::Call {
                method,
                params,
                reply,
            })
            .unwrap();
        let asked = rpc_of(engine.incoming.recv().await);
        assert_eq!(asked["method"], "media.art");
        assert_eq!(asked["params"], json!({"art_id": 3}));
        let result = json!({"content_type": "image/png", "data": "AA"});
        let answer_body = json!({"jsonrpc": "2.0", "id": asked["id"], "result": result});
        engine.outgoing.send(from_engine(&answer_body)).unwrap();
        assert_eq!(answer.await.unwrap(), Ok(result));

        let (reply, answer) = oneshot::channel();
        let params = json!({"action": "next"});
        let method = "media.control".to_owned();
        requests
            .send(Request::Call {
                method,
                params,
                reply,
            })
            .unwrap();
        let asked = rpc_of(engine.incoming.recv().await);
        let error = json!({"code": -32000, "message": "no player took it"});
        let refusal = json!({"jsonrpc": "2.0", "id": asked["id"], "error": error});
        engine.outgoing.send(from_engine(&refusal)).unwrap();
        let expected = RpcError {
            code: -32000,
            message: "no player took it".into(),
        };
        assert_eq!(answer.await.unwrap(), Err(expected));

        for surface in [0, 1, 0] {
            let layers = json!({"l1": [-30.0, 0.5, -33.0]});
            let params = json!({"surface": surface, "layers": layers});
            let levels = json!({"jsonrpc": "2.0", "method": "levels", "params": params});
            engine.outgoing.send(from_engine(&levels)).unwrap();
        }
        // A request still waiting when the link ends gets no answer
        let (reply, answer) = oneshot::channel();
        let method = "inputs.list".to_owned();
        let params = json!({});
        requests
            .send(Request::Call {
                method,
                params,
                reply,
            })
            .unwrap();
        rpc_of(engine.incoming.recv().await);
        {
            let state = shared.lock();
            let levels = &state.received.levels;
            assert_eq!(levels.len(), 2);
            assert!(
                levels[&0].update > levels[&1].update,
                "the newest of surface 0 is kept"
            );
            assert!(!state.received.notifications.contains_key("levels"));
        }
        drop(engine);
        server.await.unwrap();
        assert!(answer.await.is_err());
    }

    #[tokio::test]
    async fn screen_frames_go_to_the_engine_as_they_came_and_only_new_ones() {
        let shared = expecting("secret", None);
        // A frame from before the engine linked is stale
        shared.screen.publish(vec![0x04, 9, 9, 9]);
        let (node, mut engine) = pipe();
        let server = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move { serve(&shared, node, true).await })
        };
        engine.outgoing.send(hello("secret")).unwrap();
        welcome_of(engine.incoming.recv().await);
        tokio::task::yield_now().await;
        // Frames sent faster than the link takes them: the newest is the one that goes
        let frame = |marker: u8| {
            let mut bytes = vec![0x04];
            bytes.extend((0..=199u8).map(|i| marker ^ i));
            bytes
        };
        shared.screen.publish(frame(1));
        shared.screen.publish(frame(2));
        match engine.incoming.recv().await {
            Some(Message::Binary(bytes)) => assert_eq!(bytes, frame(2)),
            other => panic!("no frame: {other:?}"),
        }
        assert!(engine.incoming.try_recv().is_err(), "nothing else was sent");
        // An ended capture link drops what was waiting
        shared.screen.publish(frame(3));
        shared.screen.clear();
        tokio::task::yield_now().await;
        drop(engine);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn the_local_engine_links_from_loopback_only() {
        let shared = expecting("secret", None);
        let (node, mut engine) = pipe();
        engine.outgoing.send(hello("secret")).unwrap();
        serve(&shared, node, false).await;
        assert_eq!(
            welcome_of(engine.incoming.recv().await)["result"],
            "rejected"
        );
        assert!(!shared.lock().linked);
    }

    #[tokio::test]
    async fn a_wrong_token_is_rejected() {
        let shared = expecting("secret", None);
        let (node, mut engine) = pipe();
        engine.outgoing.send(hello("guess")).unwrap();
        serve(&shared, node, true).await;
        assert_eq!(
            welcome_of(engine.incoming.recv().await)["result"],
            "rejected"
        );
        assert!(engine.incoming.recv().await.is_none());
        assert!(!shared.lock().linked);
    }

    #[tokio::test]
    async fn without_an_expected_engine_every_hello_is_rejected() {
        let shared = Supervisor::shared_for_tests();
        let (node, mut engine) = pipe();
        engine.outgoing.send(hello("")).unwrap();
        serve(&shared, node, true).await;
        assert_eq!(
            welcome_of(engine.incoming.recv().await)["result"],
            "rejected"
        );
    }

    #[tokio::test]
    async fn a_second_engine_is_closed() {
        let shared = expecting("secret", None);
        shared.lock().linked = true;
        let (node, mut engine) = pipe();
        engine.outgoing.send(hello("secret")).unwrap();
        serve(&shared, node, true).await;
        assert!(
            engine.incoming.recv().await.is_none(),
            "closed without a welcome"
        );
    }

    #[tokio::test]
    async fn an_unknown_version_is_rejected() {
        let shared = expecting("secret", None);
        let (node, mut engine) = pipe();
        let hello = json!({"version": ["2.0"], "purpose": {"kind": "local", "token": "secret"}});
        engine
            .outgoing
            .send(Message::Text(hello.to_string()))
            .unwrap();
        serve(&shared, node, true).await;
        assert_eq!(
            welcome_of(engine.incoming.recv().await)["result"],
            "rejected"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn silence_counts_as_hung() {
        let shared = expecting("secret", None);
        let mut events = shared.take_events_for_tests();
        let (node, mut engine) = pipe();
        let server = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move { serve(&shared, node, true).await })
        };
        engine.outgoing.send(hello("secret")).unwrap();
        welcome_of(engine.incoming.recv().await);
        assert_eq!(events.recv().await, Some((0, Event::Linked)));
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(4)).await;
            engine.outgoing.send(status()).unwrap();
        }
        tokio::time::sleep(HEARTBEAT_TIMEOUT + Duration::from_millis(100)).await;
        server.await.unwrap();
        assert_eq!(events.recv().await, Some((0, Event::Hung)));
    }
}
