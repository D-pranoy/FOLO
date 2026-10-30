//! The node's status (docs/architecture/control-plane.md#status-and-health):
//! `NodeStatus` for `GET /api/v1/system/status` and `folo status`, which
//! asks the running node or, when none runs, reads the home's files; and
//! `folo reload`.

use std::fmt::Write as _;
use std::path::Path;

use folo_identity::{Identity, fingerprint};
use folo_settings::{Launch, Override};
use folo_store::Doc;
use folo_supervisor::capture::WORKER as CAPTURE_WORKER;
use folo_supervisor::{WORKER, WorkerStatus};
use serde_json::{Map, Value, json};

use crate::cli::{EXIT_FAILED, EXIT_NOT_HERE, EXIT_OK, Failure, LocationArgs, Place, locate};
use crate::node::Node;
use crate::{client, lock};

/// What a status is made of.
struct Parts<'a> {
    running: bool,
    id: &'a str,
    role: &'a str,
    place: &'a Place,
    listen: (String, u16),
    overrides: &'a [Override],
    providers: &'a Value,
    /// A provider instance's `ProviderStatus.state` while the node runs; `stopped` otherwise.
    provider_state: Option<&'a dyn Fn(&str) -> Value>,
    sources: &'a Value,
    worker: Option<WorkerStatus>,
    capture: Option<WorkerStatus>,
}

fn build(parts: &Parts<'_>) -> Value {
    let folders = &parts.place.folders;
    let overrides: Vec<Value> = parts
        .overrides
        .iter()
        .map(|o| {
            json!({"setting": o.setting, "source": o.source.name(), "name": o.name, "value": o.value})
        })
        .collect();
    let providers: Vec<Value> = parts.providers["providers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let id = p["id"].as_str().unwrap_or_default();
            let state = parts
                .provider_state
                .map_or(json!("stopped"), |state| state(id));
            json!({"id": p["id"], "kind": p["kind"], "enabled": p["enabled"], "state": state})
        })
        .collect();
    let paired = parts.sources["sources"]
        .as_array()
        .map_or(0, |s| s.iter().filter(|s| s["id"] != "local").count());
    let mut workers = Map::new();
    for (name, worker) in [(WORKER, parts.worker), (CAPTURE_WORKER, parts.capture)] {
        if let Some(worker) = worker {
            workers.insert(
                name.into(),
                json!({"state": worker.state.name(), "pid": worker.pid, "restarts": worker.restarts}),
            );
        }
    }
    json!({
        "running": parts.running,
        "id": parts.id,
        "version": env!("CARGO_PKG_VERSION"),
        "role": parts.role,
        "launch": parts.place.launch.name(),
        "mode": folders.mode.name(),
        "home": folders.home.display().to_string(),
        "data": folders.data.display().to_string(),
        "log": folders.log_file().display().to_string(),
        "listen": {"address": parts.listen.0, "port": parts.listen.1},
        "overrides": overrides,
        "pairing": {"owner": "", "sources": paired},
        "workers": workers,
        "providers": providers,
    })
}

/// The running node's status.
#[must_use]
pub fn of_running(node: &Node) -> Value {
    let settings = node.settings();
    let providers = node.store.get(Doc::Providers);
    let sources = node.store.get(Doc::Sources);
    let state = |id: &str| json!(node.output.provider_status(id).state.api());
    build(&Parts {
        running: true,
        id: &node.id,
        role: &node.role,
        place: &node.place,
        listen: (node.listen.ip().to_string(), node.listen.port()),
        overrides: &settings.overrides,
        providers: &providers,
        provider_state: Some(&state),
        sources: &sources,
        worker: Some(node.supervisor.status()),
        capture: Some(node.capture.status()),
    })
}

/// The status read from the home's files when no node runs. Reads only:
/// nothing in the home changes.
fn of_files(place: &Place) -> Value {
    let data = &place.folders.data;
    let read = |doc: Doc| {
        folo_store::read(data, doc).unwrap_or_else(|error| {
            eprintln!("{error}");
            json!({})
        })
    };
    let settings = &place.layered.settings;
    let address = if settings.remote_access.enabled {
        settings.listen.address.to_string()
    } else {
        "127.0.0.1".to_owned()
    };
    let port = u16::try_from(settings.listen.port.get()).unwrap_or(u16::MAX);
    build(&Parts {
        running: false,
        id: &stored_id(data),
        role: place.layered.json["role"].as_str().unwrap_or("controller"),
        place,
        listen: (address, port),
        overrides: &place.layered.overrides,
        providers: &read(Doc::Providers),
        provider_state: None,
        sources: &read(Doc::Sources),
        worker: None,
        capture: None,
    })
}

/// The node id from `secrets.json`, without changing the file; empty
/// before the node first ran.
fn stored_id(data: &Path) -> String {
    let Ok(bytes) = std::fs::read(data.join("secrets.json")) else {
        return String::new();
    };
    let Ok(secrets) = serde_json::from_slice::<Value>(&bytes) else {
        return String::new();
    };
    let identity = &secrets["identity"];
    Identity::from_stored(
        identity["private_key"].as_str().unwrap_or_default(),
        identity["public_key"].as_str().unwrap_or_default(),
    )
    .map(|i| i.node_id())
    .unwrap_or_default()
}

