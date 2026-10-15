# ADR-0031: Linux audio capture in the effect engine

Status: Proposed
Date: 2026-10-15

## Context

[ADR-0028](ADR-0028-studio-features-carried-into-the-rebuild.md) D3 decides what the effect engine captures on Linux in v1: PulseAudio/PipeWire monitors and inputs, without per-app capture. It doesn't say how the Python engine reaches them, and RB-04 step 2 needs that.

- **Audio servers on the supported platforms ([platform and packaging](../architecture/platform-and-packaging.md#supported-platforms)):**
  - Tier 1 (Ubuntu 24.04, Arch and its derivatives) run PipeWire. Its PulseAudio server, `pipewire-pulse`, is a drop-in replacement for the PulseAudio daemon.
  - Tier 2 includes Ubuntu 22.04, which runs PulseAudio itself.
  - A client of the PulseAudio API therefore works on every supported Linux. Recording a sink's monitor works through `pipewire-pulse` too.
- **Following the default output:** PulseAudio clients learn of a new default sink from server change events, then read the server info again. Those events aren't sent for every change, so a client also checks now and then. That is the same shape as the Windows capture: device notifications plus a 10 s safety poll.
- **The Windows capture** is FOLO's own: WASAPI through cffi in ABI mode, with no compiled extension ([current system](../architecture/current-system.md#audio-input)). cffi is already a dependency.
- **Where it runs:** the node's systemd user unit runs in the user's session, so it reaches the user's audio server. The system unit for headless controllers runs no local source ([platform and packaging](../architecture/platform-and-packaging.md#services)).
- `libpulse` is LGPL-2.1-or-later. Loading it at run time keeps FOLO's own code under its licence ([ADR-0023](ADR-0023-legacy-code-and-licensing.md)).

## Decision

- **D1.** **On Linux the effect engine captures audio through the PulseAudio client library** (`libpulse.so.0`), loaded at run time with cffi in ABI mode, as the WASAPI capture is on Windows. It serves PulseAudio and PipeWire (through `pipewire-pulse`) alike.
- **D2.** **Sources** ([audio inputs](../architecture/pipeline.md#audio-inputs)):
  - `default` records the monitor of the default sink.
  - An output's name records that sink's monitor.
  - `in:<name>` records that source.
  - The stream is 48 kHz 32-bit float, in the device's channel layout, as the analyzer gets on Windows.
- **D3.** **Following:** the engine subscribes to server, sink and source events. It reopens the stream when the default sink changes (for `default`) or its device goes away, and checks the server's default sink every 10 s in case an event was missed. A device that isn't there is waited for, and a lost server connection is retried, with the Windows capture's retry delays.
- **D4.** **Packets:** a capture thread runs the library's threaded main loop and drains everything the stream holds on each wake, in packets of about 10 ms. As on Windows, a gap of more than 0.1 s starts a fresh analysis window. The input reports the same stats: packets/s, delay (the stream's latency), glitches (overflows) and the peak meters.
- **D5.** **App capture stays Windows only** in v1. On Linux the input ignores `audio.app` and reports that app capture needs Windows.
- **D6.** **Dependencies:** cffi becomes a runtime dependency on Linux as well. The Linux packages depend on the PulseAudio client library: `libpulse0` (deb), `pulseaudio-libs` (rpm) and `libpulse` (Arch). The AppImage uses the system's library.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| PipeWire's own API (`libpipewire-0.3`) through cffi | Native to the Tier 1 audio server; the way to per-app capture later | Misses PulseAudio-only systems such as Ubuntu 22.04; a much larger callback API (SPA pods, stream events) for the same v1 features |
| The `soundcard` package | Ready-made, cffi over libpulse | A third-party layer over the same library; FOLO removed it with the legacy runner; it doesn't follow default-sink changes |
| `pulsectl` plus `pasimple` | Small ctypes packages | Two dependencies for one input; the simple API gives no events or timing for stats |
| PortAudio (`sounddevice`) | One API on every platform | Can't pick PulseAudio monitors reliably; adds a native library |
| GStreamer (`pulsesrc` / `pipewiresrc`) | Handles both servers | A large dependency for one input |

## Consequences

- RB-04 step 2's Linux part implements D1–D5 in `folo_source/inputs/audio/linux/`, next to the Windows capture, behind the same interface.
- **Documents:** [pipeline](../architecture/pipeline.md#audio-inputs), [platform and packaging](../architecture/platform-and-packaging.md#linux-packages), [RB-04](../runbooks/m2-engine.md#rb-04-source-pipeline-and-parity) and [RB-21](../runbooks/m8-release.md#rb-21-linux-packaging) name the library and the package dependencies.
- **Testing:** CI runs the Linux capture against a PulseAudio server with a null sink. A tone played into the sink must arrive through its monitor, and a default-sink change must reopen the stream. No real audio device is needed ([testing policy](../process/testing-policy.md)).
- **Per-app capture on Linux** (a later item, ADR-0028 D3) will need PipeWire's own API, through PipeWire stream targets. This decision doesn't block it.
