//! Basic administration (`docs/integrations/hue.md`, basic administration; ADR-0014).
//!
//! Every action that changes the bridge says first, on a read, what it would change. Scenes,
//! automations and accessories stay in the Hue app.
//!
//! | Action | Read | Change |
//! |---|---|---|
//! | `bridge` | the bridge's model, versions and address | |
//! | `lights` | the lights | |
//! | `light_rename` | what would be renamed | renames the device (the name the Hue app shows) |
//! | `light_identify` | what would blink | makes the light blink |
//! | `rooms` | rooms and zones, read only | |
//! | `entertainment` | the areas, and whether another app streams | |
//! | `entertainment_create` | what would be created | creates an area from lights |
//! | `entertainment_update` | what would change | renames, retypes or re-places lights |
//! | `entertainment_delete` | what would go | deletes an area |
//! | `entertainment_take_over` | whose stream would stop | stops another app's stream |
//! | `update_status` | the bridge's software update state | |

use folo_provider::{AdminError, AdminMethod, ProviderError};
use serde_json::{Value, json};

use crate::model::{EntertainmentConfiguration, Inventory, Light};
use crate::provider::{Connection, HueProvider};
use crate::rest::RestError;

type AdminResult = Result<Value, AdminError>;

/// The area types the bridge knows.
const CONFIGURATION_TYPES: [&str; 5] = ["screen", "monitor", "music", "3dspace", "other"];
/// The longest name the bridge takes.
const NAME_LIMIT: usize = 32;

fn invalid(message: impl Into<String>) -> AdminError {
    AdminError::Provider(ProviderError::Failed(format!(
        "invalid request: {}",
        message.into()
    )))
}

fn rest(e: RestError) -> AdminError {
    AdminError::Provider(e.into())
}

fn text<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

fn required<'a>(body: &'a Value, key: &str) -> Result<&'a str, AdminError> {
    text(body, key).ok_or_else(|| invalid(format!("{key} is required")))
}

fn checked_name(body: &Value) -> Result<Option<String>, AdminError> {
    let Some(name) = text(body, "name") else {
        return Ok(None);
    };
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_LIMIT {
        return Err(invalid(format!("name must be 1–{NAME_LIMIT} characters")));
    }
    Ok(Some(name.to_owned()))
}

fn checked_type(body: &Value) -> Result<Option<String>, AdminError> {
    let Some(kind) = text(body, "configuration_type") else {
        return Ok(None);
    };
    if !CONFIGURATION_TYPES.contains(&kind) {
        return Err(invalid(format!(
            "configuration_type must be one of {}",
            CONFIGURATION_TYPES.join(", ")
        )));
    }
    Ok(Some(kind.to_owned()))
}

/// The change an action would make, as a read answers it.
fn preview(action: &str, would: &str, current: &Value, after: &Value) -> Value {
    json!({"action": action, "would": would, "current": current, "after": after})
}

/// A position of a light in an area, each coordinate in the bridge's [-1, 1].
fn checked_positions(item: &Value) -> Result<Vec<Value>, AdminError> {
    let Some(list) = item.get("positions").and_then(Value::as_array) else {
        return Ok(vec![json!({"x": 0.0, "y": 0.0, "z": 0.0})]);
    };
    if list.is_empty() || list.len() > 2 {
        return Err(invalid("a light has one or two positions"));
    }
    for position in list {
        for axis in ["x", "y", "z"] {
            let value = match position.get(axis) {
                None => 0.0,
                Some(v) => v
                    .as_f64()
                    .ok_or_else(|| invalid(format!("position {axis} must be a number")))?,
            };
            if !(-1.0..=1.0).contains(&value) {
                return Err(invalid(format!("position {axis} must be between -1 and 1")));
            }
        }
    }
    Ok(list.clone())
}

impl Inventory {
    /// The entertainment service of a light.
    fn service_of_light(&self, light: &str) -> Option<&str> {
        let owner = &self.lights.iter().find(|l| l.id == light)?.owner.rid;
        self.services
            .iter()
            .find(|s| s.owner.rid == *owner)
            .map(|s| s.id.as_str())
    }

    fn light_name(light: &Light) -> String {
        if light.metadata.name.is_empty() {
            light.id.clone()
        } else {
            light.metadata.name.clone()
        }
    }
}

