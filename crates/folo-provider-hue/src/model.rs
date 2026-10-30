//! What the bridge has, read from its CLIP v2 resources, and how that becomes targets
//! (`docs/integrations/hue.md`, FOLO provider design).
//!
//! An entertainment configuration (an *area*) has numbered channels, and each channel belongs to
//! an entertainment service, which is one light. A light with one channel is a **point** target;
//! a gradient light has several channels (one per segment) and is a **line** target with one
//! element per channel, ordered by member index. Everything is mapped by `channel_id`, never by
//! position in a list.

use std::collections::BTreeMap;

use folo_protocol::generated::{
    Address, CapabilityDescriptor, CapabilityDescriptorAddressing, CapabilityDescriptorColorModel,
    CapabilityDescriptorDelivery, CapabilityLimits, TargetKind,
};
use folo_provider::DescribedTarget;
use serde::Deserialize;
use serde_json::Value;

use crate::encoder::MAX_CHANNELS;

/// What a target of this provider streams at: the rate FOLO sends (`docs/integrations/hue.md`).
/// The bridge itself drives its lights at about half of it.
pub const MAX_RATE_HZ: f64 = 50.0;

/// A reference to another resource.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct Reference {
    #[serde(default)]
    pub rid: String,
    #[serde(default)]
    pub rtype: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub name: String,
}

/// One channel's member: a light's entertainment service and which of its segments.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub service: Reference,
    #[serde(default)]
    pub index: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Channel {
    pub channel_id: u8,
    #[serde(default)]
    pub members: Vec<Member>,
}

/// An entertainment configuration.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct EntertainmentConfiguration {
    pub id: String,
    #[serde(default)]
    pub metadata: Metadata,
    #[serde(default)]
    pub configuration_type: String,
    /// `active` while a stream runs.
    #[serde(default)]
    pub status: String,
    /// Who streams: the application id of the app that started it.
    #[serde(default)]
    pub active_streamer: Option<Reference>,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

impl EntertainmentConfiguration {
    /// Whether a stream runs on it.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    /// Whether the stream that runs on it belongs to another application than `application_id`.
    /// A stream whose owner the bridge doesn't name counts as another's.
    #[must_use]
    pub fn streamed_by_other(&self, application_id: &str) -> bool {
        self.is_active()
            && self
                .active_streamer
                .as_ref()
                .is_none_or(|s| !s.rid.eq_ignore_ascii_case(application_id))
    }

