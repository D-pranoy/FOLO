# ADR-0014: Device administration scope

Status: Accepted
Date: 2026-10-13

## Context

Users want to manage their lights from one place. Each vendor has a full administration UI already; reproducing those is endless work.

## Decision

- **D1.** FOLO provides **basic administration** for each provider, limited to what setup and day-to-day use need:
  - [Hue](../integrations/hue.md#basic-administration)
  - [WLED](../integrations/wled.md#basic-administration)
  - [Hyperion](../integrations/hyperion.md#basic-administration)
- **D2.** Deeper features link to the device's own UI: WLED effects, presets and firmware; Hue scenes and automations; Hyperion device configuration.
- **D3.** Every administrative action states what it will change on the device before doing it.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| No administration | Least work | Users bounce between apps for basics |
| Full administration | One app for everything | Endless; duplicates vendors' work |

## Consequences

Administrative endpoints live under `/api/v1/providers/{id}/admin` and need the `admin` scope.
