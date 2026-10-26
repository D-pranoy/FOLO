//! The agent's end of the capture link (ADR-0037 D1 to D3,
//! docs/specs/source-link.md#the-capture-link): dials the node with the
//! launch token, answers its requests, sends `capture.status` at least once a
//! second and the frames the worker makes, newest wins, and reconnects with
//! backoff until the node has been gone for 30 s.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::{Instant, interval, sleep, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::cli::{EXIT_FAILED, EXIT_OK};
use crate::environment::Environment;
use crate::logs::{Level, log};
use crate::worker::{self, Command, Failure, WorkerHandle};

/// The capture link's subprotocol, version and limits (ADR-0037 D1, D2).
pub const SUBPROTOCOL: &str = "folo.capture.v1";
pub const VERSION: &str = "1";
pub const MAX_MESSAGE_BYTES: usize = 400_000;
const TYPE_RPC: u8 = 0x01;

/// The times of the link, as ADR-0037 D2 sets them; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// The first reconnect delay, doubling up to `backoff_max`.
    pub backoff_first: Duration,
    pub backoff_max: Duration,
    /// How much of each delay is cut at random, 0 to 1.
    pub jitter: f64,
    /// The agent exits after this long without its node.
    pub give_up: Duration,
    /// How long the node has to answer the hello.
    pub handshake: Duration,
    /// `capture.status` goes out at least this often.
    pub status_every: Duration,
    /// A node that reads nothing for this long counts as lost.
    pub send: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            backoff_first: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            jitter: 0.25,
            give_up: Duration::from_secs(30),
            handshake: Duration::from_secs(5),
            status_every: Duration::from_secs(1),
            send: Duration::from_secs(5),
        }
    }
}

/// How one connection ended.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    /// A restart the node asked for.
    Restarted,
    /// The node answered the hello with anything but `ok`.
    Refused,
    /// The node could not be reached, or the link broke.
    Lost,
}

/// Runs the link until the node stops the agent, refuses it, or has been
/// gone too long; returns the agent's exit status.
pub async fn run(environment: &Environment, handle: &WorkerHandle, timing: Timing) -> u8 {
    let mut gone_since: Option<Instant> = None;
    let mut attempt = 0u32;
    loop {
        let began = Instant::now();
        let ended = connect_and_serve(environment, handle, &timing, &mut gone_since).await;
        stop_capture(handle).await;
        if gone_since.is_some_and(|since| since > began) {
            // A link was made: the next loss starts the backoff over
            attempt = 0;
        }
        match ended {
            Ended::Restarted => return EXIT_OK,
            Ended::Refused => {
                log(Level::Error, "the node refused the capture agent");
                return EXIT_FAILED;
            }
            Ended::Lost => {}
        }
        // From the loss of a link, or else from the first attempt that failed
        let since = *gone_since.get_or_insert(began);
        let left = timing.give_up.saturating_sub(since.elapsed());
        if left.is_zero() {
            log(
                Level::Error,
                &format!(
                    "the node has been gone for {:.0} s; exiting",
                    timing.give_up.as_secs_f64()
                ),
            );
            return EXIT_FAILED;
        }
        let delay = delay(&timing, attempt).min(left);
        attempt = attempt.saturating_add(1);
        log(
            Level::Info,
            &format!("reconnecting in {:.1} s", delay.as_secs_f64()),
        );
        sleep(delay).await;
    }
}

/// The delay before reconnect number `attempt` (0 first): the first delay
/// doubling up to the longest, cut by up to `jitter` at random.
fn delay(timing: &Timing, attempt: u32) -> Duration {
    let full = timing
        .backoff_first
        .saturating_mul(1u32 << attempt.min(16))
        .min(timing.backoff_max);
    let mut byte = [0u8; 1];
    let random = if getrandom::fill(&mut byte).is_ok() {
        f64::from(byte[0]) / 255.0
    } else {
        0.0
    };
    full.mul_f64(1.0 - timing.jitter.clamp(0.0, 1.0) * random)
}

/// Without a node, nothing is captured (ADR-0037 D3).
async fn stop_capture(handle: &WorkerHandle) {
    let (done, stopped) = oneshot::channel();
    if handle.commands.send(Command::Stop { done }).is_ok() {
        let _ = stopped.await;
    }
}

async fn connect_and_serve(
    environment: &Environment,
    handle: &WorkerHandle,
    timing: &Timing,
    gone_since: &mut Option<Instant>,
) -> Ended {
    let Ok(mut request) = environment.link.as_str().into_client_request() else {
        log(Level::Error, "FOLO_CAPTURE_LINK isn't a WebSocket URL");
        return Ended::Refused;
    };
    // The token goes to this machine's node only
    let loopback = request.uri().host().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !loopback {
        log(Level::Error, "FOLO_CAPTURE_LINK isn't a loopback address");
        return Ended::Refused;
    }
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        SUBPROTOCOL.parse().expect("a header value"),
    );
    let config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE_BYTES));
    let connected = timeout(
        timing.handshake,
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await;
    let mut socket = match connected {
        Ok(Ok((socket, _))) => socket,
        Ok(Err(error)) => {
            log(
                Level::Warning,
                &format!("the node can't be reached: {error}"),
            );
            return Ended::Lost;
        }
        Err(_) => {
            log(
                Level::Warning,
                "the node didn't accept the connection in time",
            );
            return Ended::Lost;
        }
    };
    match handshake(&mut socket, environment, timing).await {
        Handshake::Accepted => {}
        Handshake::Refused => return Ended::Refused,
        Handshake::Lost => return Ended::Lost,
    }
    // Linked: the node's absence counts from here
    *gone_since = None;
    log(Level::Info, "linked to the node");
    let ended = serve(&mut socket, handle, timing).await;
    // The node is gone from here, not from when the agent noticed it
    *gone_since = Some(Instant::now());
    ended
}

