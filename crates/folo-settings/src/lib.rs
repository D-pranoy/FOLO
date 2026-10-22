//! The node's settings (RB-06 step 2): `node.toml`, then for foreground
//! launches the `FOLO_*` variables and the flags, as
//! docs/architecture/control-plane.md#settings-layering and ADR-0034 D2, D3
//! and D7 describe. The folders the settings live in are in [`folders`].

pub mod folders;

use folo_protocol::clamp::{ClampError, clamp};
use folo_protocol::generated::NodeSettings;
use serde_json::{Map, Value};

/// The schema definition `node.toml` is clamped with.
const DEFINITION: &str = "NodeSettings";

/// How the node was started (ADR-0034 D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// By the desktop app, with `--managed`: defaults and `node.toml` only.
    Managed,
    /// Everything else: the CLI, services and Docker.
    Foreground,
}

impl Launch {
    /// The name `folo status` reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Foreground => "foreground",
        }
    }
}

/// Where an override came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Environment,
    Flag,
}

impl Source {
    /// The name `folo status` reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Flag => "flag",
        }
    }
}

/// A deployment override: a setting given by a variable or a flag.
#[derive(Debug, Clone, PartialEq)]
pub struct Override {
    /// The setting's path in `node.toml`, for example `listen.port`.
    pub setting: &'static str,
    pub source: Source,
    /// The variable or the flag.
    pub name: &'static str,
    /// The value it gives the setting (for a list, the entries it adds).
    pub value: Value,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Text,
    Integer,
    Boolean,
    Choice(&'static [&'static str]),
    List,
}

/// A setting of `node.toml` with its variable and flag.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub path: &'static str,
    pub variable: &'static str,
    pub flag: &'static str,
    kind: Kind,
}

/// Every setting that can be overridden (docs/architecture/control-plane.md#settings-layering).
pub const SETTINGS: [Setting; 8] = [
    Setting {
        path: "role",
        variable: "FOLO_ROLE",
        flag: "--role",
        kind: Kind::Choice(&["controller", "source"]),
    },
    Setting {
        path: "listen.address",
        variable: "FOLO_LISTEN_ADDRESS",
        flag: "--address",
        kind: Kind::Text,
    },
    Setting {
        path: "listen.port",
        variable: "FOLO_LISTEN_PORT",
        flag: "--port",
        kind: Kind::Integer,
    },
    Setting {
        path: "data_dir",
        variable: "FOLO_DATA_DIR",
        flag: "--data-dir",
        kind: Kind::Text,
    },
    Setting {
        path: "remote_access.enabled",
        variable: "FOLO_REMOTE_ACCESS_ENABLED",
        flag: "--remote-access",
        kind: Kind::Boolean,
    },
    Setting {
        path: "remote_access.allowed_hosts",
        variable: "FOLO_REMOTE_ACCESS_ALLOWED_HOSTS",
        flag: "--allowed-host",
        kind: Kind::List,
    },
    Setting {
        path: "log_level",
        variable: "FOLO_LOG_LEVEL",
        flag: "--log-level",
        kind: Kind::Choice(&["error", "warn", "info", "debug", "trace"]),
    },
    Setting {
        path: "local_source.enabled",
        variable: "FOLO_LOCAL_SOURCE_ENABLED",
        flag: "--local-source",
        kind: Kind::Boolean,
    },
];

/// Settings applied while the node runs; the others need a restart (ADR-0034 D7).
const HOT: [&str; 3] = [
    "log_level",
    "remote_access.allowed_hosts",
    "local_source.enabled",
];

/// The setting with that path.
///
/// # Panics
///
/// When no setting has that path, a programming error.
#[must_use]
pub fn setting(path: &str) -> &'static Setting {
    SETTINGS
        .iter()
        .find(|s| s.path == path)
        .unwrap_or_else(|| panic!("no setting {path}"))
}

