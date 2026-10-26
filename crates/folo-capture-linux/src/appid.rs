//! The application id the portal knows the agent by (ADR-0037 D16).
//!
//! The ScreenCast portal stores a restore token per application, so it needs a
//! stable id for a program that isn't in a sandbox. The agent gets one in two
//! ways. On xdg-desktop-portal 1.19 and newer it registers itself through the
//! host registry. Otherwise the portal reads the id from the name of the
//! systemd unit the process runs in (`app-<id>-<random>.scope`), which the
//! node must create when it starts the agent: [`transient_scope_name`] is that
//! name, and the node starts the agent with
//! `systemd-run --user --scope --unit=<name> folo-capture ...`.

#[cfg(feature = "portal")]
use std::collections::HashMap;

#[cfg(feature = "portal")]
use ashpd::zbus::zvariant::Value;
#[cfg(feature = "portal")]
use ashpd::zbus::{Connection, Proxy};

/// The agent's application id.
pub const APP_ID: &str = "io.github.d-pranoy.folo-capture";

#[cfg(feature = "portal")]
const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
#[cfg(feature = "portal")]
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
#[cfg(feature = "portal")]
const REGISTRY_INTERFACE: &str = "org.freedesktop.host.portal.Registry";

/// systemd writes `-` in a unit name's application id as `\x2d`, because `-`
/// separates the parts of the name.
fn escape_unit_part(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '-' {
            escaped.push_str("\\x2d");
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn unescape_unit_part(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find("\\x") {
        plain.push_str(&rest[..position]);
        let code = rest.get(position + 2..position + 4);
        if let Some(byte) = code.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
            plain.push(char::from(byte));
            rest = &rest[position + 4..];
        } else {
            plain.push_str("\\x");
            rest = &rest[position + 2..];
        }
    }
    plain.push_str(rest);
    plain
}

/// The name of the transient scope the node starts the agent in when the
/// portal has no host registry: `app-<escaped id>-<random>.scope`.
#[must_use]
pub fn transient_scope_name(random: &str) -> String {
    format!("app-{}-{random}.scope", escape_unit_part(APP_ID))
}

/// The application id a cgroup file (`/proc/self/cgroup`) names, by the
/// systemd convention for desktop applications:
/// `app[-<launcher>]-<id>[-<random>].scope` or `app[-<launcher>]-<id>[@<instance>].service`.
#[must_use]
pub fn app_id_from_cgroup(cgroup: &str) -> Option<String> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let unit = path.rsplit('/').next()?;
    let (stem, is_scope) = match (unit.strip_suffix(".scope"), unit.strip_suffix(".service")) {
        (Some(stem), _) => (stem, true),
        (None, Some(stem)) => (stem, false),
        (None, None) => return None,
    };
    let name = stem.strip_prefix("app-")?;
    let name = if is_scope {
        name.rsplit_once('-')?.0
    } else {
        name.split('@').next()?
    };
    let id = name.rsplit('-').next().filter(|id| !id.is_empty())?;
    Some(unescape_unit_part(id))
}

/// How the portal comes to know the agent's application id.
#[cfg(feature = "portal")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// The agent registered itself through the portal's host registry.
    Registered,
    /// The agent already runs in a unit that carries the id.
    InScope,
    /// The agent is in a sandbox, which gives the portal its own id.
    Sandboxed,
    /// The portal can't be told: it is older than 1.19 and the agent isn't in
    /// its scope, or it refused. The capture works, but the portal can't keep
    /// a restore token for the agent, and asks every time.
    Unknown(String),
}

/// The text to show when the portal can't be told the agent's id.
#[cfg(feature = "portal")]
pub const UNKNOWN_HINT: &str = "the desktop portal doesn't know this program, so it will ask which screen to share every time (xdg-desktop-portal 1.19 or newer, or starting the agent in a systemd scope, fixes that)";

/// Tells the portal the agent's id over `connection`, unless the process
/// already has one. It has to be the connection that goes on to make the
/// `ScreenCast` calls, and has to come before them.
///
/// The call is made here rather than through `ashpd::register_host_app`,
/// which checks the id against a stricter grammar than the portal's (it
/// refuses a hyphen outside the last element, as in `d-pranoy`) and so never
/// lets the portal give its own answer.
#[cfg(feature = "portal")]
pub async fn register(connection: &Connection, cgroup: Option<&str>) -> Registration {
    if cgroup.and_then(app_id_from_cgroup).as_deref() == Some(APP_ID) {
        return Registration::InScope;
    }
    if ashpd::is_sandboxed() {
        return Registration::Sandboxed;
    }
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let called = async {
        let registry = Proxy::new(connection, PORTAL_NAME, PORTAL_PATH, REGISTRY_INTERFACE).await?;
        registry.call_method("Register", &(APP_ID, options)).await
    };
    match called.await {
        Ok(_) => Registration::Registered,
        Err(error) => Registration::Unknown(error.to_string()),
    }
}
