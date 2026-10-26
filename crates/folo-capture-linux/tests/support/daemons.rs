//! The private desktop the tests run in: a session bus, a PipeWire daemon and
//! its session manager, all in a folder of their own. Nothing of the user's
//! audio or screen is touched.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Set by CI: a missing daemon is then a failure, not a skip.
const REQUIRED_VARIABLE: &str = "FOLO_PIPEWIRE_TESTS";
/// The longest a daemon may outlive its test, in seconds.
const DAEMON_LIFETIME: &str = "300";

static NEXT: AtomicU32 = AtomicU32::new(0);

pub struct Desktop {
    dir: PathBuf,
    children: Vec<Child>,
    pub bus_address: String,
    pub pipewire_socket: PathBuf,
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn wait_for(path: &Path, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{what} never came up");
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl Desktop {
    /// Starts the daemons, or returns `None` (a skip) when they aren't installed
    /// and `FOLO_PIPEWIRE_TESTS` isn't set.
    pub fn start() -> Option<Self> {
        let required = std::env::var(REQUIRED_VARIABLE).is_ok_and(|value| value == "1");
        let missing: Vec<_> = ["dbus-daemon", "pipewire", "wireplumber"]
            .into_iter()
            .filter(|program| !which(program))
            .collect();
        if !missing.is_empty() {
            assert!(
                !required,
                "{REQUIRED_VARIABLE}=1 but {missing:?} aren't installed"
            );
            eprintln!("skipped: {missing:?} aren't installed");
            return None;
        }
        let dir = std::env::temp_dir().join(format!(
            "folo-capture-linux-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        let bus_socket = dir.join("bus");
        let bus_address = format!("unix:path={}", bus_socket.display());
        let pipewire_socket = dir.join("pipewire-0");
        let mut desktop = Self {
            dir: dir.clone(),
            children: Vec::new(),
            bus_address: bus_address.clone(),
            pipewire_socket: pipewire_socket.clone(),
        };
        desktop.spawn(
            Command::new("dbus-daemon")
                .args(["--session", "--nofork"])
                .arg(format!("--address={bus_address}")),
        );
        wait_for(&bus_socket, "the private session bus");
        desktop.spawn(&mut Command::new("pipewire"));
        wait_for(&pipewire_socket, "PipeWire");
        desktop.spawn(&mut Command::new("wireplumber"));
        Some(desktop)
    }

    /// Runs the daemon under `timeout`, so one the test never got to stop (the
    /// test process aborted) ends by itself.
    fn spawn(&mut self, daemon: &mut Command) {
        let mut command = Command::new("timeout");
        command.arg(DAEMON_LIFETIME).arg(daemon.get_program());
        command.args(daemon.get_args());
        let child = command
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("PIPEWIRE_RUNTIME_DIR", &self.dir)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus_address)
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting a daemon");
        self.children.push(child);
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        for child in self.children.iter_mut().rev() {
            // `timeout` passes SIGTERM on to its daemon; killing it would not.
            let _ = Command::new("kill")
                .arg(child.id().to_string())
                .stderr(Stdio::null())
                .status();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