/// Why the settings can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// `node.toml` isn't TOML: a structural error (ADR-0034 D11).
    File(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(why) => write!(f, "node.toml can't be read: {why}"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// The settings after layering.
#[derive(Debug, Clone)]
pub struct Layered {
    pub settings: NodeSettings,
    /// The same settings as JSON, clamped.
    pub json: Value,
    pub overrides: Vec<Override>,
    /// Overrides that couldn't be read, and were ignored.
    pub warnings: Vec<String>,
}

/// The node's settings: defaults, then `file` (the text of `node.toml`, if
/// there is one), then for a foreground launch the variables `env` gives,
/// then the `flags`, each a setting's path and the flag's value.
///
/// # Errors
///
/// [`SettingsError::File`] when `file` isn't TOML.
///
/// # Panics
///
/// Only if the clamped settings didn't fit the generated type, which the
/// clamper rules out.
pub fn layer(
    file: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    flags: &[(&'static str, String)],
    launch: Launch,
) -> Result<Layered, SettingsError> {
    let mut json = match file {
        Some(text) => from_toml(text)?,
        None => Value::Object(Map::new()),
    };
    json = clamped(&json);
    let mut overrides = Vec::new();
    let mut warnings = Vec::new();
    if launch == Launch::Foreground {
        for spec in &SETTINGS {
            let Some(raw) = env(spec.variable) else {
                continue;
            };
            match parse(spec.kind, &raw, true) {
                Ok(value) => {
                    apply(&mut json, spec, &value);
                    overrides.push(Override {
                        setting: spec.path,
                        source: Source::Environment,
                        name: spec.variable,
                        value,
                    });
                }
                Err(why) => warnings.push(format!("{}: {why}; ignored", spec.variable)),
            }
        }
        for (path, raw) in flags {
            let spec = setting(path);
            match parse(spec.kind, raw, false) {
                Ok(value) => {
                    apply(&mut json, spec, &value);
                    merge_override(&mut overrides, spec, value);
                }
                Err(why) => warnings.push(format!("{}: {why}; ignored", spec.flag)),
            }
        }
    }
    let json = clamped(&json);
    let settings =
        serde_json::from_value(json.clone()).expect("clamped settings fit the generated type");
    Ok(Layered {
        settings,
        json,
        overrides,
        warnings,
    })
}

/// Repeated list flags make one override.
fn merge_override(overrides: &mut Vec<Override>, spec: &Setting, value: Value) {
    if let (Kind::List, Some(existing)) = (
        spec.kind,
        overrides
            .iter_mut()
            .find(|o| o.setting == spec.path && o.source == Source::Flag),
    ) {
        if let (Value::Array(items), Value::Array(more)) = (&mut existing.value, value) {
            items.extend(more);
        }
        return;
    }
    overrides.push(Override {
        setting: spec.path,
        source: Source::Flag,
        name: spec.flag,
        value,
    });
}

fn from_toml(text: &str) -> Result<Value, SettingsError> {
    let table: toml::Table =
        toml::from_str(text).map_err(|e| SettingsError::File(e.to_string()))?;
    serde_json::to_value(table).map_err(|e| SettingsError::File(e.to_string()))
}

fn clamped(json: &Value) -> Value {
    let mut no_ids = || String::new();
    match clamp(DEFINITION, json, &mut no_ids) {
        Ok(value) => value,
        Err(ClampError::Structural(_) | ClampError::UnknownDefinition(_)) => {
            unreachable!("settings are always an object of a known definition")
        }
    }
}

/// An override's value, or why it can't be read. `variable` says whether it
/// came from a variable, where lists are comma-separated.
fn parse(kind: Kind, raw: &str, variable: bool) -> Result<Value, String> {
    let text = raw.trim();
    match kind {
        Kind::Text => Ok(Value::String(text.to_owned())),
        Kind::Integer => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("{raw:?} isn't a whole number")),
        Kind::Boolean => match text.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(Value::Bool(true)),
            "false" | "off" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!("{raw:?} isn't on or off")),
        },
        Kind::Choice(choices) => {
            let lower = text.to_ascii_lowercase();
            if choices.contains(&lower.as_str()) {
                Ok(Value::String(lower))
            } else {
                Err(format!("{raw:?} isn't one of {}", choices.join(", ")))
            }
        }
        Kind::List => {
            let items: Vec<Value> = if variable {
                text.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| Value::String(s.to_owned()))
                    .collect()
            } else if text.is_empty() {
                Vec::new()
            } else {
                vec![Value::String(text.to_owned())]
            };
            Ok(Value::Array(items))
        }
    }
}

