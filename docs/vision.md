# Vision

Status: Accepted

## Purpose

FOLO ("Fear of lights out") aims to be **the complete lighting-management system**: a single place to set up, map and drive every light in a home or venue, reacting to music, screens and other inputs, across vendors.

It grew from an audio-reactive LED studio for Hyperion. It is being rebuilt as an end-to-end, modular lighting pipeline:

**inputs → effects → surfaces → spatial projection → targets → providers**

## Goals

1. **One pipeline for every light.** Point lights, strips and matrices, from any supported vendor, placed in real space and driven by one consistent model.
2. **Many inputs.** Audio and display capture on each computer, generators, and external data streams, combined freely.
3. **Spatial by design.** Zones (rooms, TV corners, desks) where lights sit at real positions, so effects look right regardless of the hardware mix.
4. **Simple first, granular always.** Setting up should feel as easy as creating a Hue entertainment zone in the Hue app. Every parameter must still be reachable for those who want it.
5. **Distributed.** One controller drives the lights. Any number of computers contribute sources. Any number of clients (desktop, browser, later Android) manage it.
6. **Professional quality.** Tested, typed, documented, reproducibly built and signed releases, safe updates.
7. **Extensible.** New providers, inputs, effects and integrations plug in without changing the core.

## Principles

- **Documentation is the source of truth.** The software implements the documents ([ADR-0001](decisions/ADR-0001-documentation-is-the-source-of-truth.md)).
- **Setup-agnostic.** No feature assumes a particular home, device mix or ecosystem. Everything is configurable and can be turned off.
- **Simple by default, full granularity on demand.** Progressive disclosure, never hidden capability.
- **Local-first, LAN-only.** No cloud dependency. Remote access means the local network.
- **Lighting data, not media.** Sources send lighting data (surfaces) to the controller. Raw audio and full video frames never leave the computer that captured them.
- **Safe by default.** Photosensitive epilepsy warnings point to a flash limiter, one switch away. Devices are released when FOLO stops. Updates install only after approval.
- **Touch-first.** Every screen works on a phone, a tablet and a desktop.
- **Contracts first.** Interfaces between components are specified before they are built, and conformance is tested.

## Non-goals

- A cloud service, accounts or telemetry.
- Replacing vendors' own administration tools beyond the basic management FOLO needs (see [ADR-0014](decisions/ADR-0014-device-administration-scope.md)).
- Relaying audio between machines.

## Landscape

FOLO covers ground that is split across several tools today: audio-reactive LEDs (LedFx), screen sync (Hyperion, HyperHDR, Hue Sync), vendor apps (Hue, WLED), PC RGB (OpenRGB, SignalRGB) and, eventually, projection (MadMapper, Resolume). Its distinguishing features are spatial zones across vendors, several sources feeding one controller, and Hue-simple setup.
