# ADR-0022: Home Assistant integration strategy

Status: Accepted
Date: 2026-10-13

## Context

A future Home Assistant integration should preferably expose FOLO's **effects and effect controls**, and only otherwise act as a lighting source. Comparable projects show how:
- WLED's core integration (Platinum): light with an effect list, select, number, switch, sensor, button and update entities
- Hyperion's integration, with approval-based token pairing

HA isn't suited to streaming pixel data: its recorder logs every state change, and its WebSocket API drops slow clients.

## Decision

- **D1.** **Phase 0, in v1: HA readiness.**
  - Stable controller and zone ids; mDNS TXT `id` and `api`.
  - Full-state WebSocket push; an effect catalog with parameter schemas.
  - Approval-based pairing with scoped keys; OTA summaries.
  - An async Python client library published to PyPI from CI.
- **D2.** **After v1, in phases:**
  1. A HACS integration, WLED-style: a device per zone, a light with scenes as effects, numbers, selects, switches, sensors, buttons, update entities and actions; Hyperion-style approval pairing.
  2. HA context as effect inputs.
  3. An HA app for hosting the controller.
  4. A core submission.
- **D3.** HA is **never used as a pixel source**. HA lights as slow FOLO outputs, an MQTT discovery bridge and a Matter bridge are optional later items.
- **D4.** FOLO zones map to HA **devices**, because HA's "zone" means a geographic area. Update entities can install only with an admin-scoped key.

## Alternatives considered

See the comparison in [Home Assistant](../integrations/home-assistant.md#options-considered).

## Consequences

The client API must stay stable and versioned. The client library becomes a published artifact of the release pipeline.
