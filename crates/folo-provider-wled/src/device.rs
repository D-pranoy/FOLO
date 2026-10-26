//! What the device reports, read leniently from `/json/info`, `/json/state` and `/json/cfg`
//! (`docs/integrations/wled.md`, capabilities). A field the device doesn't send takes the value
//! WLED itself would assume.

use serde_json::Value;

/// `info.leds.lc` bits.
pub const LC_RGB: u8 = 0x01;
pub const LC_WHITE: u8 = 0x02;
pub const LC_CCT: u8 = 0x04;

/// Auto-white modes (`rgbwm`) that send our white channel to the LEDs: Manual and Dual. The
/// others derive white from RGB.
const AUTO_WHITE_MANUAL: u8 = 0;
const AUTO_WHITE_DUAL: u8 = 3;
/// A global `rgbwm` of this value means "use each bus's own".
const AUTO_WHITE_PER_BUS: u8 = 255;

/// The DMX start address WLED has by default.
pub const DMX_ADDRESS_DEFAULT: u32 = 1;

fn unsigned(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn small(value: &Value, key: &str, default: u8) -> u8 {
    unsigned(value, key)
        .and_then(|n| u8::try_from(n).ok())
        .unwrap_or(default)
}

fn count(value: &Value, key: &str, default: u32) -> u32 {
    unsigned(value, key)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(default)
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A segment of `/json/state`.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub id: u32,
    pub name: String,
    pub start: u32,
    /// Exclusive.
    pub stop: u32,
    pub grouping: u32,
    pub spacing: u32,
    pub lc: u8,
    pub on: bool,
    pub brightness: u8,
}

impl Segment {
    /// The LEDs the segment spans (its `stop` is exclusive).
    #[must_use]
    pub fn len(&self) -> u32 {
        self.stop.saturating_sub(self.start)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn from_json(value: &Value) -> Option<Self> {
        let id = u32::try_from(unsigned(value, "id")?).ok()?;
        let start = count(value, "start", 0);
        let stop = count(value, "stop", 0);
        // A free slot is reported with no LEDs
        (stop > start).then(|| Self {
            id,
            name: text(value, "n"),
            start,
            stop,
            grouping: count(value, "grp", 1),
            spacing: count(value, "spc", 0),
            lc: small(value, "lc", LC_RGB),
            on: flag(value, "on"),
            brightness: small(value, "bri", 255),
        })
    }
}

/// An output of `/json/cfg` → `hw.led.ins[]`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bus {
    pub start: u32,
    pub len: u32,
    /// The bus type without its refresh flag (bit 7).
    pub kind: u8,
    pub auto_white: u8,
}

/// The colour model of a bus type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusModel {
    OnOff,
    Dimmer,
    Cct,
    Rgb,
    Rgbw,
    RgbCct,
}

impl Bus {
    fn from_json(value: &Value) -> Self {
        Self {
            start: count(value, "start", 0),
            len: count(value, "len", 1),
            kind: small(value, "type", 22) & 0x7F,
            auto_white: small(value, "rgbwm", AUTO_WHITE_MANUAL),
        }
    }

    /// PWM and relay outputs (types 40–47): one LED each, whatever `len` says.
    #[must_use]
    pub fn is_point(&self) -> bool {
        (40..=47).contains(&self.kind)
    }

    /// The colour model of a PWM or relay bus, from its type: 40 on/off, 41 white, 42 white and
    /// CCT, 43 RGB, 44 RGBW, 45 and 46 RGB with CCT.
    #[must_use]
    pub fn point_model(&self) -> BusModel {
        match self.kind {
            40 => BusModel::OnOff,
            41 => BusModel::Dimmer,
            42 => BusModel::Cct,
            43 => BusModel::Rgb,
            44 => BusModel::Rgbw,
            _ => BusModel::RgbCct,
        }
    }

    /// Whether the bus has a white LED: the digital types with one (the white-only chips, WWA,
    /// FW1906, UCS8904, SK6812, TM1814, WS2805, SM16825) and the analog types 41, 42, 44–46.
    #[must_use]
    pub fn has_white(&self) -> bool {
        matches!(self.kind, 18..=19 | 21 | 28..=32 | 34 | 41 | 42 | 44..=46)
    }
}

/// `/json/cfg` → `if.live`, the realtime receive settings.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealtimeSettings {
    /// "Receive UDP realtime" (`en`).
    pub receive: bool,
    /// "Use main segment only" (`mso`).
    pub main_segment_only: bool,
    /// The realtime timeout in tenths of a second (`timeout`).
    pub timeout_ds: u32,
    /// "Force max brightness" (`maxbri`).
    pub force_max_brightness: bool,
    /// "Disable realtime gamma correction" (`no-gc`).
    pub disable_gamma: bool,
    pub dmx_address: u32,
}

