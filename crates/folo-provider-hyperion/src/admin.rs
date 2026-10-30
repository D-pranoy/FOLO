//! Basic administration ([ADR-0014](../../docs/decisions/ADR-0014-device-administration-scope.md),
//! `docs/integrations/hyperion.md`): instances, priorities with clear, component toggles,
//! automatic source selection, brightness, the API token and a link to Hyperion's own UI.
//!
//! Every action answers its current values. A read (`GET`) adds `change`, a sentence on what a
//! `POST` would change on Hyperion; when the read carries the values a `POST` would (as query
//! parameters), the sentence names them. A `POST` answers the values after the change, read
//! again from Hyperion. Every action but `instances` and `token` names the Hyperion instance it
//! applies to with `instance`, and otherwise applies to the selected one.

use std::sync::Arc;

use folo_provider::{AdminError, AdminMethod, ProviderError};
use serde_json::{Map, Value, json};

use crate::link::Link;
use crate::provider::HyperionProvider;
use crate::serverinfo::ServerInfo;
use crate::settings::WEB_UI_PORT;
use crate::wire::MIN_TOKEN_CHARS;

/// The components the administration switches, as Hyperion names them: the LED device,
/// smoothing, the black border, audio and V4L.
pub const SWITCHABLE_COMPONENTS: [&str; 5] =
    ["LEDDEVICE", "SMOOTHING", "BLACKBORDER", "AUDIO", "V4L"];

/// The priorities `clear` accepts: 1–253, and -1 for all of them.
const PRIORITIES: std::ops::RangeInclusive<i64> = 1..=253;
const ALL_PRIORITIES: i64 = -1;

type Outcome = Result<Value, AdminError>;

fn invalid(why: &str) -> AdminError {
    AdminError::BadRequest(format!("invalid request: {why}"))
}

fn read_only(method: AdminMethod, mut values: Value, change: &str) -> Value {
    if method == AdminMethod::Read {
        values["change"] = json!(change);
    }
    values
}

/// A named field of the request as a whole number.
fn number(body: &Value, key: &str) -> Result<Option<i64>, AdminError> {
    match &body[key] {
        Value::Null => Ok(None),
        // Query parameters arrive as text
        Value::String(s) => s
            .parse()
            .map(Some)
            .map_err(|_| invalid(&format!("{key} is not a number"))),
        other => other
            .as_i64()
            .map(Some)
            .ok_or_else(|| invalid(&format!("{key} is not a number"))),
    }
}

fn flag(body: &Value, key: &str) -> Result<Option<bool>, AdminError> {
    match &body[key] {
        Value::Null => Ok(None),
        Value::String(s) => match s.as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => Err(invalid(&format!("{key} is not true or false"))),
        },
        other => other
            .as_bool()
            .map(Some)
            .ok_or_else(|| invalid(&format!("{key} is not true or false"))),
    }
}

fn instance_of(body: &Value) -> Result<Option<u32>, AdminError> {
    number(body, "instance")?
        .map(|n| u32::try_from(n).map_err(|_| invalid("instance is out of range")))
        .transpose()
}

impl HyperionProvider {
    pub(crate) async fn run_admin(
        &self,
        action: &str,
        method: AdminMethod,
        body: &Value,
    ) -> Outcome {
        match action {
            "instances" => self.admin_instances(method, body),
            "priorities" => self.admin_priorities(method, body).await,
            "components" => self.admin_components(method, body).await,
            "source" => self.admin_source(method, body).await,
            "brightness" => self.admin_brightness(method, body).await,
            "token" => self.admin_token(method, body).await,
            "ui" => Ok(self.admin_ui(method)),
            other => Err(AdminError::UnknownAction(other.to_owned())),
        }
    }

    fn require_connected(&self) -> Result<(), AdminError> {
        if self.lock().links.is_empty() {
            return Err(ProviderError::Unreachable("not connected to Hyperion".to_owned()).into());
        }
        Ok(())
    }

    /// The connection of the instance a request names, or of the selected one, and which it is.
    async fn admin_link(&self, body: &Value) -> Result<(u32, Arc<Link>), AdminError> {
        self.require_connected()?;
        let instance = match instance_of(body)? {
            Some(n) => n,
            None => self.lock().selected,
        };
        let link = self.link_or_open(instance).await?;
        Ok((instance, link))
    }

    async fn admin_info(&self, body: &Value) -> Result<(u32, Arc<Link>, ServerInfo), AdminError> {
        let (instance, link) = self.admin_link(body).await?;
        let info = Self::server_info(&link).await?;
        Ok((instance, link, info))
    }

    fn instances_values(&self) -> Value {
        let state = self.lock();
        let instances: Vec<Value> = state
            .instances
            .iter()
            .map(|i| json!({"instance": i.number, "name": i.name, "running": i.running}))
            .collect();
        json!({"instances": instances, "selected": state.selected})
    }

