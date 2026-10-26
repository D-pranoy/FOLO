# WLED

Status: Accepted · Implementation: Partial (RB-12 steps 1–8 built on fakes; live verification pending)

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
- **Release:** a DNRGB packet with timeout 0, sent when the last started target is released (see [Implementation](#implementation)).

## Basic administration

- device information, on/off, brightness, rename
- **create, edit and delete segments**
- check and fix realtime settings (DDP receive, timeout, force max brightness, gamma), with the device PIN where needed
- reboot
- open the device's own UI

The effect editor, presets and playlists, and firmware flashing stay in WLED.

## Implementation

The provider is the crate `crates/folo-provider-wled` (RB-12). The choices below are the ones the sections above leave open.

**Describe.** Target ids are `strip`, `seg:<id>`, `matrix` and `bus:<index>` (the bus's place in `hw.led.ins`).
- **Strip:** the digital buses' LEDs, from the first bus's start to the last one's end. A device with only PWM or relay outputs has no strip, and its segments aren't offered. A device whose settings can't be read (PIN) offers the strip from `leds.count` alone, as RGB, with a warning.
- **Colour model:** from `lc` (`seg[].lc` for a segment). RGB with a CCT bit is `rgbcct`; RGB with white is `rgbw` only when the **white channel is honoured**; RGB is `rgb`; CCT alone is `cct`; white alone is `dimmer`; `lc = 0` is `onoff`. Point targets take the model of their bus type (40 on/off, 41 dimmer, 42 CCT, 43 RGB, 44 RGBW, 45 and 46 RGB+CCT).
- **White honoured:** the global auto-white mode (`hw.led.rgbwm`, unless it is 255) or each bus's own (`ins[].rgbwm`) is Manual (0) or Dual (3). Else white goes unsent and the packets are RGB.
- **CCT:** realtime data has no colour-temperature channel. A CCT, RGB+CCT or dimmer element sends its brightness as the white byte (as grey RGB when white isn't honoured, which every auto-white mode turns into white); its kelvin is dropped, and WLED uses the segment's own `cct`.
- **Matrix:** one grid target (`matrix`, `w × h` LEDs in row order) with its shape as a layout. Segments are rectangles there, not LED ranges, so they aren't offered.
- **Main segment only:** only that segment is a target, indices are relative to its first LED, and a warning says the rest can't be driven.
- **Warnings** (logged on each describe, and in the `info` action's `warnings`): grouping or spacing on a segment, a DMX start address other than 1, a main-segment-only device, and unreadable settings. The provider interface carries no warnings with a target.
- **Rate:** the targets' `max_rate_hz` is 66.

**Output.**
- **DDP:** the sequence counts packets, 1–15 and back to 1. One `send` is one frame: only its last packet has the push flag, whichever targets it covers.
- **DNRGB(W):** the timeout byte is the instance's `timeout_s`, kept to 1–254, since 255 would never time out. WLED ignores a packet of one pixel, so a lone trailing pixel is paired with the one before it, and a frame of a single pixel goes out with a copy of itself on the next LED. Prefer DDP for point targets. The UDP port is `info.udpport`.
- **Release:** both transports end realtime with a DNRGB packet of timeout 0, sent when the last started target is released. Starts and releases are counted per target, and a release of a target that isn't running sends nothing. If the packet is lost, the device's timeout ends the output.
- **Refresh:** the provider's keepalive is 1 s, which the sender applies to unchanged frames.

**Discovery.** mDNS `_wled._tcp` (the `mdns-sd` crate, a 3 s window); a device's id is the TXT record's `mac`, else its instance name; the address is the first IPv4 address, with `:port` when not 80. Manual entry is `probe`, which asks `/json/info` and accepts a device whose `brand` is WLED.

**Administration** (`/providers/{id}/admin/{action}`). A `GET` states the change (`summary` and `changes`) and a `POST` makes exactly that, answering with `changed` and `current`.

| Action | Body | Request made |
|---|---|---|
| `info` | | none: name, version, MAC, LEDs, segments, realtime settings, targets, warnings, UI URL |
| `power` | `{"on": bool}` | `POST /json/state` `{"on"}` |
| `brightness` | `{"bri": 0–255}` | `POST /json/state` `{"bri"}` |
| `rename` | `{"name": "…"}` (1–32 characters) | `POST /json/cfg` `{"id": {"name"}}` |
| `segments` | `{"op": "create"\|"edit"\|"delete", "id", "start", "stop", "name", "grp", "spc", "on", "bri"}` | `POST /json/state` `{"seg": [...]}`; a delete sends `stop: 0`; a create takes the lowest free id |
| `realtime` | | `POST /json/cfg` `{"if": {"live": {...}}}` with what differs of: `en` on, `timeout` the instance's `timeout_s` in tenths of a second, `maxbri` on, `no-gc` on (the device's realtime gamma off) |
| `reboot` | | `POST /json/state` `{"rb": true}` |
| `ui` | | none: `http://<host>/` |

The device PIN (`secrets.json`) goes in the body of settings requests as `pin`, as WLED reads it; reading `/json/cfg` unlocks the device with it first when needed. The `current` of the realtime action also shows `main_segment_only` and the DMX start address, which the fix leaves alone.

## Sources

- WLED source: `wled00/json.cpp`, `udp.cpp`, `e131.cpp`, `FX_fcn.cpp`, `bus_manager.cpp`, `const.h`, `cfg.cpp`
- [WLED UDP realtime](https://kno.wled.ge/interfaces/udp-realtime/), [WLED DDP](https://kno.wled.ge/interfaces/ddp/), [DDP specification](http://www.3waylabs.com/ddp/)
