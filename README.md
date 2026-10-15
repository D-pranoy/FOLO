FOLO
====

**Rebuild in progress.** FOLO is being rebuilt into a modular, spatial lighting-management system:
- a controller driving Philips Hue, WLED and Hyperion
- sources on each computer for audio and screen sync
- Hue-style zones
- a desktop app for Windows and Linux

The rebuild lands on the `v1` branch; `main` is the frozen legacy studio ([ADR-0030](docs/decisions/ADR-0030-rebuild-on-a-v1-branch.md)). The vision, roadmap, decisions and specifications live in [docs/](docs/README.md) and are the source of truth. The sections below describe the current studio. To contribute, see [CONTRIBUTING.md](CONTRIBUTING.md); report security problems privately as described in [SECURITY.md](SECURITY.md).

**Primary contributors:** Pranoy Das, Sayak Brahmachari.

A Windows / Python 3 fork of [RanzQ/hyperion-audio-effects](https://github.com/RanzQ/hyperion-audio-effects), with *Fear of lights out*: a live audio-reactive LED studio for Hyperion.NG. The upstream effect runner, ported to Python 3, is no longer on `v1`; it stays on `main` and in the `v0.9.0-legacy` tag ([ADR-0023](docs/decisions/ADR-0023-legacy-code-and-licensing.md)).

### FOLO studio (Windows, Hyperion.NG)

A web UI for building audio reactive segments: split the strip into LED ranges, give each one a frequency range, a threshold and a full brightness level, and see the result live.

Setup (Python 3.13): `uv sync`, or without [uv](https://docs.astral.sh/uv/) `py -3.13 -m venv .venv`, then `.venv\Scripts\python.exe -m pip install -r requirements.txt` (the `winrt` packages read what's playing; the studio runs without them). [CONTRIBUTING.md](CONTRIBUTING.md) has the development commands. By default the studio listens to what plays on Windows' default output and analyses each 10 ms packet as it arrives. Every channel that carries sound counts, and Hits triggers use a shorter FFT window, so they react 10–20 ms sooner than level triggers. Pick it in the Audio input panel. With an audio router such as SteelSeries Sonar, pick the physical device it outputs to, so the lights follow the processed mix.

Audio input: the lights can follow everything you hear, one output, an input, **only one app** (say your music player, so game sounds and notifications leave them alone) or **everything except one app**. App capture needs Windows 10 2004 or later.
- **Following changes:** the studio switches by itself when Windows' default output changes, waits for an app to start and follows it when it restarts, and reconnects unplugged devices.
- **Panel:** live L/R meters with a clip light, the capture format, packets per second, delay and dropouts.
- **Level:** a gain trim, and **Auto level**, which keeps quiet and loud songs at a similar strength so level triggers work at any volume.

1. Run `studio.bat` (or `.venv\Scripts\python.exe studio.py`). It opens http://127.0.0.1:8777/ in your browser.
2. On the spectrum, drag a box's sides to change its frequency range, its solid line to set the threshold and its dashed line to set full brightness. Drag empty space to add a segment.
3. On the LED track, drag segments to move them, their ends to resize, or an empty lane to create one. + Lows, + Mids and + Highs above the track add a segment that reacts like the others of its kind, in free space or in the right half of the selected segment. Delete removes the selected one. Both can be undone. Drag a segment up or down to put it on another row (the empty row at the bottom starts a new one). Overlapping segments mix, and the brightest wins.
4. Fine tune in the side panel: gain, curve, attack/release, idle glow, mode, direction and colors. Fill, pulse, spectrum and color shift follow the level directly. Scroll, drops, scan, wipe, flow and fire move by themselves (after LedFx effects) and get their own speed and size settings.
5. Pick a trigger per segment. **Level** lights when the band is louder than the threshold. **Hits** lights on sudden peaks above the band's average over the last second, so fast flicker keeps working across songs and volumes. **Beat** flashes on the beat of the music, every half beat, beat, two beats or bar, as bright as the band is loud (dark below its threshold).

The brightness bar dims the whole strip on a perceptual scale (also `[` and `]`). The studio follows Hyperion's LED count by itself and scales the segments when it changes.

Audio reactive (top bar): **On** follows the music. **Off** shows a steady ambient light instead. **Auto** follows the music and fades to the ambient light once it has been quiet for a while (5 s by default), or as soon as you pause your player, then snaps back on the first sound. The Ambient light card sets its color (Deep Night swatches or any color), Solid or a slow Breathe, its brightness and the Auto delay. It keeps glowing even when no audio device is delivering sound.

Music: the studio finds the tempo and the beats of what's playing by itself (the top bar shows the BPM with a dot flashing on the beat), and reads what your player reports to Windows (Spotify, browsers, Media Player and others, on Windows 10 1809 or later). The top bar shows the song and its album art; the Music card adds play/pause, previous and next, and these options:
- **Album colors:** the LEDs take the main colors of the album art, the strongest for lows, the next for mids and highs. Your segments keep their own colors for when you switch it off.
- **On pause:** in Auto, the ambient light comes on as soon as you pause, instead of after the quiet delay.
- **Beat timing:** fires the beats up to 200 ms earlier (or later), if the lights trail the sound.

Beats lock on after about four seconds of music with a steady beat, a new song starts afresh, and music without a steady beat (ambient, speech) doesn't lock, so beat segments stay dark there.

Effects: the menu on the left groups whole-strip looks under umbrella categories: My presets, LedFx, Electronic, Hip-hop & R&B, Pop & Rock, Jazz & Classical, Chill & Ambient and Visualizers. The LedFx group (marked with an LedFx badge) holds ports of [LedFx](https://github.com/LedFx/LedFx)'s audio reactive effects: Energy, Scroll, Wavelength, Power, Rain, Spotlight, Scan, Scan multi, Strobe, Blade Power+, Bar, Multicolor bar, Bands, Equalizer, Blocks, Fire, Lava lamp and Marching. Bar and Multicolor bar move on the studio's own beat tracking, and Beat sync (Electronic) and Half-time pulse (Hip-hop & R&B) are built on it too. The Reactive presets card has an "On the beat" group. Genre presets (EDM, Techno, Drum & Bass, Dubstep, Trap, Hip-hop, Pop, R&B, Rock, Metal, Jazz, Classical, Lo-fi, Ambient) give each frequency range its own reactive preset. Hover an effect to see its preview move, click it to apply it, use ▾ for its ranges and tags, search by name, genre or tag, save your own with "Save current" (name and tags, editable later), and star one as the default that Reset all restores. Colors by range recolors every segment in a part of the spectrum at once.

The panels are frosted glass over a Deep Night glow, and the page only redraws when new audio or input arrives, so it stays light on the CPU. A page in a background tab stops streaming until you come back to it. The studio itself skips the analysis in silence and only sends Hyperion frames that changed (plus a refresh every half second). Every panel folds away with the arrow next to its title, and the browser remembers which panels are folded. Settings starts folded.

Segments and saved presets are saved to `studio.json`. Stop the studio with Ctrl+C or by closing its window; Hyperion releases the LEDs within two seconds. `studio.py --help` lists the options.

### Contribute

Contributions are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md).
