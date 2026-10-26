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
   - **RGBW / RGB+CCT:** extract a white channel, by `calibration.white`: `off` (W is 0), `accurate` (the default: W is the smallest of R, G and B, and it is taken out of each of them, so the colour keeps its look) or `brighter` (the same W, with R, G and B left as they are). On RGB+CCT that W is the CCT channel's level, and its temperature is the colour's, as for CCT.
   - **CCT:** brightness from luminance (Rec. 709); temperature from the nearest white, by McCamy's approximation on the colour's CIE xy, clamped to the lamp's range (black takes the middle of the range). A fixed rule, not configurable, takes saturated colours, whose `(max − min) / max` of R, G and B is at least 0.5, to the warm end of the range when R is at least B, and to the cool end otherwise. Palettes may include explicit whites such as `2700K`.
   - **Dimmer:** luminance (Rec. 709).
   - **On/off:** a threshold with hysteresis and a minimum on/off interval, to protect relays and bulbs: `calibration.onoff` holds `threshold` (default 0.5), `hysteresis` (0.1) and `min_interval_ms` (1000). `hysteresis` is the **full width** of the band around `threshold`: the target switches on at `threshold + hysteresis / 2` and off at `threshold − hysteresis / 2` (each clamped to 0–1), and keeps its state in between. The level is the luminance on 0–1. An element that is off switches on at a level of at least the on point, and never at 0; one that is on switches off below the off point, and always at 0. A change waits until `min_interval_ms` has passed since the element's last change; the first change doesn't wait.
   - **xy (Hue over REST):** RGB to CIE xy, taking the values as linear sRGB with a D65 white (black is D65's white point); a point outside the lamp's gamut moves to the nearest point of the gamut's edges. The brightness component is the luminance. The Hue entertainment stream takes RGB, and the bridge converts.
3. **Calibration,** per element, on the 0–255 scale:
   - **gamma:** `255 · (v / 255)^γ`
   - **white balance:** each of R, G and B times its factor
   - **brightness range:** non-zero values map into `[min, max]` of the full scale, and zero stays zero. The minimum is the larger of `brightness_min` and the target's `limits.min_brightness`. It applies to each channel on its own, so a small non-zero channel rises to the minimum too.
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
4. **Latency offset:** a per-target delay line, so faster outputs can wait for slower ones (for example, WLED waiting for Hue). A target's delay is its `calibration.latency_ms` plus the largest `limits.latency_ms` among the zone's targets minus its own, between 0 and 5 s. Until the line holds a frame as old as the delay, it shows the first frame it got, so the lights start with a picture rather than black.
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
- **A run** is a series of steps in which an element's luminance moves the same way (a step that doesn't change it continues the run). It ends only when the luminance turns back by at least 0.02 from the run's furthest point; a smaller reversal doesn't split the run, so noise on a fade can't keep it from being counted.
- **A transition** is counted when a run's cumulative change reaches 0.10 and the darker end of that change is below 0.80. A run keeps counting: after a transition, its change starts again from the current luminance. Transitions are stamped with the time of the step that completes them (`now_us`), so the result doesn't depend on how many steps a second the controller takes.
- **A flash** is a pair of counted transitions in opposite directions: a transition opens it, the next transition that opposes that one completes it, and the one after opens the next. A 3 Hz blink (six transitions a second) is therefore three flashes a second. A transition in the same direction as the one before it (a slow rise counted in several steps) neither opens nor completes a flash. A flash is stamped with the time of the transition that completes it.
- **The window:** each element keeps its flashes of the last 1 s. A flash leaves it at 0.98 s: the last 20 ms are a tolerance for the controller's clock, so a steady 3 Hz blink isn't held by timing jitter. A transition that would complete a **fourth** flash in the window is **held**: the element's output stays at the last colour that passed, **whole**, with every channel as it was, not only the luminance. The transition that opens a fourth flash isn't held, only the one that completes it, so what the lights show can include one open flash in addition to three.
- **Release:** while a transition is held, the output stays until the oldest flash in the window is older than 1 s. The output then jumps to the **current input**, which counts as the flash it completes; the held transitions in between are not replayed. If the input has returned to within 0.10 of the held output by then, nothing is held any more and no transition is counted.
- **On/off targets** hold their state in the same way: the output stays on or off until the window allows the change.
- **Test patterns** go through the limiter as any frame does.
