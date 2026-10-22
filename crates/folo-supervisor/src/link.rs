//! The node's end of the local link (docs/specs/source-link.md#the-local-link,
//! ADR-0032): the handshake with the launch token, `config.apply`, the
//! heartbeat, and requests to the engine. The socket is abstract, so the
//! session runs over the node's HTTP server and over channels in tests.

use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Value, json};
use subtle::ConstantTimeEq as _;
use tokio::time::{Instant, sleep_until, timeout};

use crate::{Event, Shared};

/// The WebSocket subprotocol of the source link.
pub const SUBPROTOCOL: &str = "folo.source-link.v1";
/// The source-link versions the node speaks, newest first: this contract
/// and one minor version older (docs/specs/README.md#versioning).
pub const VERSIONS: [&str; 2] = ["1.2", "1.1"];
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// `restart {component: engine}`: the engine ends with status 0.
    RestartEngine,
}

/// Why a connection was turned away.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    /// Not a `local` hello with the current launch token, or no common version.
    Rejected(String),
    /// Another engine is linked already; the connection is closed at once.
    Busy,
}

/// Serves one connection to the link's endpoint until it ends.
pub(crate) async fn serve<S: LinkSocket>(shared: &Shared, mut socket: S) {
    let Ok(Some(Message::Text(first))) = timeout(HANDSHAKE_TIMEOUT, socket.recv()).await else {
        log::warn!("a connection to the source link sent no handshake");
        return;
    };
    let mut generation = shared.generation.subscribe();
    let engine = *generation.borrow_and_update();
    let accepted = check_hello(shared, &first);
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
fn check_hello(shared: &Shared, first: &str) -> Result<&'static str, Refusal> {
    let hello: Value = serde_json::from_str(first)
        .map_err(|_| Refusal::Rejected("the hello isn't JSON".into()))?;
    let purpose = &hello["purpose"];
    if purpose["kind"] != "local" {
        return Err(Refusal::Rejected("only the local purpose is served".into()));
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
    let mut requests = {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        shared.lock().requests = Some(sender);
        receiver
    };
    let _ = shared.events.send((engine, Event::Linked));
    let mut last_seen = Instant::now();
    let mut next_id: u64 = 1;
    config.mark_changed();
    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(message) = message else {
                    log::info!("the effect engine closed the link");
                    return Event::Unlinked;
                };
                last_seen = Instant::now();
                receive(shared, &message);
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
            request = requests.recv() => {
                let Some(Request::RestartEngine) = request else {
                    continue;
                };
                let message = rpc(next_id, "restart", &json!({"component": "engine"}));
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

fn rpc(id: u64, method: &str, params: &Value) -> Vec<u8> {
    let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let mut out = vec![TYPE_RPC];
    out.extend_from_slice(body.to_string().as_bytes());
    out
}

/// Keeps what the engine sent: notifications by method, frames by index.
fn receive(shared: &Shared, message: &Message) {
    let Message::Binary(bytes) = message else {
        log::debug!("ignored a text message after the handshake");
        return;
    };
    match bytes.first().copied() {
        Some(TYPE_RPC) => match serde_json::from_slice::<Value>(&bytes[1..]) {
            Ok(rpc) => receive_rpc(shared, &rpc),
            Err(_) => log::warn!("ignored a JSON-RPC message that isn't JSON"),
        },
        Some(kind @ (TYPE_SURFACE | TYPE_SIGNAL)) => {
            let index_at = if kind == TYPE_SURFACE { 9 } else { 7 };
            if let Some(index) = bytes.get(index_at..index_at + 2) {
                let index = u16::from_le_bytes([index[0], index[1]]);
                shared
                    .lock()
                    .frames
                    .insert((kind, index), bytes[1..].to_vec());
            }
        }
        other => log::debug!("ignored a message of type {other:?}"),
    }
}

fn receive_rpc(shared: &Shared, rpc: &Value) {
    if let Some(method) = rpc["method"].as_str() {
        if rpc.get("id").is_some() {
            log::debug!("ignored a request for {method} from the engine");
            return;
        }
        shared
            .lock()
            .notifications
            .insert(method.to_owned(), rpc["params"].clone());
    } else if let Some(error) = rpc.get("error") {
        log::warn!(
            "the effect engine answered request {} with {error}",
            rpc["id"]
        );
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
                "version": ["1.2"],
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
            tokio::spawn(async move { serve(&shared, node).await })
        };
        engine.outgoing.send(hello("secret")).unwrap();
        let welcome = welcome_of(engine.incoming.recv().await);
        assert_eq!(welcome, json!({"version": "1.2", "result": "ok"}));
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
        assert_eq!(shared.lock().notifications["status"]["state"], "running");

        drop(engine);
        server.await.unwrap();
        assert!(!shared.lock().linked);
    }

    #[tokio::test]
    async fn a_wrong_token_is_rejected() {
        let shared = expecting("secret", None);
        let (node, mut engine) = pipe();
        engine.outgoing.send(hello("guess")).unwrap();
        serve(&shared, node).await;
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
        serve(&shared, node).await;
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
        serve(&shared, node).await;
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
        serve(&shared, node).await;
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
            tokio::spawn(async move { serve(&shared, node).await })
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