    /// The channel ids, ascending.
    #[must_use]
    pub fn channel_ids(&self) -> Vec<u8> {
        let mut ids: Vec<u8> = self.channels.iter().map(|c| c.channel_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// An entertainment service: the part of a device that takes streamed colours.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct EntertainmentService {
    pub id: String,
    #[serde(default)]
    pub owner: Reference,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Light {
    pub id: String,
    #[serde(default)]
    pub owner: Reference,
    #[serde(default)]
    pub metadata: Metadata,
    /// Present on gradient lights.
    #[serde(default)]
    pub gradient: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Device {
    pub id: String,
    #[serde(default)]
    pub metadata: Metadata,
}

/// Everything `describe` reads from a bridge.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub configurations: Vec<EntertainmentConfiguration>,
    pub services: Vec<EntertainmentService>,
    pub lights: Vec<Light>,
    pub devices: Vec<Device>,
}

fn parse_all<T: for<'de> Deserialize<'de>>(values: Vec<Value>) -> Vec<T> {
    // A resource this code can't read is left out; the others are still of use
    values
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}

impl Inventory {
    /// From the `data` lists of the four resources.
    #[must_use]
    pub fn from_resources(
        configurations: Vec<Value>,
        services: Vec<Value>,
        lights: Vec<Value>,
        devices: Vec<Value>,
    ) -> Self {
        Self {
            configurations: parse_all(configurations),
            services: parse_all(services),
            lights: parse_all(lights),
            devices: parse_all(devices),
        }
    }

    /// The name of the light behind an entertainment service: the light's own name, else its
    /// device's, else the service id.
    fn name_of(&self, service: &str) -> String {
        let owner = self
            .services
            .iter()
            .find(|s| s.id == service)
            .map(|s| s.owner.rid.as_str());
        let from_light = owner.and_then(|owner| {
            self.lights
                .iter()
                .find(|l| l.owner.rid == owner)
                .map(|l| l.metadata.name.as_str())
        });
        let from_device = owner.and_then(|owner| {
            self.devices
                .iter()
                .find(|d| d.id == owner)
                .map(|d| d.metadata.name.as_str())
        });
        [from_light, from_device]
            .into_iter()
            .flatten()
            .find(|n| !n.is_empty())
            .map_or_else(|| service.to_owned(), str::to_owned)
    }

    /// The targets of every configuration, and how each maps to channels.
    #[must_use]
    pub fn targets(&self) -> Vec<HueTarget> {
        let mut out = Vec::new();
        for configuration in &self.configurations {
            out.extend(self.targets_of(configuration));
        }
        out
    }

    fn targets_of(&self, configuration: &EntertainmentConfiguration) -> Vec<HueTarget> {
        // service id -> (member index, channel id), in the order channels appear
        let mut by_service: BTreeMap<&str, Vec<(u32, u8)>> = BTreeMap::new();
        for channel in &configuration.channels {
            let Some(member) = channel.members.first() else {
                continue;
            };
            by_service
                .entry(member.service.rid.as_str())
                .or_default()
                .push((member.index, channel.channel_id));
        }
        by_service
            .into_iter()
            .map(|(service, mut members)| {
                members.sort_unstable();
                HueTarget {
                    id: target_id(&configuration.id, service),
                    configuration: configuration.id.clone(),
                    name: self.name_of(service),
                    channels: members.into_iter().map(|(_, channel)| channel).collect(),
                }
            })
            .collect()
    }
}

/// A target's id on the provider: the configuration, a slash and the light's entertainment
/// service. The same light is a different target in each configuration it is part of, because
/// each has its own channel for it.
#[must_use]
pub fn target_id(configuration: &str, service: &str) -> String {
    format!("{configuration}/{service}")
}

/// A light (or a gradient light) of an entertainment configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HueTarget {
    pub id: String,
    /// The configuration to stream to for this target.
    pub configuration: String,
    pub name: String,
    /// The channel ids of its elements, in element order (member index for a gradient light).
    pub channels: Vec<u8>,
}

impl HueTarget {
    /// A point for one channel, a line for several.
    #[must_use]
    pub fn kind(&self) -> TargetKind {
        if self.channels.len() > 1 {
            TargetKind::Line
        } else {
            TargetKind::Point
        }
    }

    /// What `describe` reports.
    #[must_use]
    pub fn described(&self) -> DescribedTarget {
        let elements = self.channels.len().clamp(1, MAX_CHANNELS);
        let capabilities = CapabilityDescriptor {
            addressing: if self.channels.len() > 1 {
                CapabilityDescriptorAddressing::Zoned
            } else {
                CapabilityDescriptorAddressing::Single
            },
            color_model: CapabilityDescriptorColorModel::Rgb,
            delivery: CapabilityDescriptorDelivery::Streaming,
            limits: CapabilityLimits {
                max_rate_hz: MAX_RATE_HZ,
                ..CapabilityLimits::default()
            },
        };
        DescribedTarget {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind(),
            address: Address {
                channels: self.channels.clone(),
                count: std::num::NonZeroU64::new(elements as u64)
                    .unwrap_or(std::num::NonZeroU64::MIN),
                start: 0,
            },
            elements: u32::try_from(elements).unwrap_or(1),
            capabilities,
            layouts: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn inventory() -> Inventory {
        let configuration = json!({
            "id": "area-1", "metadata": {"name": "TV"}, "configuration_type": "screen",
            "status": "inactive",
            // Channels deliberately out of order: the mapping is by channel id and member index
            "channels": [
                {"channel_id": 3, "members": [{"service": {"rid": "svc-strip", "rtype": "entertainment"}, "index": 2}]},
                {"channel_id": 0, "members": [{"service": {"rid": "svc-lamp", "rtype": "entertainment"}, "index": 0}]},
                {"channel_id": 2, "members": [{"service": {"rid": "svc-strip", "rtype": "entertainment"}, "index": 0}]},
                {"channel_id": 1, "members": [{"service": {"rid": "svc-strip", "rtype": "entertainment"}, "index": 1}]},
                {"channel_id": 4, "members": []},
            ],
        });
        Inventory::from_resources(
            vec![configuration],
            vec![
                json!({"id": "svc-lamp", "owner": {"rid": "dev-lamp", "rtype": "device"}}),
                json!({"id": "svc-strip", "owner": {"rid": "dev-strip", "rtype": "device"}}),
            ],
            vec![
                json!({"id": "light-lamp", "owner": {"rid": "dev-lamp"}, "metadata": {"name": "Desk lamp"}}),
            ],
            vec![
                json!({"id": "dev-lamp", "metadata": {"name": "Hue go"}}),
                json!({"id": "dev-strip", "metadata": {"name": "Gradient strip"}}),
            ],
        )
    }

    #[test]
    fn single_channel_lights_are_points_and_gradients_are_lines_ordered_by_member_index() {
        let targets = inventory().targets();
        assert_eq!(targets.len(), 2);
        let strip = targets.iter().find(|t| t.id == "area-1/svc-strip").unwrap();
        assert_eq!(strip.channels, vec![2, 1, 3]);
        assert_eq!(strip.kind(), TargetKind::Line);
        assert_eq!(strip.name, "Gradient strip");
        let lamp = targets.iter().find(|t| t.id == "area-1/svc-lamp").unwrap();
        assert_eq!(lamp.channels, vec![0]);
        assert_eq!(lamp.kind(), TargetKind::Point);
        assert_eq!(lamp.name, "Desk lamp");
    }

    #[test]
    fn described_targets_carry_channels_and_streaming_capabilities() {
        let described = inventory().targets()[0].described();
        assert_eq!(
            described.address.channels.len(),
            described.elements as usize
        );
        assert_eq!(
            described.capabilities.delivery,
            CapabilityDescriptorDelivery::Streaming
        );
        assert_eq!(
            described.capabilities.color_model,
            CapabilityDescriptorColorModel::Rgb
        );
        assert!((described.capabilities.limits.max_rate_hz - MAX_RATE_HZ).abs() < f64::EPSILON);
    }

    #[test]
    fn ownership_compares_the_streamer_with_our_application_id() {
        let mut configuration = inventory().configurations[0].clone();
        assert!(!configuration.streamed_by_other("mine"));
        configuration.status = "active".into();
        assert!(
            configuration.streamed_by_other("mine"),
            "an unnamed owner is another's"
        );
        configuration.active_streamer = Some(Reference {
            rid: "MINE".into(),
            rtype: "auth_v1".into(),
        });
        assert!(!configuration.streamed_by_other("mine"));
        configuration.active_streamer = Some(Reference {
            rid: "theirs".into(),
            rtype: "auth_v1".into(),
        });
        assert!(configuration.streamed_by_other("mine"));
    }

    #[test]
    fn channel_ids_are_sorted_and_unique() {
        assert_eq!(
            inventory().configurations[0].channel_ids(),
            vec![0, 1, 2, 3, 4]
        );
    }
}
