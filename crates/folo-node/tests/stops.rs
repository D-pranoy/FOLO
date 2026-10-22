//! The `folo` binary stops cleanly on its signals (RB-06 step 7,
//! docs/architecture/control-plane.md#supervision): SIGTERM and SIGINT on
//! Unix; Ctrl+C and Ctrl+Break on Windows, where the node gets a console of
//! its own and this test binary, run again as a helper, attaches to it to
//! send the event. The node logs `stopped`, exits with 0 and frees its lock.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use folo_node::client;
use serde_json::Value;

const FOLO: &str = env!("CARGO_BIN_EXE_folo");
const START_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// Set for the helper: `<process id> <ctrl-c|break>`.
#[cfg(windows)]
const HELPER_VARIABLE: &str = "FOLO_TEST_CONSOLE_EVENT";

/// A fresh home, removed afterwards.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("folo-stop-test-{}", folo_store::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Home {
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

/// `folo --home <home> --port <port> --local-source off`, in the home,
/// without the caller's `FOLO_*` variables; on Windows with a console of
/// its own that has no window, and in a process group of its own, which
/// starts it with Ctrl+C turned off as some parents do: the node must take
/// it back.
fn start(home: &Path, port: u16) -> Child {
    let mut command = Command::new(FOLO);
    command
        .arg("--home")
        .arg(home)
        .args(["--port", &port.to_string(), "--local-source", "off"])
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("FOLO_") {
            command.env_remove(name);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().unwrap()
}

fn wait_until_serving(node: &mut Child, port: u16) {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        assert!(node.try_wait().unwrap().is_none(), "the node ended early");
        if matches!(
            client::request(port, "GET", "/api/v1/system/status", None),
            Ok((200, _))
        ) {
            return;
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

/// Starts a node, sends `stop` to it, and checks it stopped cleanly.
fn stops_cleanly_on(stop: &str) {
    let home = Home::new();
    let port = free_port();
    let mut node = start(&home.0, port);
    wait_until_serving(&mut node, port);
    send(node.id(), stop);
    assert_eq!(wait_for_exit(&mut node), 0, "{stop}");
    let log = std::fs::read_to_string(home.0.join("logs").join("folo.log")).unwrap();
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
    assert_eq!(folo_node::lock::holder(&home.0.join("folo.lock")), None);
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
        std::process::exit(code);
    }
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
    // SAFETY: plain calls without pointers, changing only this helper
    // process's console.
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return Err(failed("AttachConsole"));
        }
        if SetConsoleCtrlHandler(None, 1) == 0 {
            return Err(failed("SetConsoleCtrlHandler"));
        }
        if GenerateConsoleCtrlEvent(event, 0) == 0 {
            return Err(failed("GenerateConsoleCtrlEvent"));
        }
    }
    Ok(())
}
