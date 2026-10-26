//! The data that crosses the provider interface.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use folo_protocol::generated::{
    Address, CapabilityDescriptor, CapabilityDescriptorColorModel, CapabilityDescriptorDelivery,
    Geometry, ProviderLiveState, ProviderStatusState, TargetKind,
};

/// What the asynchronous trait methods return, so the traits stay usable as `dyn`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// An RGB+CCT element: the colour channels, the white channel's level and its colour temperature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RgbCct {
    /// Red, green and blue, 0–255.
    pub rgb: [f32; 3],
    /// The CCT channel's level, 0–255.
    pub white: f32,
    /// The CCT channel's colour temperature in kelvin.
    pub kelvin: f32,
}

/// A tunable-white element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cct {
    /// Level, 0–255.
    pub brightness: f32,
    /// Colour temperature in kelvin.
    pub kelvin: f32,
}

/// An element in CIE xy, with its brightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xy {
    pub x: f32,
    pub y: f32,
    /// Level, 0–255.
    pub brightness: f32,
}

/// One colour per element, **in the target's colour model**. Levels are on the effect engine's
/// 0–255 scale as `f32`; a provider truncates them to the bytes its device takes, which keeps
/// parity byte-exact (`docs/architecture/pipeline.md`, parity mode).
#[derive(Clone, Debug, PartialEq)]
pub enum ElementColors {
    Rgb(Vec<[f32; 3]>),
    /// Red, green, blue and white.
    Rgbw(Vec<[f32; 4]>),
    RgbCct(Vec<RgbCct>),
    Cct(Vec<Cct>),
    Dimmer(Vec<f32>),
    OnOff(Vec<bool>),
    Xy(Vec<Xy>),
}

/// A level on the 0–255 scale as the byte a device takes: truncated toward zero, saturating.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn byte(level: f32) -> u8 {
    level as u8
}

/// A unit-range value as 16 bits (xy coordinates), truncated toward zero, saturating.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn word(value: f32) -> u16 {
    (value * 65535.0) as u16
}

/// A temperature as whole kelvin, saturating.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn kelvin(value: f32) -> u16 {
    value as u16
}

impl ElementColors {
    /// How many elements.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Rgb(v) => v.len(),
            Self::Rgbw(v) => v.len(),
            Self::RgbCct(v) => v.len(),
            Self::Cct(v) => v.len(),
            Self::Dimmer(v) => v.len(),
            Self::OnOff(v) => v.len(),
            Self::Xy(v) => v.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The colour model these colours are in.
    #[must_use]
    pub fn color_model(&self) -> CapabilityDescriptorColorModel {
        match self {
            Self::Rgb(_) => CapabilityDescriptorColorModel::Rgb,
            Self::Rgbw(_) => CapabilityDescriptorColorModel::Rgbw,
            Self::RgbCct(_) => CapabilityDescriptorColorModel::Rgbcct,
            Self::Cct(_) => CapabilityDescriptorColorModel::Cct,
            Self::Dimmer(_) => CapabilityDescriptorColorModel::Dimmer,
            Self::OnOff(_) => CapabilityDescriptorColorModel::Onoff,
            Self::Xy(_) => CapabilityDescriptorColorModel::Xy,
        }
    }

    /// The colours as bytes, levels truncated toward zero: a provider whose device takes plain
    /// 8-bit levels can use this as its [wire key](crate::ElementOutput::wire_key).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Rgb(v) => v.iter().flatten().for_each(|c| out.push(byte(*c))),
            Self::Rgbw(v) => v.iter().flatten().for_each(|c| out.push(byte(*c))),
            Self::RgbCct(v) => {
                for e in v {
                    e.rgb.iter().for_each(|c| out.push(byte(*c)));
                    out.push(byte(e.white));
                    out.extend(kelvin(e.kelvin).to_le_bytes());
                }
            }
            Self::Cct(v) => {
                for e in v {
                    out.push(byte(e.brightness));
                    out.extend(kelvin(e.kelvin).to_le_bytes());
                }
            }
            Self::Dimmer(v) => v.iter().for_each(|c| out.push(byte(*c))),
            Self::OnOff(v) => v.iter().for_each(|on| out.push(u8::from(*on))),
            Self::Xy(v) => {
                for e in v {
                    out.extend(word(e.x).to_le_bytes());
                    out.extend(word(e.y).to_le_bytes());
                    out.push(byte(e.brightness));
                }
            }
        }
        out
    }
}

/// The colours of one target's elements.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetOutput {
    /// The target's id on the provider, as [`DescribedTarget::id`] gives it.
    pub target: String,
    pub colors: ElementColors,
}

