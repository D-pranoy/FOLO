# Glossary

Status: Accepted

| Term | Meaning |
|---|---|
| **Node** | The FOLO daemon process (`folo`). It runs in one of two roles: controller or source. |
| **Controller** | A node in the controller role. It owns zones, targets, providers, scenes and the library, manages sources, projects surfaces onto targets, and serves the API and UI. It is the only component that listens on the network. |
| **Source** | A node in the source role, together with its source workers. It runs inputs and effects and publishes surfaces to its controller over an outbound link. A controller can also run a local source alongside itself. |
| **Source worker** | A process supervised by a node that does source work: the effect engine (`folo-source`) and the display capture agent (`folo-capture`). |
| **Input** | Something a source reads: an audio device, a screen, a generator, or a local ingest stream. |
| **Effect** | A transformation from inputs to a surface layer, with declared parameters. Example: an audio visualizer, or the Screen effect. |
| **Layer** | One effect instance on a surface, with its parameters, region and blend mode. A surface's layers form a layer stack. |
| **Surface** | A virtual canvas that effects draw on: a *strip* (1D) or a *canvas* (2D). Surfaces are placed in a zone and projected onto targets. |
| **Zone** | A spatial setup such as a room or TV corner: a coordinate space, the targets in it, the surfaces placed in it and the active scene. Comparable to a Hue entertainment area. |
| **Target** | A real light, or part of one, placed in a zone: a *point* (one element), a *line* (1D, N elements) or a *grid* (2D, W×H elements). A *display* target kind is reserved for projection mapping. |
| **Element** | One addressable unit of a target: an LED, a lamp, a gradient segment. |
| **Projection** | Mapping each target element to a sample position on a surface and sampling it. |
| **Provider** | An output backend that talks to devices: Hyperion, WLED, Philips Hue. |
| **Capability** | What a target can do: colour model, addressing, delivery mode, rate, colour range. Adapters convert surface colours to each target's capabilities. |
| **Scene** | Layer stacks for a zone's surfaces, with macros. What the user picks to change the look. |
| **Look** | One layer's full recipe: effect, parameters, reaction and palette. |
| **Reaction** | How a layer responds to audio: band, trigger, thresholds, gain, curve, attack and release. |
| **Trigger** | What drives a layer: **level** (band loudness), **hits** (sudden peaks above the band's 1 s average), or **beat** (the beats the source finds, every ½, 1, 2 or 4 beats, aligned to the bar). |
| **Beat tracker** | The source component that finds tempo, beats and the bar from the audio, and publishes them as the beat signal. |
| **Reactive mode** | A zone's choice between **On** (follow the audio), **Auto** (follow the audio, ambient light after silence or on pause) and **Off** (ambient light only). |
| **Ambient light** | A zone's steady or breathing colour, shown when the reactive mode calls for it. |
| **Now playing** | Track metadata, album art and player controls from the source machine's media sessions (Windows) or MPRIS (Linux). |
| **Album colours** | A zone option that replaces layer palettes with colours picked from the current album art. |
| **App capture** | An audio input that listens to only one app, or to everything except one app. |
| **Auto level** | An automatic input gain that keeps quiet and loud music near the level the presets were tuned on. |
| **Pack** | A versioned bundle of content (palettes, reactions, looks, scenes, zone templates) that can be installed, updated and removed. |
| **Client** | Anything that manages a controller through its API: the desktop app, a browser, the future Android app, the Home Assistant integration. |
| **Desktop app** | The Tauri application. It hosts the UI, keeps the list of controllers, shows the tray icon and manages the local node. |
| **Invite** | A one-time link or QR code issued by a controller that lets a source join it. |
| **Client key** | A per-client, revocable API key issued by a controller. |
| **Managed launch** | A node started by the desktop app, which reads only its config file. |
| **Foreground launch** | A node started from the CLI, a service manager or Docker, where environment variables and flags override the config file. |
| **Deployment override** | A setting fixed by an environment variable or CLI flag. The UI shows it as locked. |
