# Visual design

Status: Accepted · Implementation: Partial (the current studio implements the visual language; the new UI implements this whole specification)

The visual language of every FOLO screen. Decided in [ADR-0029](../decisions/ADR-0029-visual-design-language.md). Layout and navigation are in [user interface](../architecture/ui.md). Behaviour is in the [UI parity checklist](ui-parity-checklist.md).

## Principles

1. **Glass over glow.** Frosted, translucent panels float over a fixed background of soft Deep Night glows.
2. **The lights are the colour.** Canvases, LED previews, swatches and status lights carry colour. Chrome stays neutral, white at low opacity.
3. **Depth by translucency.** Panels are glass; controls are sunk into the glass as wells; selection is a coloured ring, not a fill.
4. **Quiet at rest.** Nothing moves unless data or the user moves it. No idle animation.
5. **Cheap to render.** The design must keep the page light on CPU and GPU ([performance rules](#performance-rules)).

## Tokens

Defined once (`ui/src/styles/tokens.css`) and exported as JSON for the desktop app (tray status colours). The values are those of the current studio.

**Background**

| Token | Value | Use |
|---|---|---|
| `--bg` | `#07060d` | Page background under the glows |

**Surfaces**

| Token | Value | Use |
|---|---|---|
| `--glass` | `linear-gradient(150deg, rgba(255,255,255,.085), rgba(255,255,255,.025) 42%, rgba(255,255,255,.04)), rgba(14,10,28,.38)` | Panels: cards, top bar, toasts, sheets, dialogs |
| `--glass-line` | `rgba(255,255,255,.11)` | Panel edge |
| `--glass-shadow` | `inset 0 1px 0 rgba(255,255,255,.1), 0 20px 50px -24px rgba(0,0,0,.75)` | Panel top highlight and drop shadow |
| `--glass-blur` | `blur(22px) saturate(160%)` | Panel `backdrop-filter` |
| `--panel` | `rgba(255,255,255,.05)` | Pills, small chips |
| `--panel-2` | `rgba(255,255,255,.04)` | Rows, tiles (light glass) |
| `--panel-3` | `rgba(255,255,255,.09)` | Raised controls (pressed segment buttons) |
| `--well` | `rgba(6,4,14,.42)` | Inputs, switches, meters, previews (sunk into the glass) |
| `--hover` | `rgba(255,255,255,.07)` | Hover and pressed fill for rows and tiles |

**Lines**

| Token | Value | Use |
|---|---|---|
| `--line` | `rgba(255,255,255,.08)` | Default borders |
| `--line-2` | `rgba(255,255,255,.14)` | Stronger borders, hover borders |
| `--line-hover` | `rgba(255,255,255,.24)` | Hovered controls |

**Text**

| Token | Value | Use |
|---|---|---|
| `--text` | `#ecebf5` | Primary text |
| `--muted` | `#9391ad` | Secondary text |
| `--faint` | `#5f5d78` | Decorative and non-text marks only (fails AA as body text) |

**Accents and status**

| Token | Value | Use |
|---|---|---|
| `--accent` | `#b36bff` | Selection, focus, active state, beat dot |
| `--accent-2` | `#ff4fa8` | Secondary emphasis; the first beat of the bar |
| `--ok` | `#3ddc97` | Healthy status |
| `--warn` | `#ffb020` | Degraded status |
| `--err` | `#ff5470` | Errors, destructive confirmations |
| `--seg` | Per segment (from its palette) | The selected segment's accent in the inspector and lists |

**Shape and type**

| Token | Value | Use |
|---|---|---|
| `--radius` | `16px` | Panels. Rows 10px, controls 7–9px, swatches 6–8px, pills and dots 999px or 50%. |
| `--mono` | `ui-monospace, "Cascadia Mono", "SFMono-Regular", Consolas, monospace` | Numbers and readouts, with `font-variant-numeric: tabular-nums` |
| Body font | `14px/1.45 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif` | All text. No web fonts. |

## Background

- A fixed full-screen layer behind everything (`body::before`, `position: fixed`, `z-index: -1`, `transform: translateZ(0)`).
- **Never** use `background-attachment: fixed`, which repaints on scroll.
- Five radial glows over `--bg`, drawn from the Deep Night palette:

| Position | Size | Colour |
|---|---|---|
| 6% 8% | 42vmax × 34vmax | UV `rgba(75,0,255,.42)` |
| 96% 4% | 36vmax × 30vmax | Pink `rgba(255,51,153,.26)` |
| 84% 92% | 46vmax × 36vmax | Deep blue `rgba(0,51,204,.34)` |
| 14% 96% | 34vmax × 30vmax | Violet `rgba(143,0,255,.30)` |
| 52% 46% | 30vmax × 24vmax | Magenta `rgba(255,0,255,.09)` |

Each fades to transparent at 70%.

## Surface hierarchy

| Level | Surfaces | Treatment |
|---|---|---|
| 0 | Background | Glows ([above](#background)) |
| 1 | Panels: cards, top bar, toasts, bottom sheets, dialogs | `--glass` fill, `--glass-line` border, `--glass-shadow`, `backdrop-filter: var(--glass-blur)`, radius 16px. The top bar uses a vertical glass gradient with a bottom border. |
| 2 | Rows, tiles, list items | `--panel-2` fill and `--line` border. On hover or press: `--hover` fill and `--line-2` border. Selected: an inset 1px ring in the item's accent (`--seg` or `--accent`). |
| Well | Inputs, switches, segmented controls, meters, previews, sliders | `--well` fill, `--line` border |
| Raised | Pressed segmented buttons, active toggles | `--panel-3` fill, inset `--line-2` ring, white text |

**Rule:** only level-1 panels blur. **Never nest backdrop filters.**

## Colour semantics

- **Status dots** are 7–8 px, glowing in their colour (`box-shadow: 0 0 8px <colour>`): ok, warn, err, or faint when off.
- **The beat dot** flashes `--accent` on each beat and `--accent-2` on the first beat of the bar, by opacity only.
- **The ambient light indicator** glows in the ambient colour while the ambient light is on.
- **Segment and palette colours** appear only in swatches, canvases, meters and selection rings.
- **Destructive actions** use `--err` text and a two-step confirm, or an Undo toast.

## Typography

| Element | Style |
|---|---|
| Application title | 17px, weight 650 |
| Card titles | `h2` in the card head, with a collapse arrow |
| Section labels | Small uppercase, letter-spacing 0.08em, `--muted` |
| Readouts, values, BPM, fps | `--mono`, tabular numbers |

## Motion

- **Durations:** state transitions take 150–400 ms (collapse arrow rotation 150 ms; ambient indicator fade 400 ms). No easing longer than 400 ms in chrome.
- **Live data** (meters, beat dots, LED previews) moves by `transform` or `opacity` only, never layout properties.
- **Previews** (effect tiles, reactive tiles) are still at rest. They animate only while hovered or focused, or on touch devices while pressed or while the item is the active effect.
- **`prefers-reduced-motion: reduce`** stops all preview animation and non-essential transitions.

## Focus and accessibility

- **Focus ring:** `outline: 2px solid var(--accent)` with a 2 px offset, on `:focus-visible`.
- **Contrast:** body text must meet WCAG 2.2 AA (4.5:1) against the **brightest glow** behind its panel. `--faint` is for non-text marks only, and secondary text uses `--muted`.
- **Reduced transparency:** honour `prefers-reduced-transparency` where supported. The setting below gives the same result everywhere.

## Performance rules

These are normative. They come from measurements of the current studio: the page used to cost a browser about 2.2 cores; these rules brought it to about 0.6 on a 1920×1080, 144 Hz display.

1. **Draw on invalidation only.** Canvases, meters and status update when stream data, user input, a resize or a state change asks for it. There is no render loop running at the display refresh rate.
2. **No CSS animation at rest.** Any running animation makes the browser repaint the whole page at the display's refresh rate.
3. **Cache static layers offscreen:** the analyzer grid, LED shapes, track segments without their live bars. Rebuild them only when their inputs change.
4. **Live bars** use `transform` (scale or translate). LED colours are drawn as one row of pixels stretched over the LED shapes.
5. **Hidden pages close the live stream** and reopen it when shown.
6. **Blur costs GPU time** (the panel blur costs about 8–10% of a core while canvases inside blurred panels update). Provide **Settings → Appearance → Reduce transparency**, which:
   - replaces `--glass` with an opaque `rgba(14,10,28,.94)`
   - removes `backdrop-filter`
   - keeps borders and shadows
7. **Fallback:** where `backdrop-filter` isn't supported (`@supports not (backdrop-filter: blur(1px))`), use the reduce-transparency surfaces. Older WebKitGTK builds on Linux may need this.

## Touch and responsive adaptation

The visual language is the same at every size. The new UI's layouts follow [ADR-0016](../decisions/ADR-0016-user-interface.md):

| Width | Layout | Panels |
|---|---|---|
| Under 600 px | One column, bottom navigation | Inspectors and cards open as glass bottom sheets |
| 600–1024 px | Two panes | |
| Over 1024 px | Three columns: navigation or library 290 px, editor fluid, inspector 380 px, maximum 1960 px | As today |

These replace the current studio's breakpoints (1500, 1100 and 640 px) in the new UI.

**Touch adaptations:**
- Touch targets are at least 48 dp. Visual sizes may stay compact if the hit area is padded.
- Hover styles also apply to pressed states, and nothing is reachable only by hover. Remove buttons are always visible on touch.
- **Double-click resets** (gain, beat timing) get an explicit reset button or long-press menu entry.
- Tooltips become long-press info sheets.

## Components (summary)

| Component | Appearance |
|---|---|
| Top bar | Sticky glass. Brand; status pills; now-playing chip (22 px round cover, title, artist; dimmed when paused); tempo pill (BPM with beat dot); audio-reactive segmented control (On, Auto, Off); ambient indicator; brightness slider; output toggle. |
| Pill | `--panel` fill, 999px radius, status dot, label |
| Segmented control | A well with 3 px padding; buttons 6 px radius; pressed = raised |
| Switch | A well track with a white knob that slides by `transform` |
| Slider | A well track. The brightness track is a gradient driven by `--pct`. |
| Meter | A well bar with a translucent white level bar, threshold and ceiling marks, and an L/R pair with a clip light in the audio input card |
| Card | Level-1 glass, a head with a title and collapse arrow, then body content |
| Row | Level 2. Swatch (30 px, 8 px radius), name, meta text, mini meter (6 px well), solo, switch. |
| Swatch | Square, 6–8 px radius, 1 px light border |
| Toast | Level-1 glass at the bottom, with an optional action button (Undo) |
| Music card | Cover 56 px (10 px radius), title, artist, player buttons, beat dots of the bar, tempo, timing slider |
| Ambient card | Nine colour swatches (the Deep Night colours plus custom), Solid or Breathe, brightness slider, "switch after" steps |