/// What a provider is sent: the colours of some of an instance's targets for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementFrame {
    /// The controller's monotonic clock, in microseconds.
    pub timestamp_us: u64,
    /// For state targets, how long the device should take to reach these colours: the update
    /// interval. Zero when the frame holds only streaming targets; in a frame that mixes the
    /// two it is the state targets' interval, and a streaming provider ignores it.
    pub transition: Duration,
    pub targets: Vec<TargetOutput>,
}

impl ElementFrame {
    #[must_use]
    pub fn new(timestamp_us: u64, targets: Vec<TargetOutput>) -> Self {
        Self {
            timestamp_us,
            transition: Duration::ZERO,
            targets,
        }
    }
}

/// An instance's state, which the node maps to the client API's and the live channel's lists
/// (`docs/architecture/providers.md`, states).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderState {
    /// Disabled, or a kind without an implementation.
    Stopped,
    /// Starting, or between a clean close and the next attempt.
    Connecting,
    /// The control connection is up.
    Connected,
    /// The last attempt, start or send failed; attempts continue with backoff.
    Error,
}

impl ProviderState {
    /// The client API's `ProviderStatus.state`.
    #[must_use]
    pub fn api(self) -> ProviderStatusState {
        match self {
            Self::Stopped => ProviderStatusState::Stopped,
            Self::Connecting => ProviderStatusState::Connecting,
            Self::Connected => ProviderStatusState::Connected,
            Self::Error => ProviderStatusState::Error,
        }
    }

    /// The live channel's `ProviderLive.state`.
    #[must_use]
    pub fn live(self) -> ProviderLiveState {
        match self {
            Self::Stopped => ProviderLiveState::Off,
            Self::Connecting => ProviderLiveState::Disconnected,
            Self::Connected => ProviderLiveState::Connected,
            Self::Error => ProviderLiveState::Error,
        }
    }
}

/// An instance's state and what to tell about it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderStatus {
    pub state: ProviderState,
    /// Set in [`ProviderState::Error`], and for a kind that isn't implemented yet.
    pub message: Option<String>,
}

impl ProviderStatus {
    #[must_use]
    pub fn new(state: ProviderState) -> Self {
        Self {
            state,
            message: None,
        }
    }
}

/// Whether a target is sent frames at a rate or commands at a limited pace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// Frames at the instance's rate, changed ones only, plus a keepalive.
    Streaming,
    /// Rate-limited updates, sent only on a visible change, from a token bucket.
    State,
}

impl From<CapabilityDescriptorDelivery> for Delivery {
    fn from(value: CapabilityDescriptorDelivery) -> Self {
        match value {
            CapabilityDescriptorDelivery::Streaming => Self::Streaming,
            CapabilityDescriptorDelivery::State => Self::State,
        }
    }
}

/// A token bucket: `capacity` tokens at most, refilled `per_second`. It starts full.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BucketSpec {
    pub capacity: u32,
    pub per_second: f64,
}

/// What a kind needs of the sender, per instance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeliveryPolicy {
    /// An unchanged streaming target is sent again after this long at the latest, inside the
    /// device's timeout (Hyperion 0.5 s, WLED 1 s, Hue 5 s). The sender checks once a tick, so it
    /// goes out up to one tick later.
    pub keepalive: Duration,
    /// Paces the updates of state targets: one token per target update, shared by the instance
    /// (a bridge or a device).
    pub bucket: BucketSpec,
    /// How long one call of the provider (`connect`, `describe`, `start`, `send`, a release
    /// while running) may take. A call that takes longer counts as `Unreachable`: the instance
    /// reconnects. Hue's `start` (the stream and its handshake) is the slow one.
    pub call_timeout: Duration,
    /// How long a release, or closing the connection, may take when the sender stops. Stopping
    /// as a whole takes at most three times this.
    pub release_timeout: Duration,
}

impl Default for DeliveryPolicy {
    /// One second, and five updates a second with a burst of five, calls limited to 10 s and
    /// releases to 2 s: placeholders a kind replaces.
    fn default() -> Self {
        Self {
            keepalive: Duration::from_secs(1),
            bucket: BucketSpec {
                capacity: 5,
                per_second: 5.0,
            },
            call_timeout: Duration::from_secs(10),
            release_timeout: Duration::from_secs(2),
        }
    }
}

/// A layout a device offers for a target, which the zone editor can offer and nothing writes by
/// itself (Hyperion's LED rectangles on the TV plane, WLED's segments).
#[derive(Clone, Debug)]
pub struct DescribedLayout {
    pub name: String,
    pub geometry: Geometry,
}

/// A target the device has, as `describe` finds it.
#[derive(Clone, Debug)]
pub struct DescribedTarget {
    /// The provider's stable id for the target, which frames name it by.
    pub id: String,
    pub name: String,
    pub kind: TargetKind,
    /// Where its elements are on the provider.
    pub address: Address,
    pub elements: u32,
    pub capabilities: CapabilityDescriptor,
    pub layouts: Vec<DescribedLayout>,
}