/// Settings for a command that talks to the node: `--port` overrides
/// the port as for a foreground launch.
fn place_for(location: &LocationArgs, port: Option<String>) -> Result<Place, Failure> {
    let flags: Vec<(&'static str, String)> = port.map(|p| ("listen.port", p)).into_iter().collect();
    let place = locate(location, &flags, Launch::Foreground)?;
    for warning in &place.layered.warnings {
        eprintln!("{warning}");
    }
    Ok(place)
}

fn port_of(place: &Place) -> u16 {
    u16::try_from(place.layered.settings.listen.port.get()).unwrap_or(u16::MAX)
}

/// `folo status [--json]`: 0 while a node runs, 1 when it doesn't answer,
/// 3 when none runs.
#[must_use]
pub fn command(location: &LocationArgs, port: Option<String>, as_json: bool) -> u8 {
    let place = match place_for(location, port) {
        Ok(place) => place,
        Err(failure) => {
            eprintln!("{}", failure.message);
            return failure.status;
        }
    };
    let (status, exit) = if lock::holder(&place.folders.lock_file()).is_none() {
        (of_files(&place), EXIT_NOT_HERE)
    } else {
        match client::request(port_of(&place), "GET", "/api/v1/system/status", None) {
            Ok((200, body)) => match serde_json::from_str::<Value>(&body) {
                Ok(status) => (status, EXIT_OK),
                Err(error) => {
                    eprintln!("the node's status isn't JSON: {error}");
                    return EXIT_FAILED;
                }
            },
            Ok((code, body)) => {
                eprintln!("the node answered {code}: {body}");
                return EXIT_FAILED;
            }
            Err(error) => {
                eprintln!(
                    "a node holds {} but doesn't answer on port {}: {error}",
                    place.folders.home.display(),
                    port_of(&place)
                );
                return EXIT_FAILED;
            }
        }
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_default()
        );
    } else {
        print!("{}", human(&status));
    }
    exit
}

/// `folo status` for people.
fn human(status: &Value) -> String {
    let text = |key: &str| status[key].as_str().unwrap_or_default().to_owned();
    let id = text("id");
    let mut out = format!(
        "FOLO node {} ({}): {}\n",
        text("version"),
        text("role"),
        if status["running"] == true {
            "running"
        } else {
            "not running"
        }
    );
    let line = |out: &mut String, label: &str, value: String| {
        let _ = writeln!(out, "  {label:<10}{value}");
    };
    line(
        &mut out,
        "id",
        if id.is_empty() {
            "(made on the first run)".into()
        } else {
            fingerprint(&id)
        },
    );
    line(&mut out, "home", text("home"));
    line(&mut out, "data", text("data"));
    line(&mut out, "log", text("log"));
    line(
        &mut out,
        "listen",
        format!(
            "{}:{}",
            status["listen"]["address"].as_str().unwrap_or_default(),
            status["listen"]["port"]
        ),
    );
    line(
        &mut out,
        "launch",
        format!("{}, {} folders", text("launch"), text("mode")),
    );
    for (name, label) in [(WORKER, "engine"), (CAPTURE_WORKER, "capture")] {
        if let Some(worker) = status["workers"].get(name) {
            line(
                &mut out,
                label,
                format!(
                    "{} (process {}, {} restarts)",
                    worker["state"].as_str().unwrap_or_default(),
                    worker["pid"],
                    worker["restarts"]
                ),
            );
        }
    }
    for o in status["overrides"].as_array().into_iter().flatten() {
        line(
            &mut out,
            "override",
            format!(
                "{} = {} ({})",
                o["setting"].as_str().unwrap_or_default(),
                o["value"],
                o["name"].as_str().unwrap_or_default()
            ),
        );
    }
    out
}

/// `folo reload`: 0 when reloaded, 1 on a failure, 3 when no node runs.
#[must_use]
pub fn reload_command(location: &LocationArgs, port: Option<String>) -> u8 {
    let place = match place_for(location, port) {
        Ok(place) => place,
        Err(failure) => {
            eprintln!("{}", failure.message);
            return failure.status;
        }
    };
    if lock::holder(&place.folders.lock_file()).is_none() {
        eprintln!("no node runs on {}", place.folders.home.display());
        return EXIT_NOT_HERE;
    }
    match client::request(port_of(&place), "POST", "/api/v1/system/reload", Some("{}")) {
        Ok((200, body)) => {
            let result: Value = serde_json::from_str(&body).unwrap_or_default();
            let list = |key: &str| -> String {
                let items: Vec<&str> = result[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if items.is_empty() {
                    "nothing".into()
                } else {
                    items.join(", ")
                }
            };
            println!("applied: {}", list("applied"));
            println!("after a restart: {}", list("restart_required"));
            EXIT_OK
        }
        Ok((code, body)) => {
            eprintln!("the node answered {code}: {body}");
            EXIT_FAILED
        }
        Err(error) => {
            eprintln!(
                "the node doesn't answer on port {}: {error}",
                port_of(&place)
            );
            EXIT_FAILED
        }
    }
}
