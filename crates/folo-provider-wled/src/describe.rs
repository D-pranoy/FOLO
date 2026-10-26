//! What a device offers as targets (`docs/integrations/wled.md`, FOLO provider design).
//!
//! - the strip as a line target, over the digital buses' LEDs
//! - **each segment** as a sub-target, by absolute LED range, with a warning for grouping or
//!   spacing
//! - a 2D matrix as a grid target (its segments are regions, not LED ranges, so they aren't
//!   offered)
//! - each PWM or relay output as a point target in the colour model of its bus type
//! - with "use main segment only" on, the main segment alone: realtime data reaches nothing else

use folo_protocol::generated::{
    Address, CapabilityDescriptor, CapabilityDescriptorAddressing,
    CapabilityDescriptorColorModel as ColorModel, Geometry, TargetKind,
};
use folo_provider::{DescribedLayout, DescribedTarget};
use serde_json::json;

use crate::device::{Bus, BusModel, DMX_ADDRESS_DEFAULT, Device, LC_CCT, LC_RGB, LC_WHITE};

/// How often WLED renders at most (every 15 ms, about 66 a second).
const MAX_RATE_HZ: f64 = 66.0;

/// Ids of the targets this provider describes.
pub const STRIP_ID: &str = "strip";
pub const MATRIX_ID: &str = "matrix";

#[must_use]
pub fn segment_id(id: u32) -> String {
    format!("seg:{id}")
}

#[must_use]
pub fn bus_id(index: usize) -> String {
    format!("bus:{index}")
}

/// A target and what sending to it needs.
#[derive(Clone, Debug, PartialEq)]
pub struct WledTarget {
    pub id: String,
    pub name: String,
    pub kind: TargetKind,
    /// The absolute index of its first LED.
    pub first_pixel: u32,
    pub count: u32,
    pub color_model: ColorModel,
    pub addressing: CapabilityDescriptorAddressing,
    /// Whether packets for it carry a white byte: the LEDs have white and the device's
    /// auto-white mode honours ours.
    pub send_white: bool,
    pub grid: Option<(u32, u32)>,
}

/// The targets of a device, and what to tell a person about them.
#[derive(Clone, Debug, PartialEq)]
pub struct Description {
    pub targets: Vec<WledTarget>,
    pub warnings: Vec<String>,
    /// The index realtime data starts at: the main segment's first LED when the device uses
    /// the main segment only, else 0.
    pub base_pixel: u32,
}

/// The colour model of LEDs by their capabilities. CCT can't be set by realtime data (WLED
/// uses the segment's own value), so a CCT target's colour temperature is dropped on the way.
fn model_of(lc: u8, send_white: bool) -> ColorModel {
    let rgb = lc & LC_RGB != 0;
    let cct = lc & LC_CCT != 0;
    let white = lc & LC_WHITE != 0;
    match (rgb, cct, white) {
        (true, true, _) => ColorModel::Rgbcct,
        (true, false, true) if send_white => ColorModel::Rgbw,
        (true, ..) => ColorModel::Rgb,
        (false, true, _) => ColorModel::Cct,
        (false, false, true) => ColorModel::Dimmer,
        (false, false, false) => ColorModel::Onoff,
    }
}

fn model_of_bus(model: BusModel) -> ColorModel {
    match model {
        BusModel::OnOff => ColorModel::Onoff,
        BusModel::Dimmer => ColorModel::Dimmer,
        BusModel::Cct => ColorModel::Cct,
        BusModel::Rgb => ColorModel::Rgb,
        BusModel::Rgbw => ColorModel::Rgbw,
        BusModel::RgbCct => ColorModel::Rgbcct,
    }
}

fn has_white_channel(lc: u8) -> bool {
    lc & (LC_WHITE | LC_CCT) != 0
}

fn overlapping(buses: &[Bus], start: u32, stop: u32) -> Vec<&Bus> {
    buses
        .iter()
        .filter(|b| !b.is_point() && b.start < stop && b.start + b.len > start)
        .collect()
}

impl Description {
    /// The targets and warnings of a device as it was just read.
    #[must_use]
    pub fn of(device: &Device) -> Self {
        let mut out = Self {
            targets: Vec::new(),
            warnings: Vec::new(),
            base_pixel: 0,
        };
        out.settings_warnings(device);
        if let Some(main) = device.live_segment {
            out.main_segment_only(device, main);
        } else if let Some((width, height)) = device.matrix {
            out.matrix(device, width, height);
            out.points(device);
        } else {
            // A device with only PWM or relay outputs has no strip to cut into segments
            if out.strip(device) {
                out.segments(device);
            }
            out.points(device);
        }
        out
    }

    fn settings_warnings(&mut self, device: &Device) {
        let Some(realtime) = &device.realtime else {
            self.warnings.push(
                "The device's settings can't be read (is a PIN set?), so its LED outputs, \
                 auto-white mode and realtime settings are unknown."
                    .into(),
            );
            return;
        };
        if realtime.dmx_address != DMX_ADDRESS_DEFAULT {
            self.warnings.push(format!(
                "The DMX start address is {}, not {DMX_ADDRESS_DEFAULT}: it shifts the LEDs \
                 DDP writes to. Set it back to {DMX_ADDRESS_DEFAULT} on the device.",
                realtime.dmx_address
            ));
        }
    }

    fn target_colors(device: &Device, lc: u8, buses: &[&Bus]) -> (ColorModel, bool) {
        let send_white = has_white_channel(lc) && device.honours_white(buses);
        (model_of(lc, send_white), send_white)
    }

