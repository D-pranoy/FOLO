FOLO
====

**Rebuild in progress.** FOLO is being rebuilt into a modular, spatial lighting-management system:
- a controller driving Philips Hue, WLED and Hyperion
- sources on each computer for audio and screen sync
- Hue-style zones
- a desktop app for Windows and Linux

The vision, roadmap, decisions and specifications live in [docs/](docs/README.md) and are the source of truth. The sections below describe the current studio. To contribute, see [CONTRIBUTING.md](CONTRIBUTING.md); report security problems privately as described in [SECURITY.md](SECURITY.md).

**Primary contributors:** Pranoy Das, Sayak Brahmachari.

A Windows / Python 3 fork of [RanzQ/hyperion-audio-effects](https://github.com/RanzQ/hyperion-audio-effects), with *Fear of lights out*: a live audio-reactive LED studio for Hyperion.NG. The original project's notes follow.

##### Examples: https://www.youtube.com/watch?v=zLPZ6Lzbmgc https://www.youtube.com/watch?v=AOv9BwbtA6w

Dev kit for creating audio visualizations for [hyperion](https://github.com/tvdzwan/hyperion).

- Update 27/3/2016: Config file (config.json), matrix configurator, protobuf support
- Update 8/4/2015: Some improvements and cleanup, new video
- Update 31/3/2015: New parameters and hyperion config file parsing

### FOLO studio (Windows, Hyperion.NG)

A web UI for building audio reactive segments: split the strip into LED ranges, give each one a frequency range, a threshold and a full brightness level, and see the result live.

Setup (Python 3.13): `py -3.13 -m venv .venv`, then `.venv\Scripts\python.exe -m pip install -r requirements.txt` (the `winrt` packages read what's playing; the studio runs without them). By default the studio listens to what plays on Windows' default output and analyses each 10 ms packet as it arrives. Every channel that carries sound counts, and Hits triggers use a shorter FFT window, so they react 10–20 ms sooner than level triggers. Pick it in the Audio input panel. With an audio router such as SteelSeries Sonar, pick the physical device it outputs to, so the lights follow the processed mix.

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

The original effects also run on Windows: `run.bat --effect color_spectrum` or `run.bat --effect vumeter` (`--list-devices` lists audio sources). They read the LED layout from the running Hyperion.NG and take a `palette` list in `effects/*.json`.

### Installation and running on Debian **

1. Install Gstreamer 1.0 and PyGI: `sudo apt-get install libgstreamer1.0-0 gir1.2-gstreamer-1.0 gir1.2-glib-2.0 gir1.2-gst-plugins-base-1.0 gstreamer1.0-plugins-good gstreamer1.0-tools gstreamer1.0-alsa gstreamer1.0-pulseaudio python-gi`
2. Install Tkinter for GUI: `sudo apt-get install python-tk`
3. Install pip: `sudo apt-get install python-pip`
4. Install dependencies: `sudo pip install -r requirements.txt` (or use virtualenv if you like)
5. Put `options snd-aloop index=-2` in end of `/etc/modprobe.d/alsa-base.conf` to prevent loopback device for getting first card index
6. Enable loopback device `modprobe snd-aloop` (and type line `snd-aloop` to /etc/modules to make it permanent)
7. Put the included `.asoundrc` to your home folder (backup old if exists) and change the soundcard index if needed (`"hw:<card>,<device>"`, check `aplay -l`) *
8. Reboot or reload alsa `sudo alsa force-reload`
9. Run `python main.py` with options:
	- `--config=<path>` path to hyperion config file (defaults to `./hyperion.config.json`)
	- `--gui` for gui
	- `--json` for network connection (`--host=<ip> --port=<port>`)
	- `--help` to see all options
10. Play some audio
11. Levels should be drawn to gui, also sent to hyperion if json enabled
12. Exit by closing the GUI or Ctrl+c

### Installation and running on OSMC (RPi 2)

1. Enable audio so we can use alsa and loopback: Add `dtparam=audio=on` to `/boot/config.txt`.
  - Why? https://discourse.osmc.tv/t/alsa-doesnt-work-after-last-update/10600
2. Install Gstreamer 1.0, alsa, PyGI and pip: `sudo apt-get install libgstreamer1.0-0 gir1.2-gstreamer-1.0 gir1.2-glib-2.0 gir1.2-gst-plugins-base-1.0 gstreamer1.0-plugins-good gstreamer1.0-tools gstreamer1.0-alsa alsa-base alsa-utils python-gi python-pip`
3. Install git (`sudo apt-get install git`) and clone this repo (`git clone https://github.com/RanzQ/hyperion-audio-effects.git`), or just download as zip and extract
4. Install python dependencies `cd hyperion-audio-effects/ && sudo pip install -r requirements.txt`
5. Put `options snd-aloop index=-2` in end of `/etc/modprobe.d/alsa-base.conf` (the file doesn't exist, just create it) to prevent loopback device for getting first card index
6. Enable loopback device `sudo modprobe snd-aloop` (and type line `snd-aloop` to /etc/modules to make it permanent)
7. Put the included `.asoundrc` to your home folder and change the soundcard index on line 21 if needed (check `aplay -l`, for me `"hw:0,0"` works, this depends on if you use usb-audio etc.)
8. Reboot
9. Now you must choose how to play some music. Kodi/OSMC doesn't support the loopback setup and Spotify can't be installed (maybe possible soon, check [spotifyd](https://github.com/Spotifyd/spotifyd)), so I went with mpd and mpc `sudo apt-get install mpd mpc`.
  - To get audio working with `mpd`, I needed to copy the alsa config to be global `sudo cp .asoundrc /etc/asound.conf`
  - After adding some music to `/var/lib/mpd/music` run `mpc ls | mpc add` to add all files to playlist, then `mpc play` (check `mpc help` for all commands).
  - You can select between HDMI and Headphone jack with `amixer cset numid=3 2` (HDMI) / `amixer cset numid=3 1` (Headphone)
  - I didn't find a way to adjust the audio level that the effects receives, I've done that using Spotify
10. Finally you can try the audio-effects :)
  - `cd hyperion-audio-effects/`
  - `python main.py --effect vumeter`
  - The heavier effect works with setting `"band-width-exp": 4-5` but uses quite a lot of CPU. `python main.py --effect color_spectrum`
  - You can also modify `config.json` and just run `python main.py`
  - Exit by typing `x` and enter

### Effects development
1. Copy one of the script & config pairs in `effects/` (e.g. `myeffect.py` and `myeffect.json`) and then it can be passed as `--effect=myeffect` (json values can be read from `hyperion.args` like in normal hyperion effects)
2. Adjust gstreamer parameters
   - See `spectrum_dump.py` for explanation of parameters for GstSpectrumDump
   - Using `vumeter` and `bands` you can adjust the type of received magnitudes (`self.magnitudes`)
      * With `vumeter=True` you get 4 magnitudes which correspond to peaks and decays for the L/R channels
      * With `vumeter=False` you get spectrum magnitudes for the amount of `bands` (defaults to 128)
3. Update the leds by modifying the bytearray `self.ledsData` according to values in `self.magnitudes` (I've done it in method `update_leds()`)

### Effect: VU Meter
- Volume bars for left and right of your hyperion setup, or by setting indices manually
- Min and max volume and colors can be adjusted from config

### Effect: Color spectrum
- Leds strip is divided into octaves 1-8
- Low frequencies start from red, ending in pink at high frequencies
- Adjust your sound volume if the leds are too dim or bright (no volume normalization yet)
- Config has a field `band-width-exp`, if you have performance issues, try with higher values (it sets FFT bin size to 2^x Hz, e.g. 2^3 = 8 Hz, affects on bass accuracy)
- Set `mirror` to false if you want to use whole strip instead of splitting and mirroring
- Color range can be adjusted with `hue-from` and `hue-to` parameters (`0.0-1.0`)
- New config parameter `matrix`
  - Modified version for matrix setups, need to use protobuf connection instead of json
  - Example config.json for matrix:
```
 {
    "json": false,
    "proto": true,
    "effect": "color_spectrum",
    "config": "/etc/hyperion/hyperion.config.json",
    "host": "localhost",
    "port": 19445,
    "audiosrc": "autoaudiosrc",
    "interval": 50,
    "matrix": true
}
```

### Contribute

Pull requests are welcome but try to follow the style in existing code and check yours using [pylint](https://www.pylint.org/) (`.pylintrc` included).

\* Check this if you have pulseaudio: [#4](https://github.com/RanzQ/hyperion-audio-effects/issues/4#issuecomment-67764593)

\** Windows istructions were removed since performance was poor due to the fact that Gstreamer is meant for Linux
