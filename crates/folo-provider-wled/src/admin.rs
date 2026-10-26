//! Basic administration (`docs/integrations/wled.md`, ADR-0014): information, power,
//! brightness, rename, segments, realtime settings, reboot and the device's own UI.
//!
//! Every action states its change before it is made: a read answers with `summary` and
//! `changes` (what a change would change), and a change makes exactly that and answers with the
//! same list as `changed` and the device's values afterwards as `current`. Both come from one
//! plan, so what was announced is what is sent.

use folo_provider::{AdminError, AdminMethod, ProviderError};
use serde_json::{Map, Value, json};

use crate::api::DeviceApi;
use crate::describe::Description;
use crate::device::{Device, RealtimeSettings, Segment};
use crate::reader::{CFG_PATH, STATE_PATH, read_device};
use crate::settings::Config;

/// The longest name WLED keeps (its buffer is 33 bytes).
const NAME_MAX_CHARS: usize = 32;

/// Segments a device has room for when it doesn't say.
const SEGMENTS_DEFAULT_MAX: u32 = 32;

/// The actions this kind offers.
const ACTIONS: [&str; 8] = [
    "info",
    "power",
    "brightness",
    "rename",
    "segments",
    "realtime",
    "reboot",
    "ui",
];

/// What an action did, and the device's values afterwards when something was sent.
#[derive(Debug)]
pub struct AdminOutcome {
    pub answer: Value,
    pub device: Option<Device>,
}

/// One change: where, what it is now and what it becomes.
fn change(setting: &str, from: &Value, to: &Value) -> Value {
    json!({"setting": setting, "from": from, "to": to})
}

/// The request that makes a plan's changes.
struct Request {
    path: &'static str,
    body: Value,
}

struct Plan {
    summary: String,
    changes: Vec<Value>,
    request: Option<Request>,
}

fn invalid(message: impl Into<String>) -> AdminError {
    AdminError::Provider(ProviderError::Failed(format!(
        "invalid request: {}",
        message.into()
    )))
}

fn field_u32(body: &Value, key: &str) -> Result<Option<u32>, AdminError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| invalid(format!("{key} must be a non-negative whole number"))),
    }
}

fn field_bool(body: &Value, key: &str) -> Result<Option<bool>, AdminError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| invalid(format!("{key} must be true or false"))),
    }
}

fn segment_view(segment: &Segment) -> Value {
    json!({
        "id": segment.id,
        "name": segment.name,
        "start": segment.start,
        "stop": segment.stop,
        "grp": segment.grouping,
        "spc": segment.spacing,
        "on": segment.on,
        "bri": segment.brightness,
    })
}

fn realtime_view(realtime: &RealtimeSettings) -> Value {
    json!({
        "receive": realtime.receive,
        "main_segment_only": realtime.main_segment_only,
        "timeout_ds": realtime.timeout_ds,
        "force_max_brightness": realtime.force_max_brightness,
        "disable_gamma": realtime.disable_gamma,
        "dmx_address": realtime.dmx_address,
    })
}

/// Runs one action.
pub struct Admin<'a> {
    pub api: &'a dyn DeviceApi,
    pub config: &'a Config,
}

