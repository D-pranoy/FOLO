# Capabilities, adapters and the flash limiter

Status: Accepted · Implementation: Not started

Decided in [ADR-0007](../decisions/ADR-0007-capabilities-adapters-and-flash-limiter.md).

## Capability descriptor

Every target declares what it can do. Providers discover this where possible; the user can override any of it.

| Axis | Values |
|---|---|
| Colour model | on/off, dimmer, tunable white (CCT), RGB, RGBW, RGB+CCT, xy within a gamut (Hue) |
| Addressing | Single (1 element), zoned (a few segments), pixel (many), derived from the element count |
| Delivery | **Streaming** (frames at a rate) or **state** (rate-limited commands, with transitions done by the device) |
| Limits | Maximum update rate, typical latency, CCT range, gamut, lowest brightness |

### Discovery

| Provider | Source of capability data |
|---|---|
| Hyperion | RGB pixels; Hyperion handles its own hardware |
| WLED | `/json/info` (`leds.lc`, `seglc`, `matrix`) and `/json/cfg` bus types ([WLED](../integrations/wled.md#capabilities)) |
| Hue | Light resources (gamut, colour temperature range, gradient points) and entertainment channels ([Hue](../integrations/hue.md)) |

## Adapter chain (per target, in order)

1. **Resolution.**
   - If an effect needs more elements than the target has (for example, scroll on a single lamp), render at a virtual resolution and combine down: mean in linear light, maximum, or dominant colour.
   - Otherwise sample directly.
2. **Colour model.**
   - **RGB:** pass through.
   - **RGBW / RGB+CCT:** extract a white channel. Modes: off, accurate, brighter. The CCT channel comes from the colour's temperature.
   - **CCT:** brightness from luminance; temperature from the nearest white, clamped to the lamp's range. Saturated colours use a per-target warm/cool mapping. Palettes may include explicit whites such as `2700K`.
   - **Dimmer:** luminance.
   - **On/off:** a threshold with hysteresis and a minimum on/off interval, to protect relays and bulbs.
   - **xy (Hue over REST):** RGB to CIE xy, clamped to the lamp's gamut. The Hue entertainment stream takes RGB, and the bridge converts.
3. **Calibration:** gamma, white balance, brightness minimum and maximum, colour order.
4. **Rate.**
   - Streaming targets receive frames at the provider rate, **only when they changed**, plus a keepalive inside the device's timeout ([pipeline](pipeline.md#controller-stages)).
   - State targets receive rate-limited updates (a token bucket per bridge or device), sent only on visible change, with a device-side transition equal to the update interval.
5. **Latency offset:** a per-output delay line, so faster outputs can wait for slower ones (for example, WLED waiting for Hue).
6. **Flash limiter:** see below.

**Parity mode** uses an identity chain: no gamma, no colour conversion, no limiter. This keeps byte equality with the current engine ([pipeline](pipeline.md#parity-mode)).

## Flash limiter

- **On by default**, applied after rendering. It limits large luminance transitions to at most **3 flashes per second**, following WCAG 2.3.1 ("three flashes or below threshold").
- It applies to all content, including third-party packs.
- A zone may opt out, for example for screen sync of films. Opting out shows a photosensitivity warning and is recorded in the zone's settings.