impl HueProvider {
    pub(crate) async fn administer(
        &self,
        action: &str,
        method: AdminMethod,
        body: Value,
    ) -> AdminResult {
        use AdminMethod::{Change, Read};
        let connection = self.connected()?;
        match (action, method) {
            ("bridge", Read) => self.bridge_info(&connection).await,
            ("lights", Read) => self.lights(&connection).await,
            ("light_rename", m) => self.rename_light(&connection, m, &body).await,
            ("light_identify", m) => self.identify_light(&connection, m, &body).await,
            ("rooms", Read) => self.rooms(&connection).await,
            ("entertainment", Read) => self.areas(&connection).await,
            ("entertainment_create", m) => self.create_area(&connection, m, &body).await,
            ("entertainment_update", m) => self.update_area(&connection, m, &body).await,
            ("entertainment_delete", m) => self.delete_area(&connection, m, &body).await,
            ("entertainment_take_over", m) => self.take_over(&connection, m).await,
            ("update_status", Read) => self.update_status(&connection).await,
            ("bridge" | "lights" | "rooms" | "entertainment" | "update_status", Change) => {
                Err(AdminError::UnknownAction(format!("{action} (read only)")))
            }
            _ => Err(AdminError::UnknownAction(action.to_owned())),
        }
    }

    async fn bridge_info(&self, connection: &Connection) -> AdminResult {
        let config = connection.client.public_config().await.map_err(rest)?;
        let info = crate::bridge::BridgeInfo::from_config(&config)?;
        Ok(json!({
            "name": info.name,
            "bridge_id": info.bridge_id,
            "model_id": info.model_id,
            "software_version": info.software_version,
            "api_version": info.api_version,
            "address": self.endpoint.host,
        }))
    }

    async fn lights(&self, connection: &Connection) -> AdminResult {
        let inventory = self.inventory(&connection.client).await?;
        let lights: Vec<Value> = inventory
            .lights
            .iter()
            .map(|light| {
                json!({
                    "id": light.id,
                    "name": Inventory::light_name(light),
                    "device": light.owner.rid,
                    "gradient": light.gradient.is_some(),
                    "entertainment": inventory.service_of_light(&light.id).is_some(),
                })
            })
            .collect();
        Ok(json!({"lights": lights}))
    }