impl Admin<'_> {
    /// Runs an action: a read states its change, a change makes it.
    ///
    /// # Errors
    /// [`AdminError::UnknownAction`] for an action the kind doesn't have, and
    /// [`AdminError::Provider`] when the request is invalid or the device refuses it.
    pub async fn run(
        &self,
        action: &str,
        method: AdminMethod,
        body: &Value,
    ) -> Result<AdminOutcome, AdminError> {
        if !ACTIONS.contains(&action) {
            return Err(AdminError::UnknownAction(action.to_owned()));
        }
        let device = read_device(self.api, self.config.pin.as_deref()).await?;
        if action == "info" {
            return Ok(AdminOutcome {
                answer: self.info(&device),
                device: None,
            });
        }
        if action == "ui" {
            return Ok(AdminOutcome {
                answer: json!({"url": self.ui_url()}),
                device: None,
            });
        }
        let plan = self.plan(action, &device, body)?;
        let mut answer = json!({
            "action": action,
            "current": Self::current(action, &device),
            "summary": plan.summary,
        });
        if method == AdminMethod::Read {
            answer["changes"] = Value::Array(plan.changes);
            return Ok(AdminOutcome {
                answer,
                device: None,
            });
        }
        let Some(request) = plan.request.filter(|_| !plan.changes.is_empty()) else {
            answer["changed"] = json!([]);
            return Ok(AdminOutcome {
                answer,
                device: None,
            });
        };
        self.send(&request).await?;
        let after = read_device(self.api, self.config.pin.as_deref()).await?;
        answer["changed"] = Value::Array(plan.changes);
        answer["current"] = Self::current(action, &after);
        Ok(AdminOutcome {
            answer,
            device: Some(after),
        })
    }

    fn ui_url(&self) -> String {
        format!("http://{}/", self.api.authority())
    }

    async fn send(&self, request: &Request) -> Result<(), ProviderError> {
        let mut body = request.body.clone();
        if request.path == CFG_PATH
            && let (Some(pin), Some(map)) = (&self.config.pin, body.as_object_mut())
        {
            map.insert("pin".into(), Value::String(pin.clone()));
        }
        self.api.post(request.path, &body).await.map(|_| ())
    }

    fn info(&self, device: &Device) -> Value {
        let description = Description::of(device);
        json!({
            "name": device.name,
            "version": device.version,
            "mac": device.mac,
            "on": device.on,
            "bri": device.brightness,
            "leds": {
                "count": device.led_count,
                "lc": device.lc,
                "matrix": device.matrix.map(|(w, h)| json!({"w": w, "h": h})),
                "max_segments": device.max_segments,
            },
            "segments": device.segments.iter().map(segment_view).collect::<Vec<_>>(),
            "realtime": device.realtime.as_ref().map(realtime_view),
            "main_segment": device.live_segment,
            "targets": description.targets.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            "warnings": description.warnings,
            "ui_url": self.ui_url(),
        })
    }

    fn current(action: &str, device: &Device) -> Value {
        match action {
            "power" => json!({"on": device.on}),
            "brightness" => json!({"bri": device.brightness}),
            "rename" => json!({"name": device.name}),
            "reboot" => json!({"name": device.name, "version": device.version}),
            "segments" => json!({
                "segments": device.segments.iter().map(segment_view).collect::<Vec<_>>(),
                "max_segments": device.max_segments.unwrap_or(SEGMENTS_DEFAULT_MAX),
                "led_count": device.led_count,
            }),
            "realtime" => device.realtime.as_ref().map_or(Value::Null, realtime_view),
            _ => Value::Null,
        }
    }

    fn plan(&self, action: &str, device: &Device, body: &Value) -> Result<Plan, AdminError> {
        match action {
            "power" => Self::plan_power(device, body),
            "brightness" => Self::plan_brightness(device, body),
            "rename" => Self::plan_rename(device, body),
            "reboot" => Ok(Self::plan_reboot()),
            "segments" => Self::plan_segments(device, body),
            "realtime" => self.plan_realtime(device),
            other => Err(AdminError::UnknownAction(other.to_owned())),
        }
    }

    fn nothing(summary: &str) -> Plan {
        Plan {
            summary: summary.to_owned(),
            changes: Vec::new(),
            request: None,
        }
    }

    fn plan_power(device: &Device, body: &Value) -> Result<Plan, AdminError> {
        let Some(on) = field_bool(body, "on")? else {
            return Ok(Self::nothing(
                "Reads the power state. Send {\"on\": true} or {\"on\": false} to change it.",
            ));
        };
        if on == device.on {
            return Ok(Self::nothing(if on {
                "The device is already on."
            } else {
                "The device is already off."
            }));
        }
        Ok(Plan {
            summary: format!("Turn the device {}.", if on { "on" } else { "off" }),
            changes: vec![change("on", &json!(device.on), &json!(on))],
            request: Some(Request {
                path: STATE_PATH,
                body: json!({"on": on}),
            }),
        })
    }

    fn plan_brightness(device: &Device, body: &Value) -> Result<Plan, AdminError> {
        let Some(bri) = field_u32(body, "bri")? else {
            return Ok(Self::nothing(
                "Reads the master brightness (0–255). Send {\"bri\": 255} to change it.",
            ));
        };
        let bri = u8::try_from(bri).map_err(|_| invalid("bri must be 0–255"))?;
        if bri == device.brightness {
            return Ok(Self::nothing("The brightness is already that."));
        }
        Ok(Plan {
            summary: format!(
                "Set the master brightness from {} to {bri} (of 255). Realtime data is scaled \
                 by it unless \"force max brightness\" is on.",
                device.brightness
            ),
            changes: vec![change("bri", &json!(device.brightness), &json!(bri))],
            request: Some(Request {
                path: STATE_PATH,
                body: json!({"bri": bri}),
            }),
        })
    }

    fn plan_rename(device: &Device, body: &Value) -> Result<Plan, AdminError> {
        let Some(name) = body.get("name").and_then(Value::as_str) else {
            return Ok(Self::nothing(
                "Reads the device's name. Send {\"name\": \"…\"} to rename it.",
            ));
        };
        let name = name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
            return Err(invalid(format!(
                "name must be 1–{NAME_MAX_CHARS} characters"
            )));
        }
        if name == device.name {
            return Ok(Self::nothing("The device already has that name."));
        }
        Ok(Plan {
            summary: format!(
                "Rename the device from \"{}\" to \"{name}\". This is its server description; \
                 its network (mDNS) name stays.",
                device.name
            ),
            changes: vec![change("name", &json!(device.name), &json!(name))],
            request: Some(Request {
                path: CFG_PATH,
                body: json!({"id": {"name": name}}),
            }),
        })
    }

    fn plan_reboot() -> Plan {
        Plan {
            summary: "Restart the device. Its lights stop following FOLO until it is back, and \
                      FOLO reconnects by itself."
                .into(),
            changes: vec![change("reboot", &json!(false), &json!(true))],
            request: Some(Request {
                path: STATE_PATH,
                body: json!({"rb": true}),
            }),
        }
    }

    /// The realtime settings FOLO needs: receive on, the timeout the instance names, full
    /// brightness (not scaled by the device's master brightness) and no gamma on the device,
    /// because FOLO applies its own. Other settings that matter are reported, not changed.
    fn plan_realtime(&self, device: &Device) -> Result<Plan, AdminError> {
        let Some(realtime) = &device.realtime else {
            return Err(AdminError::Provider(ProviderError::Refused(
                "the device's settings can't be read: give this instance the device's PIN".into(),
            )));
        };
        let timeout_ds =
            u32::try_from(self.config.timeout_s.saturating_mul(10)).unwrap_or(u32::MAX);
        let wanted = [
            ("en", json!(realtime.receive), json!(true)),
            ("timeout", json!(realtime.timeout_ds), json!(timeout_ds)),
            ("maxbri", json!(realtime.force_max_brightness), json!(true)),
            ("no-gc", json!(realtime.disable_gamma), json!(true)),
        ];
        let mut live = Map::new();
        let mut changes = Vec::new();
        for (key, from, to) in wanted {
            if from != to {
                changes.push(change(key, &from, &to));
                live.insert(key.to_owned(), to);
            }
        }
        let summary = if changes.is_empty() {
            "The realtime settings are right: UDP receive on, the timeout set, full brightness              and no gamma on the device."
                .to_owned()
        } else {
            let keys: Vec<&str> = changes
                .iter()
                .filter_map(|c| c.get("setting").and_then(Value::as_str))
                .collect();
            format!(
                "Change the realtime settings ({}): UDP receive on, a timeout of {} s, force                  max brightness on and the device's realtime gamma off (FOLO applies gamma).",
                keys.join(", "),
                self.config.timeout_s
            )
        };
        Ok(Plan {
            summary,
            request: (!live.is_empty()).then(|| Request {
                path: CFG_PATH,
                body: json!({"if": {"live": Value::Object(live)}}),
            }),
            changes,
        })
    }

    fn plan_segments(device: &Device, body: &Value) -> Result<Plan, AdminError> {
        let Some(op) = body.get("op").and_then(Value::as_str) else {
            return Ok(Self::nothing(
                "Reads the segments. Send {\"op\": \"create\" | \"edit\" | \"delete\", …} to change them.",
            ));
        };
        match op {
            "create" => Self::plan_segment_create(device, body),
            "edit" => Self::plan_segment_edit(device, body),
            "delete" => Self::plan_segment_delete(device, body),
            other => Err(invalid(format!("unknown segment operation: {other}"))),
        }
    }

    fn segment_request(entry: Map<String, Value>) -> Request {
        Request {
            path: STATE_PATH,
            body: json!({"seg": [Value::Object(entry)]}),
        }
    }

    /// The segment fields a body may carry, as WLED names them.
    fn segment_fields(body: &Value) -> Result<Map<String, Value>, AdminError> {
        let mut entry = Map::new();
        for (key, wled) in [
            ("start", "start"),
            ("stop", "stop"),
            ("grp", "grp"),
            ("spc", "spc"),
        ] {
            if let Some(n) = field_u32(body, key)? {
                entry.insert(wled.to_owned(), json!(n));
            }
        }
        if let Some(bri) = field_u32(body, "bri")? {
            let bri = u8::try_from(bri).map_err(|_| invalid("bri must be 0–255"))?;
            entry.insert("bri".to_owned(), json!(bri));
        }
        if let Some(on) = field_bool(body, "on")? {
            entry.insert("on".to_owned(), json!(on));
        }
        if let Some(name) = body.get("name").and_then(Value::as_str) {
            if name.chars().count() > NAME_MAX_CHARS {
                return Err(invalid(format!(
                    "name must be at most {NAME_MAX_CHARS} characters"
                )));
            }
            entry.insert("n".to_owned(), json!(name));
        }
        Ok(entry)
    }

    fn check_range(device: &Device, start: u32, stop: u32) -> Result<(), AdminError> {
        if stop <= start {
            return Err(invalid("stop must be above start"));
        }
        if stop > device.led_count {
            return Err(invalid(format!(
                "stop is {stop}, but the device has {} LEDs",
                device.led_count
            )));
        }
        Ok(())
    }

    fn plan_segment_create(device: &Device, body: &Value) -> Result<Plan, AdminError> {
        let max = device.max_segments.unwrap_or(SEGMENTS_DEFAULT_MAX);
        let id = match field_u32(body, "id")? {
            Some(id) => id,
            None => (0..max)
                .find(|id| device.segment(*id).is_none())
                .ok_or_else(|| invalid("the device has no free segment"))?,
        };
        if id >= max || device.segment(id).is_some() {
            return Err(invalid(format!(
                "segment {id} exists or is out of range (0–{})",
                max - 1
            )));
        }
        let (Some(start), Some(stop)) = (field_u32(body, "start")?, field_u32(body, "stop")?)
        else {
            return Err(invalid("a new segment needs start and stop"));
        };
        Self::check_range(device, start, stop)?;
        let mut entry = Self::segment_fields(body)?;
        entry.insert("id".to_owned(), json!(id));
        Ok(Plan {
            summary: format!(
                "Create segment {id} over LEDs {start}–{} ({} LEDs).",
                stop - 1,
                stop - start
            ),
            changes: vec![change(
                &format!("segment {id}"),
                &Value::Null,
                &Value::Object(entry.clone()),
            )],
            request: Some(Self::segment_request(entry)),
        })
    }

    fn plan_segment_edit(device: &Device, body: &Value) -> Result<Plan, AdminError> {
        let id = field_u32(body, "id")?.ok_or_else(|| invalid("edit needs a segment id"))?;
        let segment = device
            .segment(id)
            .ok_or_else(|| invalid(format!("the device has no segment {id}")))?;
        let mut entry = Self::segment_fields(body)?;
        if entry.is_empty() {
            return Ok(Self::nothing("Nothing to change: no fields were given."));
        }
        let start = field_u32(body, "start")?.unwrap_or(segment.start);
        let stop = field_u32(body, "stop")?.unwrap_or(segment.stop);
        Self::check_range(device, start, stop)?;
        entry.insert("id".to_owned(), json!(id));
        let fields: Vec<&str> = entry
            .keys()
            .map(String::as_str)
            .filter(|k| *k != "id")
            .collect();
        Ok(Plan {
            summary: format!(
                "Edit segment {id} (now LEDs {}–{}): {}.",
                segment.start,
                segment.stop - 1,
                fields.join(", ")
            ),
            changes: vec![change(
                &format!("segment {id}"),
                &segment_view(segment),
                &Value::Object(entry.clone()),
            )],
            request: Some(Self::segment_request(entry)),
        })
    }

    fn plan_segment_delete(device: &Device, body: &Value) -> Result<Plan, AdminError> {
        let id = field_u32(body, "id")?.ok_or_else(|| invalid("delete needs a segment id"))?;
        let segment = device
            .segment(id)
            .ok_or_else(|| invalid(format!("the device has no segment {id}")))?;
        let mut entry = Map::new();
        entry.insert("id".to_owned(), json!(id));
        entry.insert("stop".to_owned(), json!(0));
        Ok(Plan {
            summary: format!(
                "Delete segment {id} (LEDs {}–{}).",
                segment.start,
                segment.stop - 1
            ),
            changes: vec![change(
                &format!("segment {id}"),
                &segment_view(segment),
                &Value::Null,
            )],
            request: Some(Self::segment_request(entry)),
        })
    }
}