/// Sets the setting at its path; a list's entries are added without duplicates.
fn apply(json: &mut Value, spec: &Setting, value: &Value) {
    let mut node = json;
    let mut parts = spec.path.split('.').peekable();
    while let Some(part) = parts.next() {
        let object = node.as_object_mut().expect("settings objects are objects");
        if parts.peek().is_none() {
            match (spec.kind, object.get_mut(part), value) {
                (Kind::List, Some(Value::Array(items)), Value::Array(more)) => {
                    for item in more {
                        if !items.contains(item) {
                            items.push(item.clone());
                        }
                    }
                }
                _ => {
                    object.insert(part.to_owned(), value.clone());
                }
            }
            return;
        }
        node = object
            .entry(part.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

fn at<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(json, |node, part| node.get(part))
}

/// What `folo reload` found changed between the running settings and new ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Settings applied while the node runs.
    pub applied: Vec<&'static str>,
    /// Settings that take effect after a restart.
    pub restart_required: Vec<&'static str>,
}

/// The settings that differ between `old` and `new` (both clamped JSON), split
/// into those applied while running and those needing a restart (ADR-0034 D7).
#[must_use]
pub fn changes(old: &Value, new: &Value) -> Changes {
    let mut out = Changes::default();
    for spec in &SETTINGS {
        if at(old, spec.path) != at(new, spec.path) {
            if HOT.contains(&spec.path) {
                out.applied.push(spec.path);
            } else {
                out.restart_required.push(spec.path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_without_a_file() {
        let layered = layer(None, &no_env, &[], Launch::Foreground).unwrap();
        assert_eq!(layered.json["role"], "controller");
        assert_eq!(layered.json["listen"]["address"], "0.0.0.0");
        assert_eq!(layered.json["listen"]["port"], 8777);
        assert_eq!(layered.json["local_source"]["enabled"], true);
        assert_eq!(layered.json["log_level"], "info");
        assert_eq!(layered.overrides, []);
    }

    #[test]
    fn the_file_is_clamped() {
        let file = "role = \"source\"\nlog_level = \"loud\"\nunknown = 1\n[listen]\nport = 70000\n";
        let layered = layer(Some(file), &no_env, &[], Launch::Foreground).unwrap();
        assert_eq!(layered.json["role"], "source");
        assert_eq!(layered.json["log_level"], "info");
        assert_eq!(layered.json["listen"]["port"], 65535);
        assert!(layered.json.get("unknown").is_none());
    }

    #[test]
    fn a_file_that_isnt_toml_is_refused() {
        let error = layer(Some("role = "), &no_env, &[], Launch::Foreground).unwrap_err();
        assert!(matches!(error, SettingsError::File(_)));
    }

    #[test]
    fn variables_then_flags_win() {
        let file = "[listen]\nport = 9000\n";
        let env = env_of(&[("FOLO_LISTEN_PORT", "9100"), ("FOLO_ROLE", "Source")]);
        let flags = [("listen.port", "9200".to_owned())];
        let layered = layer(Some(file), &env, &flags, Launch::Foreground).unwrap();
        assert_eq!(layered.json["listen"]["port"], 9200);
        assert_eq!(layered.json["role"], "source");
        let names: Vec<_> = layered.overrides.iter().map(|o| o.name).collect();
        assert_eq!(names, ["FOLO_ROLE", "FOLO_LISTEN_PORT", "--port"]);
        assert_eq!(layered.overrides[2].source, Source::Flag);
        assert_eq!(layered.overrides[1].value, Value::from(9100));
    }

    #[test]
    fn managed_launches_ignore_variables_and_flags() {
        let env = env_of(&[("FOLO_LISTEN_PORT", "9100")]);
        let flags = [("listen.port", "9200".to_owned())];
        let layered = layer(None, &env, &flags, Launch::Managed).unwrap();
        assert_eq!(layered.json["listen"]["port"], 8777);
        assert_eq!(layered.overrides, []);
    }

    #[test]
    fn unreadable_overrides_are_ignored_and_out_of_range_ones_clamped() {
        let file = "[listen]\nport = 9000\n";
        let env = env_of(&[
            ("FOLO_LISTEN_PORT", "nine"),
            ("FOLO_LOCAL_SOURCE_ENABLED", "maybe"),
            ("FOLO_LOG_LEVEL", "DEBUG"),
        ]);
        let flags = [("role", "server".to_owned())];
        let layered = layer(Some(file), &env, &flags, Launch::Foreground).unwrap();
        assert_eq!(layered.json["listen"]["port"], 9000);
        assert_eq!(layered.json["local_source"]["enabled"], true);
        assert_eq!(layered.json["log_level"], "debug");
        assert_eq!(layered.json["role"], "controller");
        assert_eq!(layered.warnings.len(), 3, "{:?}", layered.warnings);

        let env = env_of(&[("FOLO_LISTEN_PORT", "0")]);
        let layered = layer(None, &env, &[], Launch::Foreground).unwrap();
        assert_eq!(layered.json["listen"]["port"], 1);
    }

    #[test]
    fn booleans_take_several_spellings() {
        for (raw, expected) in [
            ("on", true),
            ("YES", true),
            ("1", true),
            ("Off", false),
            ("no", false),
        ] {
            let env = env_of(&[("FOLO_REMOTE_ACCESS_ENABLED", raw)]);
            let layered = layer(None, &env, &[], Launch::Foreground).unwrap();
            assert_eq!(layered.json["remote_access"]["enabled"], expected, "{raw}");
        }
    }

    #[test]
    fn list_settings_add_up_without_duplicates() {
        let file = "[remote_access]\nallowed_hosts = [\"tv.local\", \"pc.local\"]\n";
        let env = env_of(&[("FOLO_REMOTE_ACCESS_ALLOWED_HOSTS", " pc.local, 10.0.0.2 ,,")]);
        let flags = [
            ("remote_access.allowed_hosts", "studio.lan".to_owned()),
            ("remote_access.allowed_hosts", "tv.local".to_owned()),
        ];
        let layered = layer(Some(file), &env, &flags, Launch::Foreground).unwrap();
        assert_eq!(
            layered.json["remote_access"]["allowed_hosts"],
            serde_json::json!(["tv.local", "pc.local", "10.0.0.2", "studio.lan"])
        );
        let flag = layered
            .overrides
            .iter()
            .find(|o| o.source == Source::Flag)
            .unwrap();
        assert_eq!(flag.value, serde_json::json!(["studio.lan", "tv.local"]));
        assert_eq!(layered.overrides.len(), 2);
    }

    #[test]
    fn list_settings_stop_at_their_limit() {
        let hosts: Vec<String> = (0..40).map(|i| format!("host{i}")).collect();
        let env = env_of(&[("FOLO_REMOTE_ACCESS_ALLOWED_HOSTS", &hosts.join(","))]);
        let layered = layer(None, &env, &[], Launch::Foreground).unwrap();
        assert_eq!(
            layered.json["remote_access"]["allowed_hosts"]
                .as_array()
                .unwrap()
                .len(),
            32
        );
    }

    #[test]
    fn changes_split_hot_and_restart_settings() {
        let old = layer(None, &no_env, &[], Launch::Foreground).unwrap().json;
        let file = "role = \"source\"\nlog_level = \"debug\"\n[listen]\nport = 9000\n[local_source]\nenabled = false\n";
        let new = layer(Some(file), &no_env, &[], Launch::Foreground)
            .unwrap()
            .json;
        let found = changes(&old, &new);
        assert_eq!(found.applied, ["log_level", "local_source.enabled"]);
        assert_eq!(found.restart_required, ["role", "listen.port"]);
        assert_eq!(changes(&old, &old), Changes::default());
    }

    #[test]
    fn every_setting_has_its_variable_and_flag() {
        for spec in &SETTINGS {
            let variable = format!("FOLO_{}", spec.path.to_ascii_uppercase().replace('.', "_"));
            assert_eq!(spec.variable, variable);
            assert!(spec.flag.starts_with("--"));
        }
    }
}
