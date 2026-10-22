# ADR-0033: The flash limiter is off by default, with epilepsy warnings

Status: Accepted
Date: 2026-10-22

## Context

[ADR-0007](ADR-0007-capabilities-adapters-and-flash-limiter.md) D4 turns the flash limiter on for every zone, limiting large luminance transitions to at most 3 per second (WCAG 2.3.1), and lets a zone opt out with a recorded reason. Two things conflict with that default:
- **Today's looks flash faster.** The beat trigger on half beats, Strobe, Bar and similar effects make more than 3 large transitions a second at common tempos. With the limiter on, a migrated setup no longer looks as it did.
- **Parity.** M3's exit criterion is that a migrated setup drives a fake Hyperion byte-identically, and parity mode is defined as an identity chain with no limiter ([pipeline](../architecture/pipeline.md#parity-mode)). The configuration model has no parity mode, so a migrated zone is an ordinary zone, with the limiter on.

The limiter is still worth having, for people with photosensitive epilepsy and for third-party packs and screen content that FOLO doesn't control. The question is who turns it on.

## Decision

This supersedes ADR-0007 D4. ADR-0007's other decisions stand.

- **D1.** Every zone has a flash limiter setting, `flash_limiter.enabled`, **off by default**. Switched on, it works as ADR-0007 describes: after rendering, it limits large luminance transitions to at most 3 per second (WCAG 2.3.1), for all content, including third-party packs and screen sync.
- **D2.** `flash_limiter.exempt_reason` is removed. An opt-out reason has no meaning when off is the default. The schema has not been released, so this and the new default need no contract version bump. The clamper drops the key from older files.
- **D3.** Migrated zones take the default, so they keep today's look, and parity mode needs no setting of its own.
- **D4.** FOLO shows a **photosensitive epilepsy warning**:
  - on the first run, before any light is driven, with a one-tap "Turn on the flash limiter for all zones"
  - beside each zone's flash limiter switch while it's off
  - in Settings, with a switch for all zones
  - as a "Flashing" badge on built-in effects that flash quickly (the beat trigger on half beats or every beat, Strobe, Bar and similar)
  - in the README and SECURITY.md
- **D5.** The warning text, to be used everywhere it appears in full:

  > **Photosensitivity warning.** Lights driven by FOLO can flash quickly and in patterns. For a small number of people, flashing lights can trigger seizures, including in people with no history of epilepsy. Stop using FOLO and see a doctor if you or anyone watching feel dizzy, have altered vision, eye or face twitching, jerking or shaking, disorientation or loss of awareness. If you or anyone in your household has had a seizure or epilepsy, talk to a doctor before use, and turn on the flash limiter, which keeps large brightness changes to 3 a second or fewer.

- **D6.** The flash limiter failing to act while it's on, or turning off without the user, counts as a security problem.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| On by default (ADR-0007 D4) | Safe without anyone choosing it | Changes today's looks on migration; breaks M3 parity unless migrated zones opt out |
| On by default, migrated zones opt out automatically | Keeps today's look for upgrades | Upgraded setups lose the default without anyone choosing that; two defaults to explain |
| A parity-mode setting that turns the limiter off | Explicit | A contract change for a test concern; the same safety trade-off under another name |
| No limiter | Simplest | No protection for people who need it |

## Consequences

- People who need the limiter must turn it on. The warnings in D4 are how they learn that it exists, so the first-run warning is always shown, even when the rest of the first-run wizard is skipped.
- Third-party packs and screen sync can flash at full rate until a user turns the limiter on.
- The M3 parity test runs on migrated zones as they are.
- Documents that change: [capabilities](../architecture/capabilities.md), [security](../architecture/security.md), [configuration model](../specs/config-model.md), [user interface](../architecture/ui.md), the [UI parity checklist](../specs/ui-parity-checklist.md), [vision](../vision.md), [M3](../runbooks/m3-controller-core.md), README.md and SECURITY.md.
- Contracts: `FlashLimiter` in `specs/schema/zones.json` loses `exempt_reason`, and `enabled` defaults to false. The migration goldens and the clamp corpus follow.
- Live device tests still turn the limiter on ([testing policy](../process/testing-policy.md)).