    fn admin_instances(&self, method: AdminMethod, body: &Value) -> Outcome {
        self.require_connected()?;
        let wanted = instance_of(body)?;
        if method == AdminMethod::Change {
            let Some(wanted) = wanted else {
                return Err(invalid("instance is missing"));
            };
            self.select_instance(wanted)?;
        }
        let change = match wanted {
            Some(n) => format!(
                "Administration moves to instance {n}; nothing changes on Hyperion, and this \
                 provider keeps using every running instance for output"
            ),
            None => "Chooses the instance that priorities, components, source selection and \
                     brightness apply to by default; nothing changes on Hyperion"
                .to_owned(),
        };
        Ok(read_only(method, self.instances_values(), &change))
    }

    fn select_instance(&self, instance: u32) -> Result<(), AdminError> {
        let mut state = self.lock();
        if !state
            .instances
            .iter()
            .any(|i| i.number == instance && i.running)
        {
            return Err(invalid(&format!("instance {instance} isn't running")));
        }
        state.selected = instance;
        Ok(())
    }

    fn priorities_values(info: &ServerInfo) -> Value {
        let list: Vec<Value> = info
            .priorities
            .iter()
            .map(|p| {
                json!({
                    "priority": p.priority,
                    "origin": p.origin,
                    "owner": p.owner,
                    "component": p.component,
                    "active": p.active,
                    "visible": p.visible,
                })
            })
            .collect();
        json!({"priorities": list, "auto": info.auto_select})
    }

    async fn admin_priorities(&self, method: AdminMethod, body: &Value) -> Outcome {
        let (instance, link, mut info) = self.admin_info(body).await?;
        let priority = number(body, "priority")?;
        if let Some(priority) = priority
            && priority != ALL_PRIORITIES
            && !PRIORITIES.contains(&priority)
        {
            return Err(invalid("priority is not 1–253 or -1"));
        }
        if method == AdminMethod::Change {
            let Some(priority) = priority else {
                return Err(invalid("priority is missing"));
            };
            link.request(json!({"command": "clear", "priority": priority}))
                .await?;
            info = Self::server_info(&link).await?;
        }
        let change = match priority {
            Some(ALL_PRIORITIES) => {
                format!("Clears every priority of instance {instance}, ours and others'")
            }
            Some(p) => match info.priorities.iter().find(|e| e.priority == p) {
                Some(e) => format!(
                    "Clears priority {p} of instance {instance} ({} from {})",
                    e.component, e.origin
                ),
                None => format!(
                    "Priority {p} of instance {instance} isn't in the list; nothing changes"
                ),
            },
            None => "Clears one priority of the instance, or all of them with -1".to_owned(),
        };
        Ok(read_only(method, Self::priorities_values(&info), &change))
    }

    fn components_values(info: &ServerInfo) -> Value {
        let list: Vec<Value> = info
            .components
            .iter()
            .map(|c| json!({"name": c.name, "enabled": c.enabled}))
            .collect();
        json!({"components": list, "switchable": SWITCHABLE_COMPONENTS})
    }

    async fn admin_components(&self, method: AdminMethod, body: &Value) -> Outcome {
        let (instance, link, mut info) = self.admin_info(body).await?;
        let component = body["component"].as_str().map(str::to_uppercase);
        if let Some(name) = &component
            && !SWITCHABLE_COMPONENTS.contains(&name.as_str())
        {
            return Err(invalid(&format!("{name} can't be switched here")));
        }
        let enabled = flag(body, "enabled")?;
        if method == AdminMethod::Change {
            let (Some(name), Some(state)) = (&component, enabled) else {
                return Err(invalid("component and enabled are needed"));
            };
            let state_command = json!({"component": name, "state": state});
            link.request(json!({"command": "componentstate", "componentstate": state_command}))
                .await?;
            info = Self::server_info(&link).await?;
        }
        let change = match (&component, enabled) {
            (Some(name), Some(state)) => {
                let now = info.components.iter().find(|c| c.name == *name);
                let from = now.map_or("unknown", |c| if c.enabled { "on" } else { "off" });
                format!(
                    "Switches {name} of instance {instance} from {from} to {}",
                    if state { "on" } else { "off" }
                )
            }
            _ => "Switches one of the LED device, smoothing, black border, audio or V4L".to_owned(),
        };
        Ok(read_only(method, Self::components_values(&info), &change))
    }

    fn source_values(info: &ServerInfo) -> Value {
        let visible = info
            .priorities
            .iter()
            .find(|p| p.visible)
            .map(|p| p.priority);
        json!({"auto": info.auto_select, "visible": visible})
    }