impl RealtimeSettings {
    fn from_json(value: &Value) -> Self {
        Self {
            receive: flag(value, "en"),
            main_segment_only: flag(value, "mso"),
            timeout_ds: count(value, "timeout", 25),
            force_max_brightness: flag(value, "maxbri"),
            disable_gamma: flag(value, "no-gc"),
            dmx_address: value.get("dmx").map_or(DMX_ADDRESS_DEFAULT, |d| {
                count(d, "addr", DMX_ADDRESS_DEFAULT)
            }),
        }
    }
}

/// Everything the provider reads of a device.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub version: String,
    pub mac: String,
    /// The highest bus end (`info.leds.count`), not a sum.
    pub led_count: u32,
    /// `info.leds.matrix`, on 2D setups.
    pub matrix: Option<(u32, u32)>,
    pub lc: u8,
    pub max_segments: Option<u32>,
    /// `info.liveseg`: the main segment's id when "use main segment only" is on.
    pub live_segment: Option<u32>,
    pub udp_port: Option<u16>,
    pub on: bool,
    pub brightness: u8,
    pub segments: Vec<Segment>,
    /// `None` when `/json/cfg` couldn't be read (the PIN).
    pub buses: Option<Vec<Bus>>,
    /// `hw.led.rgbwm`: 255 when each bus has its own.
    pub global_auto_white: u8,
    pub realtime: Option<RealtimeSettings>,
}

impl Device {
    /// Reads the three documents; `cfg` is `None` when the device wouldn't give it.
    #[must_use]
    pub fn from_json(info: &Value, state: &Value, cfg: Option<&Value>) -> Self {
        let leds = info.get("leds").cloned().unwrap_or(Value::Null);
        let matrix = leds.get("matrix").and_then(|m| {
            let (w, h) = (count(m, "w", 0), count(m, "h", 0));
            (w > 0 && h > 0).then_some((w, h))
        });
        let live_segment = info
            .get("liveseg")
            .and_then(Value::as_i64)
            .and_then(|n| u32::try_from(n).ok());
        let segments = state
            .get("seg")
            .and_then(Value::as_array)
            .map(|all| all.iter().filter_map(Segment::from_json).collect())
            .unwrap_or_default();
        let led_cfg = cfg.and_then(|c| c.get("hw")).and_then(|h| h.get("led"));
        Self {
            name: text(info, "name"),
            version: text(info, "ver"),
            mac: text(info, "mac"),
            led_count: count(&leds, "count", 0),
            matrix,
            lc: small(&leds, "lc", LC_RGB),
            max_segments: unsigned(&leds, "maxseg").and_then(|n| u32::try_from(n).ok()),
            live_segment,
            udp_port: unsigned(info, "udpport").and_then(|n| u16::try_from(n).ok()),
            on: flag(state, "on"),
            brightness: small(state, "bri", 128),
            segments,
            buses: led_cfg
                .and_then(|l| l.get("ins"))
                .and_then(Value::as_array)
                .map(|all| all.iter().map(Bus::from_json).collect()),
            global_auto_white: led_cfg.map_or(AUTO_WHITE_PER_BUS, |l| {
                small(l, "rgbwm", AUTO_WHITE_PER_BUS)
            }),
            realtime: cfg
                .and_then(|c| c.get("if"))
                .and_then(|i| i.get("live"))
                .map(RealtimeSettings::from_json),
        }
    }

    /// Whether the white channel of FOLO's RGBW frames reaches the LEDs of `buses`: the global
    /// auto-white mode, or each bus's own, is Manual or Dual.
    #[must_use]
    pub fn honours_white(&self, buses: &[&Bus]) -> bool {
        // Without the settings the mode is unknown, and RGB is right in every mode
        if self.buses.is_none() {
            return false;
        }
        let honoured = |mode: u8| mode == AUTO_WHITE_MANUAL || mode == AUTO_WHITE_DUAL;
        if self.global_auto_white != AUTO_WHITE_PER_BUS {
            return honoured(self.global_auto_white);
        }
        buses
            .iter()
            .filter(|b| b.has_white())
            .all(|b| honoured(b.auto_white))
    }

    /// The segment with this id.
    #[must_use]
    pub fn segment(&self, id: u32) -> Option<&Segment> {
        self.segments.iter().find(|s| s.id == id)
    }
}
