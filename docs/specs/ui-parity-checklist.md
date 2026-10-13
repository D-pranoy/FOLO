# UI parity checklist

Status: Accepted · Implementation: Not started

Every feature of the current studio UI (`web/studio.js`, `index.html`, `studio.css`) that the new UI must keep. "Fix" marks a defect that is corrected rather than reproduced. New-UI behaviour that replaces a feature is noted. Line references are to the current `web/studio.js`.

## 1. Top bar and brightness

- [ ] Status pills:
  - **Audio:** device name / "No audio" with message / "Studio offline".
  - **Output:** generalised from Hyperion to every provider and source.
  - **FPS:** "N fps", or "– fps" when offline.
  - Pills write to the DOM only on change.
- [ ] Output on/off toggle (`aria-pressed`).
- [ ] Two synced brightness sliders, perceptual: `master = (p/100)^2.2`. The slider that has focus is skipped when syncing. `--pct` drives the gradient track.
- [ ] Keys `[` and `]` change brightness by ∓/±5. **Fix:** clamp the readout to 0–100.

## 2. Effects menu (library)

- [ ] Order: saved presets, genre presets, effects, LedFx-inspired effects. The default preset comes first.
- [ ] Category groups as collapsible sections:
  - My presets, LedFx-inspired, Electronic, Hip-hop & R&B, Pop & Rock, Jazz & Classical, Chill & Ambient, Visualizers
  - a "More" catch-all group
  - each group shows a count, a blurb and an "active" dot
- [ ] Open groups remembered (`studio.openCategories`). All groups open while searching.
- [ ] Search: whitespace-separated terms, all required, matched against name, description, group, tags, range roles and reaction names.
- [ ] Entry: animated mini preview, name, source badge, ★ default, tempo dot; ▾ details with ranges (swatch, role, Hz, reaction), tags (hidden tags excluded) and the star toggle.
- [ ] Saved presets:
  - **Save current:** name of up to 40 characters; genre chips; extra tags, deduplicated case-insensitively.
  - **Edit:** save changes, cancel, delete (two-step; also clears the default).
  - **Fix:** the stale reference when saving an edit.
- [ ] Active marking: same segment count, and every segment equal on the comparison keys within 0.01.
- [ ] Apply replaces segments, master and background, selects the first segment, and offers Undo.

## 3. Edit flow and live data

- [ ] Every edit mutates local state and saves after a 60 ms debounce.
- [ ] A server response replaces local state only if no newer edit happened and no drag is in progress (edit sequence, pending save and saving counters).
- [ ] When the server's revision or server id moves and nothing is pending, the UI reloads from the server and says why (LED count change or sync).
- [ ] Initial load retries every 2 s. The stream reconnects automatically, and the offline state shows in the pills.
- [ ] **New:** a visible "unsaved / save failed, retry" state. A failed save currently only shows a toast.

## 4. Analyzer canvas

- [ ] Log frequency axis over the display range; dB axis −100…0; grid every 10 dB, labels every 20; frequency ticks at 20, 50, 100, 200, 500, 1k, 2k, 5k, 10k and 20k, needing at least 34 px between labels.
- [ ] Spectrum smoothing: rises instantly; falls 0.35 of the way per message; the peak line decays 0.6 dB per message. Pink→violet→blue fill with a white line.
- [ ] Segment boxes:
  - **hits boxes sit at the live average plus their offsets**
  - fill alpha follows the level; solid threshold line, dashed ceiling line
  - live level line
  - the selected box has handles and is drawn last; dimmed when disabled or another segment is soloed
  - name shown when the box is wider than 26 px
- [ ] Pointer rules:
  - Hit tolerance 6 px (16 px on touch). Priority: threshold → ceiling → low edge → high edge → move. Cursors per part.
  - Low edge clamped to [10, hi/1.02], high edge to [lo×1.02, 24000], rounded to 3 significant figures.
  - dB rounded to 0.5 and kept at least 1 dB apart.
  - Move scales both frequencies by one factor and shifts both dB lines together.
  - Hits drags use the base frozen at pointer-down.
- [ ] Drag on empty space creates a segment (at least 8 px wide; dB from the box, or −60/−25 when it spans under 6 dB; LEDs from the free range).
- [ ] The readout box flips sides past 55% of the width.

## 5. LED strip and lane track

- [ ] Strip: LED gaps by cell width; unlit colour `#17171f`; a blurred glow layer; the selected segment outlined. Clicking cycles the selection through the segments under that LED. Hover shows the LED index and names.
- [ ] Track geometry: lane height 28, gap 6, an empty extra row with a hint text.
- [ ] Track drags:
  - edges snap to LED boundaries; moving keeps the length
  - vertical position picks the row; other segments don't move during the drag
  - readout "name · LED a–b (len) · row r"
- [ ] Dragging on empty space creates a segment on that row (at least 2 LEDs). A click without movement deselects.
- [ ] Lane algorithms:
  - `computeLanes`: pinned segments first, then the moved one, then unpinned ones from row 0; empty rows close up
  - `settleLanes` stores every segment's displayed row
  - `pinLanes` makes the dragged segment the one pushed down on conflict
  - **Fix:** the fallback differs between hit-testing and drawing
