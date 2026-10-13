# WLED

Status: Accepted · Implementation: Not started

Decided in [ADR-0012](../decisions/ADR-0012-wled-provider.md). Facts come from the WLED source (v16.0.1 stable and main, `wled00/`) and the WLED documentation.

## Capabilities

**`GET /json/info` → `leds`:**

| Field | Meaning |
|---|---|
| `count` | The highest bus end (start + length) over all buses, not a sum |
| `matrix {w, h}` | Present only on 2D setups |
| `lc` | The OR of all active segments' capabilities. Bits: `0x01` RGB, `0x02` white channel, `0x04` CCT. `lc = 0` means on/off only. |
| `seglc[]` | Deprecated. One entry per active segment, so positions aren't segment ids. |
| `wv`, `cct` | `lc & 0x02` and `lc & 0x04`. They are **numbers, not booleans**. |

Other `info` fields: `udpport`, `live`, `liveseg` (the main segment id when "use main segment only" is on, else −1), `lm`, `lip`.

**Bus types** come from `GET /json/cfg` → `hw.led.ins[].type` (it may need the device PIN):
- 40 on/off
- 41 white only
- 42 white + CCT
- 43 RGB
- 44 RGBW
- 45 RGB + CCT

PWM and relay outputs count as **one LED** each.

**Segments** (`GET /json/state` → `seg[]`): `id`, `start`, `stop` (exclusive), `len`, `grp`, `spc`, `of`, `on`, `bri` (opacity), `cct`, `lc`, and more.

## Realtime protocols

| Protocol | Port | Format | Per packet |
|---|---|---|---|
| **DDP** (default) | 4048 | 10-byte header: flags `0x40` with push `0x01` on the last packet; sequence 1–15; type `0x0B` RGB or `0x1B` RGBW; destination 1; byte offset (u32 big-endian); length (u16 big-endian) | ≤1440 data bytes (480 RGB / 360 RGBW pixels) |
| DNRGB / DNRGBW (option) | 21324 | Byte 0 is the protocol (4 or 5), byte 1 the timeout in seconds (0 = leave realtime now, 255 = forever), then a start index (u16) and pixels | ≤489 / ≤367 LEDs |

**Behaviour:**
- After WLED sees the first DDP push flag, it renders **only on push**, so frames split across packets don't tear. DNRGB renders on every packet and can tear.
- A non-default "DMX start address" shifts DDP indices. FOLO warns about it.
- The realtime timeout defaults to **2.5 s** and applies to DDP. FOLO refreshes at least once a second while the output is static.
- WLED renders at most every 15 ms, about 66 fps. FOLO's default output rate is 60 fps.
- **Gamma:** WLED does **not** gamma-correct realtime data by default (`arlsDisableGammaCorrection` is true), so FOLO applies gamma.
- **Brightness:** realtime output is scaled by WLED's master brightness unless "Force max brightness" is on. The UI suggests 255 or force-max.
- **White channel:** a white value FOLO sends is honoured only in the Manual or Dual auto-white modes. Otherwise WLED derives white itself, and FOLO's adapter follows the device's mode.
- **Segments:** realtime data **ignores segments** unless "Use main segment only" (`mso`) is on. Then indices start at the main segment's first LED, and other segments keep animating.
- **Live override:** `lor` = 1 (until realtime ends) or 2 (always) makes WLED show its own effects while still accepting packets.

## FOLO provider design

- **Discovery:** mDNS `_wled._tcp` and manual entry.
- **Targets:**
  - the whole strip as a line target
  - **each segment as a selectable sub-target**, by absolute LED range, with a warning when `grp` or `spc` ≠ 1
  - a 2D matrix as a grid target
  - PWM and relay outputs as point targets with the colour model of their bus type
- **Main-segment-only mode:** detected through `info.liveseg`; indices are made relative to the main segment.
- **Release:** a DNRGB packet with timeout 0, or let the timeout run out.

## Basic administration

- device information, on/off, brightness, rename
- **create, edit and delete segments**
- check and fix realtime settings (DDP receive, timeout, force max brightness, gamma), with the device PIN where needed
- reboot
- open the device's own UI

The effect editor, presets and playlists, and firmware flashing stay in WLED.

## Sources

- WLED source: `wled00/json.cpp`, `udp.cpp`, `e131.cpp`, `FX_fcn.cpp`, `bus_manager.cpp`, `const.h`, `cfg.cpp`
- [WLED UDP realtime](https://kno.wled.ge/interfaces/udp-realtime/), [WLED DDP](https://kno.wled.ge/interfaces/ddp/), [DDP specification](http://www.3waylabs.com/ddp/)
