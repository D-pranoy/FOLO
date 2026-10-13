# ADR-0011: Hyperion provider

Status: Accepted
Date: 2026-10-13

## Context

Today's studio drives a WLED strip through Hyperion.NG's JSON API. Hyperion also knows its LED layout as rectangles on a screen.

## Decision

- **D1.** Keep the JSON API (TCP, `color` command, `duration` 2000 ms, configurable priority and origin of at most 20 characters). FlatBuffers is a later option.
- **D2.** Describe each Hyperion instance as a line target. Element positions come from `serverinfo` LED rectangles placed on the zone's TV plane.
- **D3.** Follow layout changes, scaling mappings proportionally.
- **D4.** Basic administration as listed in [Hyperion](../integrations/hyperion.md#basic-administration).
- **D5.** Migrated studio configurations use this provider in parity mode.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| FlatBuffers now | More efficient | Not needed at current sizes |
| Drop Hyperion | Simpler | Existing setups depend on it |

## Consequences

Parity tests use a fake Hyperion JSON server ([testing policy](../process/testing-policy.md)).
