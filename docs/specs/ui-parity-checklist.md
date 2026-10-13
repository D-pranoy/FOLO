# UI parity checklist

Status: Accepted · Implementation: Not started

Every feature of the current studio UI (`web/studio.js`, `index.html`, `studio.css`) that the new UI must keep.
- **Fix** marks a defect that is corrected rather than reproduced.
- **New** marks new-UI behaviour that replaces a feature.
- **Visual details** follow the [visual design specification](visual-design.md).
- **Placement** in the new navigation is noted where it changes ([user interface](../architecture/ui.md#navigation)).

## 1. Top bar and global controls

In the new UI these live in the Home header on desktops and in Home cards on phones.

- [ ] **Status pills:**
  - **Audio:** device name / "No audio" with message / "Studio offline".
  - **Output:** generalised from Hyperion to every provider and source.
  - **FPS:** "N fps", or "– fps" when offline.
  - Pills write to the DOM only on change.
- [ ] **Now-playing chip:** cover (or ♪), title, artist; dimmed when paused; opens the Music card.
- [ ] **Tempo pill:** "N BPM", with a dot that flashes on each beat (accent) and on the bar's first beat (accent-2), by opacity only.
- [ ] **Audio-reactive switch:** On, Auto, Off. It sets the zone's reactive mode (it was the global `reactive` setting).
- [ ] **Ambient indicator:** glows in the ambient colour while the ambient light shows; clicking it opens the Ambient light settings.
- [ ] **Output toggle:** on/off, with `aria-pressed`.
- [ ] **Brightness:**
  - two synced sliders, perceptual: `master = (p/100)^2.2`
  - the slider that has focus is skipped when syncing; `--pct` drives the gradient track
  - keys `[` and `]` change it by ∓/±5. **Fix:** clamp the readout to 0–100.

## 2. Effects menu (library)

- [ ] Order: saved presets, genre presets, effects, LedFx-inspired effects. The default preset comes first.
- [ ] Category groups as collapsible sections:
  - My presets, LedFx-inspired, Electronic, Hip-hop & R&B, Pop & Rock, Jazz & Classical, Chill & Ambient, Visualizers
  - a "More" catch-all group
  - each group shows a count, a blurb and an "active" dot
- [ ] Open groups remembered (`studio.openCategories`). All groups open while searching.
- [ ] Search: whitespace-separated terms, all required, matched against name, description, group, tags, range roles and reaction names.
- [ ] Entry: a mini preview, name, source badge, ★ default, tempo dot; ▾ details with ranges (swatch, role, Hz, reaction), tags (hidden tags excluded) and the star toggle.
  - **Previews animate only while hovered or focused.** New: on touch, while pressed or when the entry is the active effect.
- [ ] Saved presets:
  - **Save current:** name of up to 40 characters; genre chips; extra tags, deduplicated case-insensitively.
  - **Edit:** save changes, cancel, delete (two-step; also clears the default).
  - **Fix:** the stale reference when saving an edit.
- [ ] Active marking: same segment count, and every segment equal on the comparison keys within 0.01. The comparison keys now include `beats`.
- [ ] Apply replaces segments, master and background, selects the first segment, and offers Undo.

## 3. Edit flow and live data

- [ ] Every edit mutates local state and saves after a 60 ms debounce.
- [ ] A server response replaces local state only if no newer edit happened and no drag is in progress (edit sequence, pending save and saving counters).
- [ ] When the server's revision or server id moves and nothing is pending, the UI reloads from the server and says why.
- [ ] Initial load retries every 2 s. The stream reconnects automatically, and the offline state shows in the pills.
- [ ] **A hidden page closes its live stream** and reopens it when shown.
- [ ] **New:** a visible "unsaved / save failed, retry" state.

## 4. Analyzer canvas

- [ ] Log frequency axis over the display range; dB axis −100…0; grid every 10 dB, labels every 20; frequency ticks at 20, 50, 100, 200, 500, 1k, 2k, 5k, 10k and 20k, needing at least 34 px between labels.
- [ ] Spectrum smoothing: rises instantly; falls 0.35 of the way per message; the peak line decays 0.6 dB per message.
- [ ] Segment boxes:
  - **hits boxes sit at the live average plus their offsets**
  - fill alpha follows the level; solid threshold line, dashed ceiling line
  - live level line
  - the selected box has handles and is drawn last; dimmed when disabled or another segment is soloed
  - name shown when the box is wider than 26 px
- [ ] Pointer rules:
  - Hit tolerance 6 px (16 px on touch). Priority: threshold → ceiling → low edge → high edge → move.
  - Frequency clamps [10, hi/1.02] and [lo×1.02, 24000], rounded to 3 significant figures.
  - dB rounded to 0.5 and kept at least 1 dB apart.
  - Move scales both frequencies by one factor and shifts both dB lines together.
  - Hits drags use the base frozen at pointer-down.
- [ ] Drag on empty space creates a segment (at least 8 px wide; dB from the box, or −60/−25 when it spans under 6 dB).
- [ ] The readout flips sides past 55% of the width.
- [ ] The static grid is cached offscreen.

## 5. LED strip and lane track

- [ ] **Strip:**
  - LED gaps by cell width; unlit colour `#17171f`
  - LED colours drawn as one stretched pixel row over cached LED shapes
  - a blurred glow layer; the selected segment outlined
  - clicking cycles the selection through the segments under that LED; hover shows the LED index and names
- [ ] **Track:**
  - lane height 28, gap 6, an empty extra row with a hint
  - static segment layers cached; only the level bars redraw
- [ ] **Track drags:**
  - edges snap to LEDs; moving keeps the length
  - vertical position picks the row; other segments don't move during the drag
  - readout "name · LED a–b (len) · row r"
- [ ] Dragging on empty space creates a segment on that row (at least 2 LEDs). A click without movement deselects.
- [ ] **Lane algorithms:**
  - `computeLanes`, `settleLanes` and `pinLanes`, as today
  - **Fix:** the fallback differs between hit-testing and drawing
- [ ] **Track bar:** + Lows, + Mids, + Highs and Delete.
  - **Kind:** by name prefix, else by band centre (<250 Hz Lows, <4000 Hz Mids, else Highs).
  - **Settings:** copied from the last segment of that kind, or the kind's defaults.
  - **Name:** the lowest free number.
  - **Placement:** the longest free run of at least 4 LEDs, else the right half of the selected or widest segment; "No room" when that host is under 2 LEDs.
  - Inherits the host's lane, is listed in strip order, and offers Undo.
- [ ] Delete (button or the Delete key) offers Undo.

## 6. Segment list

- [ ] Row: swatch, name, meta "LED a–b · lo–hi · Mode[ · Hits/Beat]", mini meter (transform-driven), solo, on/off switch. Selected (inset ring) and muted states. Enter or Space selects.
- [ ] Add segment:
  - band cycles through the 7 bands; palette cycles through the new palettes
  - free LED range; defaults level, −60/−25, fill, forward, attack 15, release 200, speed 120, width 20, beats 1
  - **Fix:** set a lane on creation
- [ ] Auto split:
  - n = 1…min(16, LEDs); log bands from 30 Hz to 16 kHz
  - thresholds from −50 to −72 dB, ceiling = threshold + 32
  - pulse, center, attack 10, release 220
  - asks before replacing; **New:** offers Undo
- [ ] Clear and Reset all. **New:** both offer Undo.
  - Reset keeps the audio, output, reactive, ambient, beat and now-playing settings and the saved presets.
  - Reset runs on the controller.

## 7. Inspector

- [ ] Header: swatch, name (up to 40 characters; blank becomes "Segment"), solo, on.
- [ ] **Reacts to:**
  - 7 band chips
  - low and high frequency (log scale)
  - trigger: **Level, Hits or Beat**, with help text
  - with Beat, **"Flash every"**: ½ beat, 1 beat, 2 beats, 1 bar
  - threshold and ceiling per trigger: hits are dB above the average (0–39, 1–40); level and beat are dBFS
  - meters and "x dB | y% out"
- [ ] Switching triggers converts thresholds only to and from hits (to hits: 4/12; from hits: average plus offset). The inspector rebuilds.
- [ ] **Response:** gain 0.1–8, curve 0.2–5, attack and release, idle glow 0–100%. **Fix:** attack and release limits come from the schema.
- [ ] **LEDs:**
  - first and last LED, "x of N LEDs"
  - mode (10) with help text; direction (4)
  - peak hold (fill only)
  - speed 0–2000 (6 moving modes)
  - size 1–100% (5 modes, not fire), labelled per mode
- [ ] **Colours:** chips, remove (2 or more; always visible on touch), add (up to 16), 9 palette presets, reverse.
- [ ] Footer: Duplicate (Ctrl/⌘+D) and Delete.
- [ ] **New:** fields are generated from the effect's parameter schema.

## 8. Colours by range

- [ ] 6 rows. A segment belongs to a row if √(lo·hi) falls in [lo, hi).
- [ ] The row's palette is seeded from its first segment. Edits apply to every segment in the range. Shows a "mixed" note. Doesn't re-render while focus is inside it.

## 9. Reactive presets

- [ ] Tiles: preview (animated only while hovered or focused; on touch, while pressed), name, "Hz · description", tempo badge.
- [ ] **25 presets in 5 groups**, including "On the beat" (5 presets with `beats`).
- [ ] Scopes: selected, all, new. "Keep range" switch for the selected scope. Matching tiles are pressed. Note text.
- [ ] Apply per scope, as today. **New:** offers Undo.

## 10. Ambient light

Moves to Zones → zone, with a shortcut from Home.

- [ ] Reactive mode: On, Auto, Off (shared with the top-bar switch).
- [ ] Colour: the eight Deep Night swatches plus a custom colour.
- [ ] Style: Solid or Breathe.
- [ ] Brightness: perceptual slider (`level` is linear).
- [ ] In Auto, "switch after" in steps of 1, 2, 3, 5, 8, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300 and 600 s.
- [ ] A live line saying what the zone shows (reactive, fading or ambient, and seconds of silence), and the top-bar indicator in the ambient colour.

## 11. Music (now playing and beat)

Moves to Home, per active source.

- [ ] Art (56 px), title, artist, player.
- [ ] **Previous / play-pause / next** (sent to the source's media session).
- [ ] **Album colours:** a switch, with the art's colours as swatches.
- [ ] **On pause:** a switch (the ambient light on pause, in Auto).
- [ ] **Players:** a switch to read media sessions at all.
- [ ] The four beats of the bar as dots, the tempo, and the **Beat timing** slider (±200 ms). **New:** a reset button replaces double-click.
- [ ] What's playing is rebuilt only when the media state changes. Beat dots change opacity only.
- [ ] States: off, unavailable (including Windows service mode), error, none, playing, paused, stopped.

## 12. Audio input

Moves to Sources → source → inputs.

- [ ] **Listen to:** one select, grouped:
  - everything you hear (the default output follows Windows)
  - each output
  - only one app (with "playing" marks)
  - everything except one app
  - inputs
- [ ] Rescan; the list refreshes every 5 s.
- [ ] **Status:** format, packets/s, delay, glitches.
- [ ] **Meters:** L/R, −60…0 dBFS, transform-driven, with a CLIP light.
- [ ] **Gain:** ±24 dB. **New:** a reset button replaces double-click.
- [ ] **Auto level:** a switch, showing the gain it adds right now.
- [ ] States for app capture: waiting (app not running), following, unavailable (Windows older than 10 2004, or Linux in v1).

## 13. Settings

- [ ] Output connection settings (generalised per provider), background colour, LED count, reload layout (generalised).

## 14. Shell behaviours

- [ ] Collapsible cards:
  - arrow buttons with `aria-expanded`
  - keys: effects, spectrum, strip, segments, colors, reactive, inspector, ambient, music, audio-input, settings
  - Settings starts collapsed; state remembered (`studio.collapsedCards`)
- [ ] Toasts:
  - one slot; 3.2 s, or 9 s with an action
  - **Fix:** a hidden Undo must not stay clickable
  - **Fix:** a new toast must not silently replace a pending Undo
- [ ] Two-step confirm buttons keep their size when armed.
- [ ] Keyboard shortcuts are ignored while typing: Delete, Escape, Ctrl/⌘+D, `[` and `]`.
- [ ] **Drawing only on invalidation;** no CSS animation at rest; previews still under reduced motion.
- [ ] Never use `innerHTML` for user content.

## 15. Content to carry over

- 9 palettes
- 25 reactive presets in 5 groups
- 14 effects, including Beat sync and Half-time pulse
- 18 LedFx-inspired effects (rewritten clean-room; Bar and Multicolor bar on the beat trigger)
- 14 genre presets with layouts (alternate, mirror, zones, stack)
- 8 categories, 7 bands, 6 colour ranges, genre tags

All move into packs ([content](../architecture/content.md#built-in-library)).
