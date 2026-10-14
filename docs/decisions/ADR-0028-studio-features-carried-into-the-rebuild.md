# ADR-0028: Studio features carried into the rebuild

Status: Accepted
Date: 2026-10-13

## Context

After the first documentation set was written, the current studio gained:
- **Its own WASAPI audio input** (cffi): output loopback, inputs, **only one app** or **everything except one app** (process loopback, Windows 10 2004+), following default-output changes and app restarts, with live stats and meters.
- **Input gain and auto level.**
- **Beat tracking:** tempo, beats and bar found from the audio. A **Beat** trigger flashes every half beat, beat, two beats or bar, with a timing offset. Five "On the beat" reactive presets, and the Beat sync and Half-time pulse effects.
- **Now playing:** Windows media sessions (title, artist, album art), player controls, album colours on the LEDs, and the ambient light on pause.
- **Audio-reactive modes** (On, Auto, Off) and an **ambient light** (solid or breathing colour after a configurable silence).
- **A lighter engine and page:**
  - quiet windows skip the FFTs
  - per-configuration preparation
  - Hyperion only gets changed frames, plus a 0.5 s keepalive
  - the page draws only on invalidation and closes its stream when hidden
- **A glass visual design** ([ADR-0029](ADR-0029-visual-design-language.md)).

Measured per-packet engine time roughly halved (see [pipeline](../architecture/pipeline.md#performance-budget)).

## Decision

- **D1.** All of these features are **in scope for version 1** and part of feature parity ([UI parity checklist](../specs/ui-parity-checklist.md)).
- **D2.** **Placement in the new architecture:**
  - **Effect engine (source):**
    - audio inputs, including app capture
    - gain and auto level
    - the analyzer and its quiet-window skip
    - the **beat tracker**, published as a beat signal: BPM, confidence, lock, phase, bar beat
    - the `beat` trigger in audio-visualizer layers
    - **now playing** (media state, album art, cover colours, player controls)
  - **Controller:**
    - the **reactive mode and ambient light per zone**, implemented in the compositor as the zone's idle look with the current fade and breathe behaviour, driven by source activity and the media pause state
    - the beat timing offset per zone, in addition to per-output latency offsets
- **D3.** **Platform backends for v1:**

  | Platform | Audio capture | Now playing |
  |---|---|---|
  | Windows | FOLO's own WASAPI capture: output loopback, inputs, process loopback for app capture | Media sessions through the `winrt` packages (Windows 10 1809+) |
  | Linux | PulseAudio/PipeWire monitors and inputs | **MPRIS** over the session D-Bus |

  Per-app capture on Linux (through PipeWire stream targets) is a later item.
- **D4.** **Session constraint:** media sessions (and MPRIS) belong to the signed-in user's session. In Windows service mode, now playing is unavailable in v1, and the UI says so. Moving media integration into the user-session agent is a later option. Whether per-app capture works from a service is verified in RB-20.
- **D5.** **Every provider sends only changed frames**, with a keepalive inside the device's timeout: Hyperion 0.5 s; WLED within its 2.5 s realtime timeout; Hue at least every 5 s within its 10 s stream timeout.
- **D6.** **Parity:**
  - The reactive output stays byte-identical ([ADR-0005](ADR-0005-pipeline-and-data-model.md) D8).
  - Ambient-light frames, now computed by the controller, must match the current engine within ±1 per channel.
  - The beat tracker is pinned by golden tests on synthetic songs: beats within ±70 ms of truth, bar starts found, and **no lock** on music without a steady beat.
- **D7.** Testing rules from the studio's practice are adopted ([testing policy](../process/testing-policy.md)):
  - synthetic songs for the beat tracker
  - an inaudible tone from a renamed process for app capture
  - **never pressing player controls** on someone's real player

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Ambient light in the effect engine | Byte-exact parity is easy | Conflicts with switching between sources; can't show without a source |
| Beat tracking on the controller | One tracker per zone | Sources would have to send analysis data; contradicts sources running effects |
| Drop now playing on Linux | Less work | Linux is Tier 1; MPRIS is standard and simple |

## Consequences

- The configuration model gains zone `reactive`, `ambient`, `beat_offset_ms` and media options.
- Source inputs gain app capture, gain and auto level.
- Layers gain the `beat` trigger and `beats`.
- The source link gains beat and media messages.
- RB-03, RB-04, RB-05, RB-08 and RB-11 include these features.
