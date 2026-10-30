//! What `serverinfo` tells: the instances, and for the instance a connection is on, its LED
//! rectangles, priorities, components, adjustments and automatic source selection.

use folo_provider::ProviderError;
use serde_json::Value;

/// One Hyperion instance of the server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceInfo {
    pub number: u32,
    pub name: String,
    pub running: bool,
}

/// An LED's rectangle on the screen, as fractions (0–1) of its width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedRect {
    pub hmin: f64,
    pub hmax: f64,
    pub vmin: f64,
    pub vmax: f64,
}

/// One entry of the priority list.
#[derive(Clone, Debug, PartialEq)]
pub struct PriorityInfo {
    pub priority: i64,
    pub origin: String,
    pub owner: String,
    pub component: String,
    pub active: bool,
    pub visible: bool,
}

/// A component and whether it is on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentInfo {
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerInfo {
    pub instances: Vec<InstanceInfo>,
    pub leds: Vec<LedRect>,
    pub priorities: Vec<PriorityInfo>,
    pub auto_select: bool,
    pub components: Vec<ComponentInfo>,
    /// The ids of the adjustments (colour correction sets), each with its brightness (0–100).
    pub adjustments: Vec<(Option<String>, Option<u8>)>,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn fraction(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0).clamp(0.0, 1.0)
}

impl ServerInfo {
    /// Reads the answer to `serverinfo`. What is missing is empty: an old or minimal Hyperion
    /// without an instance list has one instance.
    ///
    /// # Errors
    /// [`ProviderError::Failed`] when the answer has no `info`.
    pub fn parse(reply: &Value) -> Result<Self, ProviderError> {
        let info = &reply["info"];
        if !info.is_object() {
            return Err(ProviderError::Failed(
                "Hyperion's serverinfo has no info".to_owned(),
            ));
        }
        let instances = list(&info["instance"], |i| {
            let number = u32::try_from(i["instance"].as_u64()?).ok()?;
            Some(InstanceInfo {
                number,
                name: text(&i["friendly_name"]),
                running: i["running"].as_bool().unwrap_or(false),
            })
        });
        let leds = list(&info["leds"], |l| {
            Some(LedRect {
                hmin: fraction(&l["hmin"]),
                hmax: fraction(&l["hmax"]),
                vmin: fraction(&l["vmin"]),
                vmax: fraction(&l["vmax"]),
            })
        });
        let priorities = list(&info["priorities"], |p| {
            Some(PriorityInfo {
                priority: p["priority"].as_i64()?,
                origin: text(&p["origin"]),
                owner: text(&p["owner"]),
                component: text(&p["componentId"]),
                active: p["active"].as_bool().unwrap_or(false),
                visible: p["visible"].as_bool().unwrap_or(false),
            })
        });
        let components = list(&info["components"], |c| {
            Some(ComponentInfo {
                name: c["name"].as_str()?.to_owned(),
                enabled: c["enabled"].as_bool().unwrap_or(false),
            })
        });
        let adjustments = list(&info["adjustment"], |a| {
            Some((
                a["id"].as_str().map(str::to_owned),
                a["brightness"].as_u64().and_then(|b| u8::try_from(b).ok()),
            ))
        });
        Ok(Self {
            instances,
            leds,
            priorities,
            auto_select: info["priorities_autoselect"].as_bool().unwrap_or(false),
            components,
            adjustments,
        })
    }

    /// The instances that run, lowest number first.
    #[must_use]
    pub fn running(&self) -> Vec<&InstanceInfo> {
        let mut running: Vec<_> = self.instances.iter().filter(|i| i.running).collect();
        running.sort_by_key(|i| i.number);
        running
    }
}

fn list<T>(value: &Value, read: impl Fn(&Value) -> Option<T>) -> Vec<T> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(read).collect())
        .unwrap_or_default()
}
