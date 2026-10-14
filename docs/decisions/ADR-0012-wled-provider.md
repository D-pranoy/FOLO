# ADR-0012: WLED provider

Status: Accepted
Date: 2026-10-13

## Context

WLED offers several realtime protocols. Its source code shows:
- realtime data bypasses segments (unless "main segment only" is on)
- gamma isn't applied to realtime data by default
- frames render at most every 15 ms
- DDP's push flag allows tear-free multi-packet frames

## Decision

- **D1.** **DDP** (UDP 4048) is the default transport, with push on the last packet of each frame. DNRGB/DNRGBW (UDP 21324, with a timeout byte) is an option.
- **D2.** Default output 60 fps, refreshed at least once a second while static.
- **D3.** FOLO applies gamma. The UI suggests WLED brightness 255 or "force max brightness".
- **D4.** Capabilities come from `/json/info` (`lc`, `seglc`, `matrix`) and `/json/cfg` bus types:
  - segments as selectable sub-targets
  - matrices as grids
  - PWM and relay outputs as point targets
- **D5.** "Main segment only" devices are handled through `info.liveseg`.
- **D6.** Basic administration as listed in [WLED](../integrations/wled.md#basic-administration), including segment create, edit and delete.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| DNRGB by default | Timeout per packet | Tears on multi-packet frames |
| E1.31 / Art-Net | Standard | Universe handling; 170 pixels each |
| Through Hyperion only | No new code | Not direct; one device per instance |

## Consequences

A fake UDP receiver tests packet formats and push behaviour.
