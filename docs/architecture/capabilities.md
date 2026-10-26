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
   - If an effect needs more elements than the target has (for example, scroll on a single lamp), the surface is rendered at a virtual resolution and combined down by the **mean** of the channel values (they are linear light already, [pipeline](pipeline.md#frames-and-signals)).
   - Otherwise elements sample directly.
2. **Colour model.**
   - **RGB:** pass through.
   - **RGBW / RGB+CCT:** extract a white channel, by `calibration.white`: `off`, `accurate` (the default) or `brighter`. The CCT channel comes from the colour's temperature.
   - **CCT:** brightness from luminance; temperature from the nearest white, clamped to the lamp's range. A fixed rule, not configurable, takes saturated colours to the warm or the cool end of the range. Palettes may include explicit whites such as `2700K`.
   - **Dimmer:** luminance.
   - **On/off:** a threshold with hysteresis and a minimum on/off interval, to protect relays and bulbs: `calibration.onoff` holds `threshold` (default 0.5), `hysteresis` (0.1) and `min_interval_ms` (1000). `hysteresis` is the **full width** of the band around `threshold`: the target switches on at `threshold + hysteresis / 2` and off at `threshold − hysteresis / 2` (each clamped to 0–1), and keeps its state in between. Past the minimum interval since its last change, a level beyond either point switches it.
   - **xy (Hue over REST):** RGB to CIE xy, clamped to the lamp's gamut. The Hue entertainment stream takes RGB, and the bridge converts.
3. **Calibration,** per element, on the 0–255 scale:
   - **gamma:** `255 · (v / 255)^γ`
   - **white balance:** each of R, G and B times its factor
   - **brightness range:** non-zero values map into `[min, max]` of the full scale, and zero stays zero. The minimum is the larger of `brightness_min` and the target's `limits.min_brightness`.
   - **colour order:** a permutation of R, G and B
   - **Channels each step touches,** by colour model:

     | Colour model | gamma and brightness range | white balance and colour order |
     |---|---|---|
     | RGB | R, G, B | R, G, B |
     | RGBW | R, G, B and W | R, G, B |
     | RGB+CCT | R, G, B and the CCT channel's brightness | R, G, B |
     | CCT, dimmer | the brightness | none |
     | xy | the brightness component | none |
     | on/off | none: `calibration.onoff` governs it | none |

     The colour temperature of a CCT channel isn't touched by calibration.
   - **A target whose calibration holds only defaults skips the stage**, so the chain stays an identity for [parity](pipeline.md#parity-mode).
4. **Latency offset:** a per-target delay line, so faster outputs can wait for slower ones (for example, WLED waiting for Hue). A target's delay is its `calibration.latency_ms` plus the largest `limits.latency_ms` among the zone's targets minus its own, between 0 and 5 s.
5. **Flash limiter:** see below.

`calibration.white` and `calibration.onoff` are fields of the target's calibration ([configuration model](../specs/config-model.md#entities-summary)).

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

For zones that switched the limiter on. The limiter is the chain's last stage, so it acts on what the lights will show.
- **Luminance** is measured per element, from the values as they leave calibration, scaled to 0–1 and treated as linear light (no transfer curve is applied). It follows the target's colour model:

  | Colour model | Luminance |
  |---|---|
  | RGB | `0.2126 R + 0.7152 G + 0.0722 B` (Rec. 709), on the **logical** channels: the limiter undoes the target's `color_order` first |
  | RGBW, RGB+CCT | that, plus the white (or the CCT) channel's value, at most 1 |
  | CCT, dimmer | the brightness |
  | xy | the brightness component |
  | on/off | 0 or 1 |

  A white flash on an RGBW strip, which `accurate` extraction turns into W alone, therefore still counts.
- **A run** is a series of steps in which an element's luminance moves the same way (a step that doesn't change it continues the run). It ends when the luminance turns.
- **A transition** is counted when a run's cumulative change reaches 0.10 and the darker end of that change is below 0.80. A run keeps counting: after a transition, its change starts again from the current luminance. Transitions are stamped with the time of the step that completes them (`now_us`), so the result doesn't depend on how many steps a second the controller takes.
- **A flash** is a counted transition that opposes the one counted before it (two opposing transitions). It is stamped with its own time.
- **The window:** each element keeps its flashes of the last 1 s. A transition that would complete a **fourth** flash in the window is **held**: the element's output stays at the last colour that passed, **whole**, with every channel as it was, not only the luminance.
- **Release:** while a transition is held, the output stays until the oldest flash in the window is older than 1 s. The output then jumps to the **current input**, which counts as the flash it completes; the held transitions in between are not replayed. If the input has returned to within 0.10 of the held output by then, nothing is held any more and no transition is counted.
- **On/off targets** hold their state in the same way: the output stays on or off until the window allows the change.
- **Test patterns** go through the limiter as any frame does.
