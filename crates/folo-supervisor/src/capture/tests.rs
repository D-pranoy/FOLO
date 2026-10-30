//! The capture link's node end over an in-memory socket: the handshake and
//! its refusals, `capture.start` and `capture.stop`, the restore token, and
//! screen frames passed on unchanged.

use super::*;
use crate::Supervisor;
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

    fn send(&mut self, message: Message) -> impl std::future::Future<Output = bool> + Send {
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

#[derive(Default)]
struct Sink {
    tokens: Mutex<Vec<String>>,
    lines: Mutex<Vec<String>>,
}

impl CaptureSink for Sink {
    fn agent_line(&self, line: &str) {
        self.lines.lock().unwrap().push(line.to_owned());
    }

    fn restore_token(&self, token: &str) {
        self.tokens.lock().unwrap().push(token.to_owned());
    }
}

fn hello(token: &str) -> Message {
    Message::Text(
        json!({
            "version": ["1"],
            "agent": {"version": "0", "platform": "test"},
            "token": token,
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

fn from_agent(body: &Value) -> Message {
    let mut bytes = vec![TYPE_RPC];
    bytes.extend_from_slice(body.to_string().as_bytes());
    Message::Binary(bytes)
}

fn status_of(state: &str, token: Option<&str>) -> Message {
    let mut params = json!({
        "state": state, "backend": "synthetic", "screen": "synthetic-0", "fps": 30,
        "hdr": false, "protected": false,
        "bars": {"top": 0.1, "bottom": 0.1, "left": 0, "right": 0},
    });
    if let Some(token) = token {
        params["restore_token"] = json!(token);
    }
    from_agent(&json!({"jsonrpc": "2.0", "method": "capture.status", "params": params}))
}

fn params() -> Value {
    json!({
        "screen": "synthetic-0", "width": 256, "height": 144, "rate_hz": 60,
        "black_bars": {"preset": "auto", "mode": "default", "threshold": 0.05},
        "profile": "video",
    })
}

struct Rig {
    supervisor: CaptureSupervisor,
    shared: Arc<Shared>,
    sink: Arc<Sink>,
    frames: ScreenFrames,
}

/// A supervisor that starts no process (the tests act as the agent), and
/// that expects an agent with `token`.
fn rig(token: Option<&str>, restore_token: Option<&str>) -> Rig {
    let engine = Supervisor::shared_for_tests();
    let frames = engine.screen.clone();
    let sink = Arc::new(Sink::default());
    let supervisor = CaptureSupervisor::without_process(
        frames.clone(),
        Arc::clone(&sink) as Arc<dyn CaptureSink>,
        restore_token.map(str::to_owned),
    );
    let shared = supervisor.shared_for_tests();
    shared.lock().token = token.map(str::to_owned);
    Rig {
        supervisor,
        shared,
        sink,
        frames,
    }
}

/// Serves a connection on its own task; the second end is the agent's.
fn connect(rig: &Rig) -> (Pipe, tokio::task::JoinHandle<()>) {
    let (node, agent) = pipe();
    let shared = Arc::clone(&rig.shared);
    (
        agent,
        tokio::spawn(async move { serve(&shared, node, true).await }),
    )
}

/// Lets the session task run until it waits.
async fn settle() {
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn the_agent_links_with_the_token_and_is_told_to_capture() {
    let rig = rig(Some("secret"), None);
    // Wanted before it links: the first message after the welcome is the start
    rig.shared
        .control
        .send_modify(|c| c.params = Some(params()));
    let (mut agent, server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    assert_eq!(
        welcome_of(agent.incoming.recv().await),
        json!({"version": "1", "result": "ok"})
    );
    let start = rpc_of(agent.incoming.recv().await);
    assert_eq!(start["method"], "capture.start");
    assert_eq!(start["params"], params());
    assert!(rig.supervisor.linked());
    drop(agent);
    server.await.unwrap();
    assert!(!rig.supervisor.linked());
}

#[tokio::test]
async fn changes_go_to_the_agent_and_stopping_stops_it() {
    let rig = rig(Some("secret"), None);
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    settle().await;
    // Not wanted yet: nothing is sent
    assert!(agent.incoming.try_recv().is_err());

    rig.supervisor.set_wanted(Some(params()));
    assert_eq!(
        rpc_of(agent.incoming.recv().await)["method"],
        "capture.start"
    );
    // The same wish again sends nothing; a changed one is sent again in place
    rig.supervisor.set_wanted(Some(params()));
    let mut faster = params();
    faster["rate_hz"] = json!(30);
    rig.supervisor.set_wanted(Some(faster.clone()));
    let again = rpc_of(agent.incoming.recv().await);
    assert_eq!(again["method"], "capture.start");
    assert_eq!(again["params"], faster);
    assert!(agent.incoming.try_recv().is_err());

    rig.supervisor.set_wanted(None);
    assert_eq!(
        rpc_of(agent.incoming.recv().await)["method"],
        "capture.stop"
    );
    // The supervisor's stop: no second capture.stop, then restart {capture}
    let requests = rig.shared.lock().requests.clone().unwrap();
    requests.send(Call::Restart).unwrap();
    let restart = rpc_of(agent.incoming.recv().await);
    assert_eq!(restart["method"], "restart");
    assert_eq!(restart["params"], json!({"component": "capture"}));
}

#[tokio::test]
async fn the_restore_token_goes_only_with_a_start_from_off_and_a_new_one_is_kept_aside() {
    let rig = rig(Some("secret"), Some("old-token"));
    rig.supervisor.set_wanted(Some(params()));
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    let start = rpc_of(agent.incoming.recv().await);
    assert_eq!(start["params"]["restore_token"], "old-token");

    agent
        .outgoing
        .send(status_of("capturing", Some("new-token")))
        .unwrap();
    settle().await;
    assert_eq!(*rig.sink.tokens.lock().unwrap(), ["new-token"]);
    // What clients see has no token anywhere
    let live = rig.supervisor.screen_live();
    assert!(
        !live.to_string().contains("token"),
        "restore token in {live}"
    );
    assert_eq!(live["state"], "capturing");
    assert_eq!(live["bars"]["top"], 0.1);

    // An in-place change doesn't send it again
    let mut faster = params();
    faster["rate_hz"] = json!(30);
    rig.supervisor.set_wanted(Some(faster));
    let again = rpc_of(agent.incoming.recv().await);
    assert!(again["params"].get("restore_token").is_none());
    // Starting from off again (after a stop) sends the new one
    rig.supervisor.set_wanted(None);
    rpc_of(agent.incoming.recv().await);
    rig.supervisor.set_wanted(Some(params()));
    let restarted = rpc_of(agent.incoming.recv().await);
    assert_eq!(restarted["params"]["restore_token"], "new-token");
}

#[tokio::test]
async fn screen_frames_are_passed_on_unchanged_and_the_newest_wins() {
    let rig = rig(Some("secret"), None);
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    let mut watcher = rig.frames.subscribe();
    let frame = |marker: u8| {
        let mut bytes = vec![TYPE_SCREEN];
        bytes.extend((0..64).map(|i| marker.wrapping_add(i)));
        bytes
    };
    agent.outgoing.send(Message::Binary(frame(1))).unwrap();
    agent.outgoing.send(Message::Binary(frame(2))).unwrap();
    settle().await;
    watcher.changed().await.unwrap();
    let got = watcher.borrow_and_update().clone().unwrap();
    assert_eq!(
        *got,
        frame(2),
        "the second replaced the first, byte for byte"
    );
    // A frame is not a status: nothing of it is in what clients see
    assert_eq!(rig.supervisor.screen_live()["state"], "off");
}

#[tokio::test]
async fn an_oversize_message_is_dropped_without_reading_it() {
    let rig = rig(Some("secret"), None);
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    let watcher = rig.frames.subscribe();
    let mut big = vec![TYPE_SCREEN];
    big.resize(MAX_MESSAGE_BYTES + 1, 0);
    agent.outgoing.send(Message::Binary(big)).unwrap();
    settle().await;
    assert!(!watcher.has_changed().unwrap());
}

#[tokio::test]
async fn a_refused_start_is_the_screens_error() {
    let rig = rig(Some("secret"), None);
    rig.supervisor.set_wanted(Some(params()));
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    let start = rpc_of(agent.incoming.recv().await);
    agent.outgoing.send(status_of("off", None)).unwrap();
    let error = json!({"code": -32000, "message": "no screen has the id \"x\""});
    let refusal = json!({"jsonrpc": "2.0", "id": start["id"], "error": error});
    agent.outgoing.send(from_agent(&refusal)).unwrap();
    settle().await;
    let live = rig.supervisor.screen_live();
    assert_eq!(live["state"], "error");
    assert_eq!(live["message"], "no screen has the id \"x\"");
}

#[tokio::test]
async fn requests_get_their_answers() {
    let rig = rig(Some("secret"), None);
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    let supervisor = rig.supervisor;
    let asking = tokio::spawn(async move { supervisor.screens().await });
    let asked = rpc_of(agent.incoming.recv().await);
    assert_eq!(asked["method"], "screens.list");
    let result =
        json!({"screens": [{"id": "a", "name": "A", "width": 1, "height": 1, "primary": true}]});
    agent
        .outgoing
        .send(from_agent(
            &json!({"jsonrpc": "2.0", "id": asked["id"], "result": result}),
        ))
        .unwrap();
    assert_eq!(asking.await.unwrap(), Ok(result));
}

async fn refused(rig: &Rig, first: Message, loopback: bool) -> Option<Value> {
    let (node, mut agent) = pipe();
    agent.outgoing.send(first).unwrap();
    serve(&rig.shared, node, loopback).await;
    match agent.incoming.recv().await {
        Some(Message::Text(text)) => Some(serde_json::from_str(&text).unwrap()),
        _ => None,
    }
}

#[tokio::test]
async fn a_wrong_token_is_rejected_before_the_slot_is_tested() {
    let rig = rig(Some("secret"), None);
    let rejected = json!({"version": "1", "result": "rejected"});
    assert_eq!(
        refused(&rig, hello("guess"), true).await,
        Some(rejected.clone())
    );
    assert!(!rig.supervisor.linked());
    // Even with an agent linked, a wrong token gets the rejection, not silence:
    // a rogue connection learns nothing about the slot
    rig.shared.lock().linked = true;
    assert_eq!(refused(&rig, hello("guess"), true).await, Some(rejected));
}

#[tokio::test]
async fn a_second_agent_with_the_token_is_closed_without_a_welcome() {
    let rig = rig(Some("secret"), None);
    rig.shared.lock().linked = true;
    assert_eq!(refused(&rig, hello("secret"), true).await, None);
}

#[tokio::test]
async fn other_hellos_are_rejected() {
    let rig = rig(Some("secret"), None);
    let rejected = json!({"version": "1", "result": "rejected"});
    // Not from loopback
    assert_eq!(
        refused(&rig, hello("secret"), false).await,
        Some(rejected.clone())
    );
    // Not JSON
    let bad = Message::Text("{".into());
    assert_eq!(refused(&rig, bad, true).await, Some(rejected.clone()));
    // A version it doesn't speak
    let old = json!({"version": ["2"], "agent": {}, "token": "secret"});
    let old = Message::Text(old.to_string());
    assert_eq!(refused(&rig, old, true).await, Some(rejected.clone()));
    // No agent expected
    rig.shared.lock().token = None;
    assert_eq!(refused(&rig, hello(""), true).await, Some(rejected));
    // A binary first message ends the connection without an answer
    assert_eq!(refused(&rig, Message::Binary(vec![1]), true).await, None);
    assert!(!rig.supervisor.linked());
}

#[tokio::test(start_paused = true)]
async fn a_late_handshake_ends_the_connection() {
    let rig = rig(Some("secret"), None);
    let (node, mut agent) = pipe();
    let shared = Arc::clone(&rig.shared);
    let server = tokio::spawn(async move { serve(&shared, node, true).await });
    tokio::time::sleep(HANDSHAKE_TIMEOUT + Duration::from_millis(100)).await;
    server.await.unwrap();
    assert!(agent.incoming.recv().await.is_none());
    assert!(!rig.supervisor.linked());
}

#[tokio::test(start_paused = true)]
async fn silence_counts_as_hung_and_any_message_is_a_sign_of_life() {
    let rig = rig(Some("secret"), None);
    let mut events = rig.shared.take_events();
    let (mut agent, server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    assert_eq!(events.recv().await, Some((0, Event::Linked)));
    // Frames count as messages, as statuses do
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(4)).await;
        agent
            .outgoing
            .send(Message::Binary(vec![TYPE_SCREEN, 0]))
            .unwrap();
    }
    tokio::time::sleep(HEARTBEAT_TIMEOUT + Duration::from_millis(100)).await;
    server.await.unwrap();
    assert_eq!(events.recv().await, Some((0, Event::Hung)));
}

#[tokio::test]
async fn what_clients_see_follows_wanted_and_the_agents_state() {
    let rig = rig(Some("secret"), None);
    assert_eq!(rig.supervisor.screen_live()["state"], "off");
    rig.supervisor.set_wanted(Some(params()));
    // Wanted but not linked yet
    assert_eq!(rig.supervisor.screen_live()["state"], "starting");
    rig.shared.lock().status.state = WorkerState::Failed;
    let live = rig.supervisor.screen_live();
    assert_eq!(live["state"], "unavailable");
    assert!(live["message"].as_str().is_some_and(|m| !m.is_empty()));
    rig.supervisor.set_wanted(None);
    assert_eq!(rig.supervisor.screen_live()["state"], "off");
    drop(rig);
}

#[tokio::test]
async fn without_a_session_to_run_in_capture_is_unavailable_and_no_screens_are_listed() {
    let engine = Supervisor::shared_for_tests();
    let supervisor = CaptureSupervisor::start(
        CaptureCommand {
            program: PathBuf::from("no such program"),
            run_args: Vec::new(),
            list_program: PathBuf::from("no such program"),
            list_args: Vec::new(),
            scope_launcher: None,
        },
        String::new(),
        Arc::new(Sink::default()),
        engine.screen.clone(),
        None,
        Some("the node runs as a service".into()),
    );
    assert_eq!(supervisor.screen_live()["state"], "off");
    supervisor.set_wanted(Some(params()));
    let live = supervisor.screen_live();
    assert_eq!(live["state"], "unavailable");
    assert_eq!(live["message"], "the node runs as a service");
    assert_eq!(supervisor.screens().await, Ok(json!({"screens": []})));
    // Nothing ever starts the agent
    assert_eq!(supervisor.status().state.name(), "stopped");
    supervisor.shutdown().await;
}

#[tokio::test]
async fn statuses_are_clamped_and_one_that_isnt_a_status_is_ignored() {
    let rig = rig(Some("secret"), None);
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    rig.supervisor.set_wanted(Some(params()));
    rpc_of(agent.incoming.recv().await);
    let junk = json!({"jsonrpc": "2.0", "method": "capture.status", "params": "x"});
    agent.outgoing.send(from_agent(&junk)).unwrap();
    settle().await;
    assert_eq!(rig.supervisor.screen_live()["state"], "starting");
    agent.outgoing.send(status_of("paused", None)).unwrap();
    settle().await;
    assert_eq!(rig.supervisor.screen_live()["state"], "paused");
}

#[tokio::test]
async fn a_repeated_token_is_kept_once_and_one_the_contract_does_not_take_not_at_all() {
    let rig = rig(Some("secret"), None);
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    for _ in 0..3 {
        agent
            .outgoing
            .send(status_of("capturing", Some("same-token")))
            .unwrap();
    }
    agent
        .outgoing
        .send(status_of("capturing", Some(&"x".repeat(257))))
        .unwrap();
    agent
        .outgoing
        .send(status_of("capturing", Some("bad\ntoken")))
        .unwrap();
    settle().await;
    assert_eq!(*rig.sink.tokens.lock().unwrap(), ["same-token"]);
    assert_eq!(rig.supervisor.screen_live()["state"], "off");
    rig.supervisor.set_wanted(Some(params()));
    assert_eq!(rig.supervisor.screen_live()["state"], "capturing");
}

#[tokio::test]
async fn a_long_reason_is_cut_to_what_the_contract_takes_and_the_agent_gets_no_answers_to_ask() {
    let rig = rig(Some("secret"), None);
    rig.supervisor.set_wanted(Some(params()));
    let (mut agent, _server) = connect(&rig);
    agent.outgoing.send(hello("secret")).unwrap();
    welcome_of(agent.incoming.recv().await);
    let start = rpc_of(agent.incoming.recv().await);
    agent.outgoing.send(status_of("off", None)).unwrap();
    let error = json!({"code": -32000, "message": "é".repeat(2000)});
    let refusal = json!({"jsonrpc": "2.0", "id": start["id"], "error": error});
    agent.outgoing.send(from_agent(&refusal)).unwrap();
    // A request from the agent is answered -32601
    let ask = json!({"jsonrpc": "2.0", "id": 77, "method": "screens.list", "params": {}});
    agent.outgoing.send(from_agent(&ask)).unwrap();
    let answer = rpc_of(agent.incoming.recv().await);
    assert_eq!(
        (answer["id"].clone(), answer["error"]["code"].clone()),
        (json!(77), json!(-32601))
    );
    let live = rig.supervisor.screen_live();
    assert_eq!(live["state"], "error");
    assert_eq!(live["message"].as_str().unwrap().chars().count(), 500);
}

// ------------------------------------------------- the Linux scope launch

#[cfg(target_os = "linux")]
mod scope {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    /// A folder of its own, removed afterwards.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("folo-scope-{name}-{}", launch_token()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A `systemd-run` that writes its arguments and the link's variables
    /// to files instead of running anything.
    fn stand_in(folder: &Folder) -> PathBuf {
        let script = folder.0.join("systemd-run");
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{dir}/argv'\n\
             printf '%s\\n%s\\n' \"$FOLO_CAPTURE_LINK\" \"$FOLO_CAPTURE_TOKEN\" > '{dir}/env'\n",
            dir = folder.0.display()
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn command(launcher: Option<PathBuf>, run_args: Vec<OsString>) -> CaptureCommand {
        CaptureCommand {
            program: PathBuf::from("/opt/folo/folo-capture"),
            run_args,
            list_program: PathBuf::from("/opt/folo/folo-capture"),
            list_args: Vec::new(),
            scope_launcher: launcher,
        }
    }

    #[tokio::test]
    async fn the_agent_starts_in_a_transient_scope_and_the_token_is_only_in_the_environment() {
        let folder = Folder::new("scope");
        let sink: Arc<dyn CaptureSink> = Arc::new(Sink::default());
        let command = command(Some(stand_in(&folder)), Vec::new());
        let mut child = spawn(&command, "ws://127.0.0.1:1/link", "the-token", &sink).unwrap();
        assert!(child.wait().await.unwrap().success());
        let recorded = std::fs::read_to_string(folder.0.join("argv")).unwrap();
        let args: Vec<&str> = recorded.lines().collect();
        assert_eq!(&args[..3], ["--user", "--scope", "--quiet"]);
        let unit = args[3].strip_prefix("--unit=").unwrap();
        let random = unit
            .strip_prefix(r"app-io.github.d\x2dpranoy.folo\x2dcapture-")
            .and_then(|rest| rest.strip_suffix(".scope"))
            .unwrap();
        assert!(!random.is_empty() && random.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(&args[4..], ["/opt/folo/folo-capture", "run"]);
        let env = std::fs::read_to_string(folder.0.join("env")).unwrap();
        assert_eq!(env, "ws://127.0.0.1:1/link\nthe-token\n");
        assert!(!recorded.contains("the-token") && !recorded.contains("127.0.0.1"));
    }

    #[test]
    fn the_agents_own_arguments_come_before_run_and_every_scope_is_named_anew() {
        let launcher = Some(PathBuf::from("systemd-run"));
        let command = command(launcher, vec!["--backend".into(), "portal".into()]);
        let (program, args) = launch_plan(&command);
        assert_eq!(program, Path::new("systemd-run"));
        assert_eq!(
            &args[4..],
            ["/opt/folo/folo-capture", "--backend", "portal", "run"]
        );
        assert_ne!(launch_plan(&command).1[3], args[3]);
    }

    #[tokio::test]
    async fn without_systemd_run_the_agent_starts_directly_with_the_environment_only() {
        let folder = Folder::new("direct");
        let script = folder.0.join("folo-capture");
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{dir}/argv'\n\
             printf '%s\\n%s\\n' \"$FOLO_CAPTURE_LINK\" \"$FOLO_CAPTURE_TOKEN\" > '{dir}/env'\n",
            dir = folder.0.display()
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sink: Arc<dyn CaptureSink> = Arc::new(Sink::default());
        let mut command = command(None, vec!["--backend".into(), "portal".into()]);
        command.program = script;
        let mut child = spawn(&command, "ws://127.0.0.1:1/link", "the-token", &sink).unwrap();
        assert!(child.wait().await.unwrap().success());
        assert_eq!(
            std::fs::read_to_string(folder.0.join("argv")).unwrap(),
            "--backend\nportal\n"
        );
        let env = std::fs::read_to_string(folder.0.join("env")).unwrap();
        assert_eq!(env, "ws://127.0.0.1:1/link\nthe-token\n");
    }
}
