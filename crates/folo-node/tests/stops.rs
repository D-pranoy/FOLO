//! The `folo` binary run as a process (RB-06 steps 2 and 7): it stops
//! cleanly on its signals (docs/architecture/control-plane.md#supervision),
//! SIGTERM and SIGINT on Unix, Ctrl+C and Ctrl+Break on Windows, where the
//! node gets a console of its own and this test binary, run again as a
//! helper, attaches to it to send the event. The node logs `stopped`, exits
//! with 0 and frees its lock. A managed launch takes `node.toml` only.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use folo_node::client;
use serde_json::{Value, json};

const FOLO: &str = env!("CARGO_BIN_EXE_folo");
const START_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// Set for the helper: `<process id> <ctrl-c|break>`.
#[cfg(windows)]
const HELPER_VARIABLE: &str = "FOLO_TEST_CONSOLE_EVENT";

/// A fresh folder, removed afterwards.
struct Folder(PathBuf);

impl Folder {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-stop-test-{}", folo_store::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `folo <args>` in `dir`, without the caller's `FOLO_*` variables but
/// with `env`. On Windows it gets a console of its own that has no window,
/// in a process group of its own, which starts it with Ctrl+C turned off as
/// some parents do: the node must take it back.
fn start(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Child {
    let mut command = Command::new(FOLO);
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("FOLO_") {
            command.env_remove(name);
        }
    }
    command.envs(env.iter().copied());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().unwrap()
}

/// The node's status, once it answers on `port`.
fn wait_until_serving(node: &mut Child, port: u16) -> Value {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        assert!(node.try_wait().unwrap().is_none(), "the node ended early");
        if let Ok((200, body)) = client::request(port, "GET", "/api/v1/system/status", None) {
            return serde_json::from_str(&body).unwrap();
        }
        assert!(Instant::now() < deadline, "the node didn't start");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_exit(node: &mut Child) -> i32 {
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        if let Some(status) = node.try_wait().unwrap() {
            return status.code().unwrap_or(-1);
        }
        if Instant::now() > deadline {
            let _ = node.kill();
            panic!("the node didn't stop");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Sends `stop` to a running node and checks it stopped cleanly: status 0,
/// `stopped` last in its log, and its home's lock free.
fn stops_cleanly(node: &mut Child, status: &Value, stop: &str) {
    send(node.id(), stop);
    assert_eq!(wait_for_exit(node), 0, "{stop}");
    let log = std::fs::read_to_string(status["log"].as_str().unwrap()).unwrap();
    let messages: Vec<String> = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON lines"))
        .map(|record| record["msg"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        messages.iter().any(|m| m == "stopping"),
        "{stop}: {messages:?}"
    );
    assert_eq!(
        messages.last().map(String::as_str),
        Some("stopped"),
        "{stop}"
    );
    let home = Path::new(status["home"].as_str().unwrap());
    assert_eq!(folo_node::lock::holder(&home.join("folo.lock")), None);
}

/// Starts a node on a home of its own, and stops it with `stop`.
fn stops_cleanly_on(stop: &str) {
    let home = Folder::new();
    let port = free_port();
    let home_arg = home.0.display().to_string();
    let port_arg = port.to_string();
    let args = [
        "--home",
        &home_arg,
        "--port",
        &port_arg,
        "--local-source",
        "off",
    ];
    let mut node = start(&home.0, &args, &[]);
    let status = wait_until_serving(&mut node, port);
    stops_cleanly(&mut node, &status, stop);
}

/// A managed launch takes the user's folders and their `node.toml`, and
/// ignores every `FOLO_*` variable it inherits (ADR-0034 D1 and D2). The
/// user's folders are moved into a folder of the test's own.
#[test]
fn a_managed_launch_takes_node_toml_only() {
    let user = Folder::new();
    let port = free_port();
    let base = user.0.display().to_string();
    let (home, mut env) = if cfg!(windows) {
        (user.0.join("FOLO"), vec![("LOCALAPPDATA", base.as_str())])
    } else {
        let xdg = ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"];
        let mut env = vec![("HOME", base.as_str())];
        env.extend(xdg.map(|name| (name, ""))); // empty: the XDG defaults
        (user.0.join(".config").join("folo"), env)
    };
    std::fs::create_dir_all(&home).unwrap();
    let settings = format!("[listen]\nport = {port}\n[local_source]\nenabled = false\n");
    std::fs::write(home.join("node.toml"), settings).unwrap();
    let elsewhere = user.0.join("elsewhere").display().to_string();
    env.extend([
        ("FOLO_HOME", elsewhere.as_str()),
        ("FOLO_LISTEN_PORT", "1"),
        ("FOLO_LOCAL_SOURCE_ENABLED", "on"),
    ]);
    let mut node = start(&user.0, &["--managed"], &env);
    let status = wait_until_serving(&mut node, port);
    assert_eq!(status["launch"], "managed");
    assert_eq!(status["mode"], "user");
    assert_eq!(status["home"], home.display().to_string());
    assert_eq!(status["overrides"], json!([]));
    assert_eq!(status["workers"]["folo-source"]["state"], "stopped");
    assert!(!Path::new(&elsewhere).exists());
    stops_cleanly(
        &mut node,
        &status,
        if cfg!(windows) { "break" } else { "-TERM" },
    );
}

#[cfg(unix)]
fn send(pid: u32, stop: &str) {
    let status = Command::new("kill")
        .args([stop, &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn sigterm_stops_the_node() {
    stops_cleanly_on("-TERM");
}

#[cfg(unix)]
#[test]
fn sigint_stops_the_node() {
    stops_cleanly_on("-INT");
}

/// Runs this test binary again as the helper that sends `stop` to the
/// console of process `pid`.
#[cfg(windows)]
fn send(pid: u32, stop: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "console_helper",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_VARIABLE, format!("{pid} {stop}"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "the helper failed: {output:?}");
}

#[cfg(windows)]
#[test]
fn ctrl_c_stops_the_node() {
    stops_cleanly_on("ctrl-c");
}

#[cfg(windows)]
#[test]
fn ctrl_break_stops_the_node() {
    stops_cleanly_on("break");
}

/// The helper: does nothing in an ordinary test run.
#[test]
fn console_helper() {
    #[cfg(windows)]
    if let Ok(request) = std::env::var(HELPER_VARIABLE) {
        use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
        let (pid, stop) = request.split_once(' ').unwrap();
        let event = if stop == "ctrl-c" {
            CTRL_C_EVENT
        } else {
            CTRL_BREAK_EVENT
        };
        let code = match console_event(pid.parse().unwrap(), event) {
            Ok(()) => 0,
            Err(why) => {
                eprintln!("{why}");
                1
            }
        };
        // Long enough for the event to reach this process too, so the
        // handler is always what keeps it alive, not luck
        std::thread::sleep(Duration::from_millis(500));
        std::process::exit(code);
    }
}

/// The helper's console handler: it handles every event by ignoring it.
/// Without a handler only Ctrl+C can be ignored, and the Ctrl+Break the
/// helper sends would end it with `STATUS_CONTROL_C_EXIT` whenever the event
/// arrives before it exits.
#[cfg(windows)]
extern "system" fn ignore_event(_event: u32) -> windows_sys::core::BOOL {
    1
}

/// Attaches to the console of process `pid` and sends it `event`, which
/// reaches every process attached to it; this one ignores it.
#[cfg(windows)]
#[allow(unsafe_code)] // Win32 console calls, in a test only
fn console_event(pid: u32, event: u32) -> Result<(), String> {
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
    };
    let failed = |what: &str| format!("{what} failed: {}", std::io::Error::last_os_error());
    // SAFETY: plain calls, changing only this helper process's console;
    // the handler is a function that lives as long as the process.
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return Err(failed("AttachConsole"));
        }
        if SetConsoleCtrlHandler(Some(ignore_event), 1) == 0 {
            return Err(failed("SetConsoleCtrlHandler"));
        }
        if GenerateConsoleCtrlEvent(event, 0) == 0 {
            return Err(failed("GenerateConsoleCtrlEvent"));
        }
    }
    Ok(())
}