- [ ] Track bar: + Lows, + Mids, + Highs, and Delete.
  - **Kind:** by name prefix, else by band centre (<250 Hz Lows, <4000 Hz Mids, else Highs).
  - **Settings:** copied from the last segment of that kind, or the kind's defaults.
  - **Name:** the lowest free number.
  - **Placement:** the longest free run of at least 4 LEDs, else the right half of the selected or widest segment; "No room" when that host is under 2 LEDs.
  - Inherits the host's lane, is listed in strip order, and offers Undo.
- [ ] Delete (button or the Delete key) offers Undo.

## 6. Segment list

- [ ] Row: swatch, name, meta "LED a–b · lo–hi · Mode[ · Hits]", mini meter, solo button, on/off switch. Selected and muted states. Enter or Space selects.
- [ ] Add segment:
  - band cycles through the 7 bands; palette cycles through the new palettes
  - free LED range; defaults level, −60/−25, fill, forward, attack 15, release 200, motion defaults
  - **Fix:** set a lane on creation
- [ ] Auto split:
  - n = 1…min(16, LEDs); log bands from 30 Hz to 16 kHz; names like "1 · Bass"
  - thresholds from −50 to −72 dB, ceiling = threshold + 32
  - pulse, center, attack 10, release 220, adjacent Deep Night colours
  - asks before replacing; **New:** offers Undo
- [ ] Clear and Reset all. **New:** both offer Undo. Reset uses the default preset on the controller.

## 7. Inspector

- [ ] Header: swatch, name (up to 40 characters, saved on each keystroke, blank becomes "Segment"), solo, on.
- [ ] **Reacts to:**
  - 7 band chips
  - low and high frequency (log scale)
  - trigger (Level/Hits) with help text; threshold and ceiling fields per trigger
  - meters: zone, raw level, threshold and ceiling marks, output, and "x dB | y% out"
- [ ] **Response:** gain 0.1–8, curve 0.2–5, attack and release, idle glow 0–100%. **Fix:** attack and release limits come from the schema (2000/5000 ms).
- [ ] **LEDs:**
  - first and last LED, "x of N LEDs"
  - mode (10) with help text; direction (4)
  - peak hold (fill only)
  - speed 0–2000 on a power-2.5 slider (6 moving modes)
  - size 1–100% (5 modes, not fire), labelled per mode (Trail, Drop size, Eye size, Bar size, Blob size)
- [ ] **Colours:** chips, remove (only with 2 or more), add (up to 16, duplicating the last), 9 palette presets, reverse.
- [ ] Footer: Duplicate (also Ctrl/⌘+D) and Delete.
- [ ] Switching the trigger to hits sets thresholds 4/12; switching to level uses the average plus the offset. Either way the inspector rebuilds.
- [ ] **New:** fields are generated from the effect's parameter schema.

## 8. Colours by range

- [ ] 6 rows. A segment belongs to a row if √(lo·hi) falls in [lo, hi).
- [ ] The row's palette is seeded from its first segment. Edits apply to every segment in the range. Shows a "mixed" note. Doesn't re-render while focus is inside it.

## 9. Reactive presets

- [ ] Tiles: animated preview, name, "Hz · description", tempo badge.
- [ ] Scopes: selected, all, new. "Keep range" switch for the selected scope. Matching tiles are pressed. Note text.
- [ ] Apply: response and range per scope, as today. **New:** offers Undo.

## 10. Settings

- [ ] Audio source picker with labels such as "Default output (what you hear) · X" and "· not found"; Rescan.
- [ ] Output connection settings (generalised per provider), background colour, LED count, reload layout (generalised).

## 11. Shell behaviours

- [ ] Collapsible cards: arrow buttons with `aria-expanded`. Keys: effects, spectrum, strip, segments, colors, reactive, inspector, settings. Settings starts collapsed. State remembered (`studio.collapsedCards`).
- [ ] Toasts: one slot; 3.2 s, or 9 s when there's an action.
  - **Fix:** a hidden Undo must not stay clickable.
  - **Fix:** a new toast must not silently replace a pending Undo; queue it instead.
- [ ] Two-step confirm buttons keep their size when armed.
- [ ] Keyboard shortcuts are ignored while typing: Delete, Escape (deselect), Ctrl/⌘+D, `[` and `]`.
- [ ] Layout: three columns, two at ≤1500 px, one at ≤1100 px, compact at ≤640 px. The sticky menu offset is measured from the top bar's height. Preview animations stop under reduced motion.
- [ ] Never use `innerHTML` for user content.

## 12. Content to carry over

- 9 palettes
- 20 reactive presets in 4 groups
- 12 effects, 18 LedFx-inspired effects (rewritten clean-room), 14 genre presets with layouts (alternate, mirror, zones, stack)
- 8 categories, 7 bands, 6 colour ranges, genre tags

All move into packs ([content](../architecture/content.md#built-in-library)).
