# Capabilities, adapters and the flash limiter

Status: Accepted · Implementation: Not started

Decided in [ADR-0007](../decisions/ADR-0007-capabilities-adapters-and-flash-limiter.md). [ADR-0036](../decisions/ADR-0036-controller-output.md) supersedes the order of stages in ADR-0007 D2 and fixes what each stage computes.

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

For each target, after projection ([pipeline](pipeline.md#controller-stages)): **resolution → colour model → calibration → latency → flash limiter**. The rate isn't a stage of the chain: the provider's sender applies it afterwards ([providers](providers.md#the-sender)). A limiter or a delay line placed after the rate would act on frames the sender drops, repeats or paces. Colours are on the 0–255 scale.

1. **Resolution.**
   - The chain fits the projected elements to the target's `address.count`: a mean when there are more elements than addresses, repetition when there are fewer.
   - If an effect needs more elements than the target has (for example, scroll on a single lamp), the surface is rendered at a virtual resolution and combined down by the **mean** in linear light.
   - Otherwise elements sample directly.
2. **Colour model.**
   - **RGB:** pass through.
   - **RGBW / RGB+CCT:** extract a white channel, by `calibration.white`: `off`, `accurate` (the default) or `brighter`. The CCT channel comes from the colour's temperature.
   - **CCT:** brightness from luminance; temperature from the nearest white, clamped to the lamp's range. A fixed rule, not configurable, takes saturated colours to the warm or the cool end of the range. Palettes may include explicit whites such as `2700K`.
   - **Dimmer:** luminance.
   - **On/off:** a threshold with hysteresis and a minimum on/off interval, to protect relays and bulbs: `calibration.onoff` holds `threshold` (default 0.5), `hysteresis` (0.1) and `min_interval_ms` (1000).
   - **xy (Hue over REST):** RGB to CIE xy, clamped to the lamp's gamut. The Hue entertainment stream takes RGB, and the bridge converts.
3. **Calibration,** per element and channel:
   - **gamma:** `255 · (v / 255)^γ`
   - **white balance:** each channel times its factor
   - **brightness range:** non-zero values map into `[min, max]` of the full scale; zero stays zero
   - **colour order:** a permutation of the channels
   - **A target whose calibration holds only defaults skips the stage**, so the chain stays an identity for [parity](pipeline.md#parity-mode).
4. **Latency offset:** a per-target delay line, so faster outputs can wait for slower ones (for example, WLED waiting for Hue). A target's delay is its `calibration.latency_ms` plus the largest `limits.latency_ms` among the zone's targets minus its own, between 0 and 5 s.
5. **Flash limiter:** see below.

`calibration.white` and `calibration.onoff` are new fields of the target's calibration, which the configuration model gets with the contracts change that [ADR-0036](../decisions/ADR-0036-controller-output.md#consequences) lists.

**Rate** (the sender, after the chain):
- Streaming targets receive frames at the provider rate, **only when they changed**, plus a keepalive inside the device's timeout ([pipeline](pipeline.md#controller-stages)).
- State targets receive rate-limited updates (a token bucket per bridge or device), sent only on visible change, with a device-side transition equal to the update interval.

**Parity mode** uses an identity chain: no gamma, no colour conversion, no limiter. This keeps byte equality with the current engine ([pipeline](pipeline.md#parity-mode)). Migrated zones have the flash limiter off, as every zone does by default.

## Flash limiter

Decided in [ADR-0033](../decisions/ADR-0033-flash-limiter-off-by-default.md), which supersedes ADR-0007 D4; the algorithm is [ADR-0036](../decisions/ADR-0036-controller-output.md) D18.

- **A per-zone setting, off by default.** Switched on, it is applied after rendering and limits large luminance transitions to at most **3 flashes per second**, following WCAG 2.3.1 ("three flashes or below threshold").
- It applies to all content, including third-party packs and screen sync.
- **Warnings:** FOLO shows a photosensitive epilepsy warning on the first run, beside each zone's switch while it's off, and in Settings, which can turn it on for all zones. Built-in effects that flash quickly carry a "Flashing" badge ([user interface](ui.md#photosensitivity)).

### Algorithm

For zones that switched the limiter on:
- It works **per element**, on relative luminance (Rec. 709 weights on linear light), or on the brightness component for colour models that aren't RGB.
- A **transition** is a change of luminance of at least 0.10 whose darker side is below 0.80. A **flash** is two opposing transitions.
- Each element keeps a sliding window of 1 s. A transition that would make a fourth flash in the window is **held**: the element keeps its brightness until the window allows the change.
- On/off targets hold their state in the same way.
- Test patterns go through the limiter as any frame does.