    async fn admin_source(&self, method: AdminMethod, body: &Value) -> Outcome {
        let (instance, link, mut info) = self.admin_info(body).await?;
        let auto = flag(body, "auto")?;
        let priority = number(body, "priority")?;
        if auto.is_some() && priority.is_some() {
            return Err(invalid("auto and priority can't be given together"));
        }
        if priority.is_some_and(|p| !PRIORITIES.contains(&p)) {
            return Err(invalid("priority is not 1–253"));
        }
        // Hyperion answers success to any priority and ignores one it doesn't have
        if priority.is_some_and(|p| !info.priorities.iter().any(|e| e.priority == p)) {
            return Err(invalid("priority isn't in the list"));
        }
        if method == AdminMethod::Change {
            let command = match (auto, priority) {
                (Some(true), None) => json!({"command": "sourceselect", "auto": true}),
                (None, Some(p)) => json!({"command": "sourceselect", "priority": p}),
                _ => return Err(invalid("give auto=true, or the priority to show")),
            };
            link.request(command).await?;
            info = Self::server_info(&link).await?;
        }
        let change = match (auto, priority) {
            (Some(true), _) => {
                format!("Lets instance {instance} show the highest priority automatically")
            }
            (_, Some(p)) => format!("Pins instance {instance} to priority {p}"),
            _ => "Selects the source automatically, or pins one priority".to_owned(),
        };
        Ok(read_only(method, Self::source_values(&info), &change))
    }

    fn brightness_values(info: &ServerInfo) -> Value {
        let brightness = info.adjustments.iter().find_map(|(_, b)| *b);
        json!({"brightness": brightness})
    }

    async fn admin_brightness(&self, method: AdminMethod, body: &Value) -> Outcome {
        let (instance, link, mut info) = self.admin_info(body).await?;
        let wanted = number(body, "brightness")?
            .map(|b| u8::try_from(b).ok().filter(|b| *b <= 100))
            .map(|b| b.ok_or_else(|| invalid("brightness is not 0–100")))
            .transpose()?;
        if method == AdminMethod::Change {
            let Some(brightness) = wanted else {
                return Err(invalid("brightness is missing"));
            };
            // Every colour adjustment of the instance, each by its id
            let mut ids: Vec<Option<&String>> =
                info.adjustments.iter().map(|(id, _)| id.as_ref()).collect();
            if ids.is_empty() {
                ids.push(None);
            }
            for id in ids {
                let mut adjustment = Map::new();
                adjustment.insert("brightness".to_owned(), json!(brightness));
                if let Some(id) = id {
                    adjustment.insert("id".to_owned(), json!(id));
                }
                link.request(json!({"command": "adjustment", "adjustment": adjustment}))
                    .await?;
            }
            info = Self::server_info(&link).await?;
        }
        let current = Self::brightness_values(&info);
        let change = match wanted {
            Some(b) => format!(
                "Sets the brightness of instance {instance} from {} to {b}%",
                current["brightness"]
                    .as_u64()
                    .map_or("unknown".to_owned(), |c| format!("{c}%"))
            ),
            None => "Sets the brightness of the instance (0–100%)".to_owned(),
        };
        Ok(read_only(method, current, &change))
    }

    fn token_values(&self, required: Option<bool>) -> Value {
        json!({
            "required": required,
            "configured": self.lock().token.is_some(),
            "persisted": false,
        })
    }

    async fn token_required(&self) -> Option<bool> {
        let selected = self.lock().selected;
        let link = self.link(selected)?;
        let reply = link
            .request(json!({"command": "authorize", "subcommand": "tokenRequired"}))
            .await
            .ok()?;
        reply["info"]["required"].as_bool()
    }

    async fn admin_token(&self, method: AdminMethod, body: &Value) -> Outcome {
        self.require_connected()?;
        // A read ignores the token: it would travel in a URL
        let body = if method == AdminMethod::Read {
            &Value::Null
        } else {
            body
        };
        let offered = match &body["token"] {
            Value::Null => None,
            Value::String(t) if t.chars().count() >= MIN_TOKEN_CHARS => Some(t.clone()),
            Value::String(_) => {
                return Err(invalid(&format!(
                    "a token has at least {MIN_TOKEN_CHARS} characters"
                )));
            }
            _ => return Err(invalid("token is not a text")),
        };
        if method == AdminMethod::Change {
            let Some(token) = offered.clone() else {
                return Err(invalid("token is missing"));
            };
            // Tried on a connection of its own, so a refused token leaves the others alone
            let link = Link::open(&self.config.host, self.config.port).await?;
            link.request(json!({"command": "authorize", "subcommand": "login", "token": token}))
                .await?;
            link.close().await;
            self.lock().token = Some(token);
        }
        let values = self.token_values(self.token_required().await);
        let change = "Logs in with the token given in a POST body; if Hyperion accepts it, this \
                      provider uses it from now on and the node keeps it in secrets.json. \
                      Creating tokens stays in Hyperion's own UI";
        Ok(read_only(method, values, change))
    }

    fn admin_ui(&self, method: AdminMethod) -> Value {
        let host = &self.config.host;
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        let values = json!({"url": format!("http://{host}:{WEB_UI_PORT}/")});
        read_only(
            method,
            values,
            "Nothing: this is the address of Hyperion's own UI",
        )
    }
}