    async fn rename_light(
        &self,
        connection: &Connection,
        method: AdminMethod,
        body: &Value,
    ) -> AdminResult {
        let id = required(body, "id")?;
        let name = checked_name(body)?.ok_or_else(|| invalid("name is required"))?;
        let inventory = self.inventory(&connection.client).await?;
        let light = inventory
            .lights
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| invalid(format!("no light {id}")))?;
        let current = Inventory::light_name(light);
        if method == AdminMethod::Read {
            let would = format!("Renames the light {current:?} to {name:?} on the bridge");
            return Ok(preview(
                "light_rename",
                &would,
                &json!(current),
                &json!(name),
            ));
        }
        // The device carries the name the Hue app shows
        let path = format!("device/{}", light.owner.rid);
        connection
            .client
            .put(&path, &json!({"metadata": {"name": name}}))
            .await
            .map_err(rest)?;
        self.refresh_targets(&connection.client).await?;
        Ok(json!({"id": id, "name": name}))
    }

    async fn identify_light(
        &self,
        connection: &Connection,
        method: AdminMethod,
        body: &Value,
    ) -> AdminResult {
        let id = required(body, "id")?;
        let inventory = self.inventory(&connection.client).await?;
        let light = inventory
            .lights
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| invalid(format!("no light {id}")))?;
        let name = Inventory::light_name(light);
        if method == AdminMethod::Read {
            let would =
                format!("Makes the light {name:?} blink for a few seconds; nothing is changed");
            return Ok(preview(
                "light_identify",
                &would,
                &Value::Null,
                &Value::Null,
            ));
        }
        let body = json!({"identify": {"action": "identify"}});
        connection
            .client
            .put(&format!("light/{id}"), &body)
            .await
            .map_err(rest)?;
        Ok(json!({"id": id, "name": name, "identified": true}))
    }

    async fn rooms(&self, connection: &Connection) -> AdminResult {
        let client = &connection.client;
        let (rooms, zones, devices, lights) = tokio::try_join!(
            client.resources("room"),
            client.resources("zone"),
            client.resources("device"),
            client.resources("light"),
        )
        .map_err(rest)?;
        let inventory = Inventory::from_resources(Vec::new(), Vec::new(), lights, devices);
        let named = |kind: &str, list: &[Value]| -> Vec<Value> {
            list.iter()
                .map(|item| room_summary(kind, item, &inventory))
                .collect()
        };
        let mut all = named("room", &rooms);
        all.extend(named("zone", &zones));
        Ok(json!({"rooms": all}))
    }

    /// The areas and who streams to them.
    async fn areas(&self, connection: &Connection) -> AdminResult {
        let inventory = self.inventory(&connection.client).await?;
        let me = connection.application_id.as_deref().unwrap_or_default();
        let targets = inventory.targets();
        let areas: Vec<Value> = inventory
            .configurations
            .iter()
            .map(|area| {
                let lights: Vec<&str> = targets
                    .iter()
                    .filter(|t| t.configuration == area.id)
                    .map(|t| t.name.as_str())
                    .collect();
                json!({
                    "id": area.id,
                    "name": area.metadata.name,
                    "configuration_type": area.configuration_type,
                    "status": area.status,
                    "channels": area.channel_ids().len(),
                    "lights": lights,
                    "in_use_by_another_app": area.streamed_by_other(me),
                })
            })
            .collect();
        Ok(json!({"areas": areas}))
    }

    /// The service locations the bridge takes for these lights.
    fn service_locations(inventory: &Inventory, lights: &Value) -> Result<Vec<Value>, AdminError> {
        let items = lights
            .as_array()
            .filter(|l| !l.is_empty())
            .ok_or_else(|| invalid("lights is required"))?;
        items
            .iter()
            .map(|item| {
                let service = match (text(item, "service"), text(item, "light")) {
                    (Some(service), _) => service.to_owned(),
                    (None, Some(light)) => inventory
                        .service_of_light(light)
                        .ok_or_else(|| invalid(format!("light {light} can't stream")))?
                        .to_owned(),
                    (None, None) => return Err(invalid("each light needs a light or a service")),
                };
                let mut location = json!({
                    "service": {"rid": service, "rtype": "entertainment"},
                    "positions": checked_positions(item)?,
                });
                if let Some(factor) = item.get("equalization_factor").and_then(Value::as_f64) {
                    location["equalization_factor"] = json!(factor);
                }
                Ok(location)
            })
            .collect()
    }

    async fn create_area(
        &self,
        connection: &Connection,
        method: AdminMethod,
        body: &Value,
    ) -> AdminResult {
        let name = checked_name(body)?.ok_or_else(|| invalid("name is required"))?;
        let kind = checked_type(body)?.unwrap_or_else(|| "other".to_owned());
        let inventory = self.inventory(&connection.client).await?;
        let locations =
            Self::service_locations(&inventory, body.get("lights").unwrap_or(&Value::Null))?;
        if method == AdminMethod::Read {
            let would = format!(
                "Creates the entertainment area {name:?} ({kind}) with {} light(s); the bridge derives its channels",
                locations.len()
            );
            return Ok(preview(
                "entertainment_create",
                &would,
                &Value::Null,
                &json!({"name": name, "configuration_type": kind}),
            ));
        }
        let request = json!({
            "metadata": {"name": name},
            "configuration_type": kind,
            "locations": {"service_locations": locations},
        });
        let created = connection
            .client
            .post("entertainment_configuration", &request)
            .await
            .map_err(rest)?;
        self.refresh_targets(&connection.client).await?;
        let id = created
            .first()
            .and_then(|c| c.get("rid"))
            .cloned()
            .unwrap_or(Value::Null);
        Ok(json!({"id": id, "name": name, "configuration_type": kind}))
    }

    /// The area `body.id`, which must not be the one FOLO is streaming to.
    async fn editable_area(
        &self,
        connection: &Connection,
        body: &Value,
    ) -> Result<(EntertainmentConfiguration, Inventory), AdminError> {
        let id = required(body, "id")?;
        let inventory = self.inventory(&connection.client).await?;
        let area = inventory
            .configurations
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| invalid(format!("no entertainment area {id}")))?;
        if self
            .stream
            .lock()
            .await
            .as_ref()
            .is_some_and(|s| s.configuration == id)
        {
            return Err(invalid(
                "FOLO is streaming to this area: stop the zones that use it first",
            ));
        }
        Ok((area, inventory))
    }

    async fn update_area(
        &self,
        connection: &Connection,
        method: AdminMethod,
        body: &Value,
    ) -> AdminResult {
        let (area, inventory) = self.editable_area(connection, body).await?;
        let mut request = json!({});
        let mut changes = Vec::new();
        if let Some(name) = checked_name(body)? {
            changes.push(format!("renames it to {name:?}"));
            request["metadata"] = json!({"name": name});
        }
        if let Some(kind) = checked_type(body)? {
            changes.push(format!("makes it a {kind} area"));
            request["configuration_type"] = json!(kind);
        }
        if let Some(lights) = body.get("lights") {
            let locations = Self::service_locations(&inventory, lights)?;
            changes.push(format!(
                "places {} light(s); the bridge derives the channels again",
                locations.len()
            ));
            request["locations"] = json!({"service_locations": locations});
        }
        if changes.is_empty() {
            return Err(invalid(
                "nothing to change: give name, configuration_type or lights",
            ));
        }
        if method == AdminMethod::Read {
            let would = format!(
                "Changes the entertainment area {:?}: {}",
                area.metadata.name,
                changes.join(", ")
            );
            return Ok(preview(
                "entertainment_update",
                &would,
                &json!({"name": area.metadata.name, "configuration_type": area.configuration_type}),
                &request,
            ));
        }
        let path = format!("entertainment_configuration/{}", area.id);
        connection.client.put(&path, &request).await.map_err(rest)?;
        self.refresh_targets(&connection.client).await?;
        Ok(json!({"id": area.id, "changed": changes}))
    }

    async fn delete_area(
        &self,
        connection: &Connection,
        method: AdminMethod,
        body: &Value,
    ) -> AdminResult {
        let (area, _) = self.editable_area(connection, body).await?;
        if method == AdminMethod::Read {
            let would = format!(
                "Deletes the entertainment area {:?} ({} channels) from the bridge; the lights stay",
                area.metadata.name,
                area.channel_ids().len()
            );
            return Ok(preview(
                "entertainment_delete",
                &would,
                &json!({"id": area.id, "name": area.metadata.name}),
                &Value::Null,
            ));
        }
        connection
            .client
            .delete(&format!("entertainment_configuration/{}", area.id))
            .await
            .map_err(rest)?;
        self.refresh_targets(&connection.client).await?;
        Ok(json!({"id": area.id, "deleted": true}))
    }

    /// Stops another application's stream so a start can follow: the only way FOLO takes a
    /// stream over, and only because a person asked.
    async fn take_over(&self, connection: &Connection, method: AdminMethod) -> AdminResult {
        let inventory = self.inventory(&connection.client).await?;
        let me = connection.application_id.as_deref().unwrap_or_default();
        let others: Vec<&EntertainmentConfiguration> = inventory
            .configurations
            .iter()
            .filter(|a| a.streamed_by_other(me))
            .collect();
        let listed: Vec<Value> = others
            .iter()
            .map(|a| json!({"id": a.id, "name": a.metadata.name}))
            .collect();
        if method == AdminMethod::Read {
            let would = if others.is_empty() {
                "No other app is streaming; nothing would be stopped".to_owned()
            } else {
                "Stops the stream of another app, which ends its light effect; FOLO's targets then start on their next attempt".to_owned()
            };
            return Ok(preview(
                "entertainment_take_over",
                &would,
                &json!(listed),
                &json!([]),
            ));
        }
        for area in &others {
            self.try_stop_area(&connection.client, &area.id)
                .await
                .map_err(rest)?;
        }
        Ok(json!({"stopped": listed}))
    }

    async fn update_status(&self, connection: &Connection) -> AdminResult {
        let config = connection
            .client
            .authenticated_config()
            .await
            .map_err(rest)?;
        let update = config.get("swupdate2").cloned().unwrap_or(Value::Null);
        Ok(json!({
            "software_version": config.get("swversion"),
            "api_version": config.get("apiversion"),
            "update_state": update.get("state"),
            "bridge_update_state": update.pointer("/bridge/state"),
            "last_install": update.pointer("/bridge/lastinstall"),
            "auto_install": update.pointer("/autoinstall/on"),
        }))
    }
}

/// A room or zone with the names of its lights.
fn room_summary(kind: &str, item: &Value, inventory: &Inventory) -> Value {
    let children = item
        .get("children")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lights = Vec::new();
    for child in &children {
        let rid = text(child, "rid").unwrap_or_default();
        // A room lists devices and a zone lists lights
        let by_device = inventory.lights.iter().find(|l| l.owner.rid == rid);
        let by_light = inventory.lights.iter().find(|l| l.id == rid);
        if let Some(light) = by_device.or(by_light) {
            lights.push(Inventory::light_name(light));
        }
    }
    json!({
        "id": item.get("id"),
        "kind": kind,
        "name": item.pointer("/metadata/name"),
        "archetype": item.pointer("/metadata/archetype"),
        "lights": lights,
    })
}
