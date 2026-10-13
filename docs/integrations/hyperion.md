# Hyperion.NG

Status: Accepted · Implementation: Not started

Decided in [ADR-0011](../decisions/ADR-0011-hyperion-provider.md).

## Protocol

- **JSON API** over TCP (default port 19444). Commands are newline-terminated JSON. Hyperion answers every command, so replies must be read continuously, or the socket buffers fill up.
- **Frames:** `{"command":"color","color":[r,g,b,…],"priority":100,"origin":"FOLO","duration":2000}`
  - `origin` must be 20 characters or fewer.
  - `duration` makes Hyperion release the LEDs if FOLO stops sending.
- **Layout:** `{"command":"serverinfo"}` → `info.leds[]` with `hmin/hmax/vmin/vmax`, a rectangle per LED on the screen.
- **Useful admin commands** (no authentication on default installs):
  - `{"command":"clear","priority":N}`
  - `{"command":"sourceselect","auto":true}`
  - `{"command":"componentstate","componentstate":{"component":"<C>","state":<bool>}}`
  - Other commands may need an API token when Hyperion's authentication is on.
- A FlatBuffers interface exists (port 19400). It is a later efficiency option.

## FOLO provider design

- **Targets:**
  - One line target per Hyperion instance, with the LED count from `serverinfo`.
  - Element positions come from the LED rectangles, placed on the zone's TV plane: x from the rectangle's horizontal centre, z from its vertical centre. Hyperion setups therefore place themselves.
- **Layout changes:** watched (polling `serverinfo`). Mappings scale proportionally, as the current studio does.
- **Output:** one sender per instance at its configured rate (default 40 Hz). Priority and origin are configurable. On stop, FOLO clears its priority.
- **Parity:** migrated studio setups run through this provider unchanged ([pipeline](../architecture/pipeline.md#parity-mode)).

## Basic administration

- instance list and selection
- LED layout view and import
- **priority list, with clear**
- component toggles: LED device, smoothing, black border, audio, V4L
- automatic source selection
- basic brightness
- API token support
- open Hyperion's own UI

Device configuration and effect scripting stay in Hyperion.