enum Handshake {
    Accepted,
    Refused,
    Lost,
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn handshake(socket: &mut Socket, environment: &Environment, timing: &Timing) -> Handshake {
    let hello = json!({
        "version": [VERSION],
        "agent": {"version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS},
        "token": environment.token,
    });
    if socket.send(Message::text(hello.to_string())).await.is_err() {
        return Handshake::Lost;
    }
    let Ok(Some(Ok(Message::Text(first)))) = timeout(timing.handshake, socket.next()).await else {
        return Handshake::Lost;
    };
    let welcome: Value = serde_json::from_str(first.as_str()).unwrap_or(Value::Null);
    if welcome["result"] == "ok" {
        Handshake::Accepted
    } else {
        Handshake::Refused
    }
}

/// The linked session: requests in, status and frames out.
async fn serve(socket: &mut Socket, handle: &WorkerHandle, timing: &Timing) -> Ended {
    let mut frames = handle.frames.clone();
    let mut status = handle.status.clone();
    let (answers, mut answered) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let mut tick = interval(timing.status_every);
    // Linked agents say what they are at once
    status.mark_changed();
    loop {
        let outgoing: Vec<u8>;
        tokio::select! {
            message = socket.next() => {
                let Some(Ok(message)) = message else {
                    log(Level::Info, "the node closed the link");
                    return Ended::Lost;
                };
                match message {
                    Message::Binary(bytes) => match receive(&bytes, handle, &answers).await {
                        Received::Nothing => {}
                        Received::Answer(answer) => {
                            if !send(socket, Message::binary(answer), timing).await {
                                return Ended::Lost;
                            }
                        }
                        Received::Restart(answer) => {
                            let _ = timeout(timing.send, socket.send(Message::binary(answer))).await;
                            let _ = socket.close(None).await;
                            return Ended::Restarted;
                        }
                    },
                    Message::Close(_) => return Ended::Lost,
                    _ => {}
                }
                continue;
            }
            Some(answer) = answered.recv() => outgoing = answer,
            _ = tick.tick() => outgoing = status_message(&status.borrow()),
            changed = status.changed() => {
                if changed.is_err() {
                    return Ended::Lost;
                }
                outgoing = status_message(&status.borrow_and_update());
            }
            changed = frames.changed() => {
                if changed.is_err() {
                    return Ended::Lost;
                }
                // Newest wins: a frame replaced before this point is gone
                let Some(frame) = frames.borrow_and_update().clone() else { continue };
                if !send(socket, Message::Binary(frame), timing).await {
                    return Ended::Lost;
                }
                continue;
            }
        }
        if !send(socket, Message::binary(outgoing), timing).await {
            return Ended::Lost;
        }
    }
}

async fn send(socket: &mut Socket, message: Message, timing: &Timing) -> bool {
    matches!(timeout(timing.send, socket.send(message)).await, Ok(Ok(())))
}

fn status_message(status: &folo_capture_core::types::CaptureStatus) -> Vec<u8> {
    let body = json!({
        "jsonrpc": "2.0",
        "method": "capture.status",
        "params": status,
    });
    message_of(&body)
}

fn message_of(body: &Value) -> Vec<u8> {
    let mut out = vec![TYPE_RPC];
    out.extend_from_slice(body.to_string().as_bytes());
    out
}

enum Received {
    Nothing,
    /// An answer to send at once.
    Answer(Vec<u8>),
    /// The answer to `restart {component: capture}`, after which the agent ends.
    Restart(Vec<u8>),
}

/// Handles a message of the node. Requests go to the worker; the answer
/// comes back through `answers` when it is ready. Anything that isn't a
/// JSON-RPC request is ignored.
async fn receive(
    bytes: &Bytes,
    handle: &WorkerHandle,
    answers: &tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) -> Received {
    if bytes.first() != Some(&TYPE_RPC) {
        return Received::Nothing;
    }
    let Ok(request) = serde_json::from_slice::<Value>(&bytes[1..]) else {
        log(Level::Warning, "ignored a JSON-RPC message that isn't JSON");
        return Received::Nothing;
    };
    let (Some(method), Some(id)) = (request["method"].as_str(), request.get("id")) else {
        return Received::Nothing;
    };
    let (id, params) = (id.clone(), request["params"].clone());
    if method == "restart" {
        return restart(handle, id, &params).await;
    }
    let (reply, result) = oneshot::channel();
    let call = Command::Call {
        method: method.to_owned(),
        params,
        reply,
    };
    if handle.commands.send(call).is_err() {
        return Received::Nothing;
    }
    let answers = answers.clone();
    tokio::spawn(async move {
        if let Ok(result) = result.await {
            let _ = answers.send(answer_message(&id, &result));
        }
    });
    Received::Nothing
}

/// `restart {component: capture}`: stops capturing, answers `{}`, and ends
/// the agent for the node to start it again; other components get `-32602`.
async fn restart(handle: &WorkerHandle, id: Value, params: &Value) -> Received {
    if params["component"] != "capture" {
        let failure = Failure {
            code: worker::INVALID_PARAMS,
            message: "restart takes only the component capture".into(),
        };
        // A refused restart leaves the agent as it is
        return Received::Answer(answer_message(&id, &Err(failure)));
    }
    stop_capture(handle).await;
    Received::Restart(answer_message(&id, &Ok(json!({}))))
}

fn answer_message(id: &Value, result: &Result<Value, Failure>) -> Vec<u8> {
    let body = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(Failure { code, message }) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    };
    message_of(&body)
}