    /// Adds the strip; whether there is one.
    fn strip(&mut self, device: &Device) -> bool {
        let (first, end, buses): (u32, u32, Vec<&Bus>) = match &device.buses {
            Some(all) => {
                let digital: Vec<&Bus> = all.iter().filter(|b| !b.is_point()).collect();
                let first = digital.iter().map(|b| b.start).min().unwrap_or(0);
                let end = digital.iter().map(|b| b.start + b.len).max().unwrap_or(0);
                (first, end, digital)
            }
            None => (0, device.led_count, Vec::new()),
        };
        if end <= first {
            return false;
        }
        let (color_model, send_white) = Self::target_colors(device, device.lc, &buses);
        self.targets.push(WledTarget {
            id: STRIP_ID.into(),
            name: if device.name.is_empty() {
                "WLED".into()
            } else {
                device.name.clone()
            },
            kind: TargetKind::Line,
            first_pixel: first,
            count: end - first,
            color_model,
            addressing: CapabilityDescriptorAddressing::Pixel,
            send_white,
            grid: None,
        });
        true
    }

    fn segments(&mut self, device: &Device) {
        for segment in &device.segments {
            self.segment(device, segment);
        }
    }

    fn segment(&mut self, device: &Device, segment: &crate::device::Segment) {
        if segment.grouping > 1 || segment.spacing > 0 {
            self.warnings.push(format!(
                "Segment {} groups {} LED(s) and leaves {} between groups: realtime data \
                 addresses physical LEDs, not the segment's groups.",
                segment.id, segment.grouping, segment.spacing
            ));
        }
        let buses = device
            .buses
            .as_deref()
            .map(|all| overlapping(all, segment.start, segment.stop))
            .unwrap_or_default();
        let (color_model, send_white) = Self::target_colors(device, segment.lc, &buses);
        self.targets.push(WledTarget {
            id: segment_id(segment.id),
            name: if segment.name.is_empty() {
                format!("Segment {}", segment.id)
            } else {
                segment.name.clone()
            },
            kind: TargetKind::Line,
            first_pixel: segment.start,
            count: segment.len(),
            color_model,
            addressing: CapabilityDescriptorAddressing::Pixel,
            send_white,
            grid: None,
        });
    }

    fn main_segment_only(&mut self, device: &Device, main: u32) {
        let Some(segment) = device.segment(main) else {
            self.warnings.push(format!(
                "The device uses its main segment only, but segment {main} isn't in its state."
            ));
            return;
        };
        self.warnings.push(format!(
            "The device uses its main segment only (segment {main}, LEDs {}–{}): that is all \
             FOLO can drive, and its other segments keep their own effects.",
            segment.start,
            segment.stop - 1
        ));
        self.base_pixel = segment.start;
        self.segment(device, segment);
    }

    fn matrix(&mut self, device: &Device, width: u32, height: u32) {
        let (color_model, send_white) = Self::target_colors(device, device.lc, &[]);
        self.targets.push(WledTarget {
            id: MATRIX_ID.into(),
            name: if device.name.is_empty() {
                "WLED matrix".into()
            } else {
                device.name.clone()
            },
            kind: TargetKind::Grid,
            first_pixel: 0,
            count: width * height,
            color_model,
            addressing: CapabilityDescriptorAddressing::Pixel,
            send_white,
            grid: Some((width, height)),
        });
    }

    fn points(&mut self, device: &Device) {
        let Some(buses) = &device.buses else { return };
        for (index, bus) in buses.iter().enumerate().filter(|(_, b)| b.is_point()) {
            let model = bus.point_model();
            self.targets.push(WledTarget {
                id: bus_id(index),
                name: format!("Output {}", index + 1),
                kind: TargetKind::Point,
                first_pixel: bus.start,
                count: 1,
                color_model: model_of_bus(model),
                addressing: CapabilityDescriptorAddressing::Single,
                send_white: bus.has_white() && device.honours_white(&[bus]),
                grid: None,
            });
        }
    }

    /// The target with this id.
    #[must_use]
    pub fn target(&self, id: &str) -> Option<&WledTarget> {
        self.targets.iter().find(|t| t.id == id)
    }

    /// The targets in the form the provider interface describes them.
    #[must_use]
    pub fn described(&self) -> Vec<DescribedTarget> {
        self.targets.iter().map(WledTarget::described).collect()
    }
}

impl WledTarget {
    fn geometry(&self) -> Geometry {
        let value = match (self.kind, self.grid) {
            (TargetKind::Grid, Some((width, height))) => {
                json!({"kind": "grid", "width": width, "height": height})
            }
            (TargetKind::Point, _) => json!({"kind": "point"}),
            _ => json!({"kind": "line", "count": self.count}),
        };
        serde_json::from_value(value).unwrap_or_default()
    }

    fn descriptor(&self) -> CapabilityDescriptor {
        serde_json::from_value(json!({
            "color_model": self.color_model,
            "addressing": self.addressing,
            "delivery": "streaming",
            "limits": {"max_rate_hz": MAX_RATE_HZ},
        }))
        .unwrap_or_default()
    }

    fn address(&self) -> Address {
        serde_json::from_value(json!({
            "start": self.first_pixel.min(u32::from(u16::MAX)),
            "count": self.count.clamp(1, u32::from(u16::MAX)),
        }))
        .unwrap_or_default()
    }

    /// As the provider interface describes it. A grid target offers its shape as a layout, so
    /// the zone editor can place it.
    #[must_use]
    pub fn described(&self) -> DescribedTarget {
        let layouts = match self.grid {
            Some(_) => vec![DescribedLayout {
                name: self.name.clone(),
                geometry: self.geometry(),
            }],
            None => Vec::new(),
        };
        DescribedTarget {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind,
            address: self.address(),
            elements: self.count,
            capabilities: self.descriptor(),
            layouts,
        }
    }
}
