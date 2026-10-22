# Security policy

## Reporting a vulnerability

Report security problems **privately**, through GitHub's private vulnerability reporting: open the repository's **Security** tab and choose **Report a vulnerability** ([direct link](https://github.com/D-pranoy/FOLO/security/advisories/new)). Don't open a public issue, pull request or discussion for them.

Include:
- what an attacker can do, and from where (the same machine, the LAN, a web page, a content pack)
- steps to reproduce or a proof of concept
- the version or commit you tested

Leave out real device addresses, keys and personal data.

Maintainers confirm the report, prepare the fix in a private security advisory, and credit you in the advisory unless you ask not to be named.

## Supported versions

FOLO is being rebuilt (see the [roadmap](docs/roadmap.md)). During the rebuild, security fixes are made on `v1` and ship in the next release; only the latest release is supported. The legacy studio on `main` (`v0.9.0-legacy` and earlier) gets no fixes ([ADR-0030](docs/decisions/ADR-0030-rebuild-on-a-v1-branch.md)).

## Threat model in brief

The full model is in [docs/architecture/security.md](docs/architecture/security.md).

**What matters:** control of the lights (physical safety, including photosensitivity), device credentials (Hue keys, WLED PINs, Hyperion tokens), the paths that install code or content (updates and packs), and the privacy of captured screens and audio.

**The current studio** is a single-machine tool:
- it listens on 127.0.0.1 only
- it refuses requests whose `Host` header isn't local, against DNS rebinding
- it accepts only `application/json` POSTs, against cross-site form posts
- it has no authentication: any program on the same machine can control it

**The rebuilt system** is designed so that:
- remote access is off by default, and LAN-only when on, with per-client revocable API keys; loopback clients need no key
- UI traffic is plain HTTP by decision ([ADR-0009](docs/decisions/ADR-0009-client-access.md)), so a client key can be sniffed on the LAN: this is a known, documented limitation
- source links between computers are Noise-encrypted, with keys pinned at pairing; invites are one-time and expire
- updates are signature-checked against a key built into FOLO, and content packs are data only, validated and clamped
- a flash limiter can be turned on per zone (it is off by default), and photosensitive epilepsy warnings say so on first run and wherever it can be switched
- screens and audio are captured on demand only; full frames and raw audio are never stored or sent

## In scope

FOLO's own code: the studio server and UI, the effect engine, and, as they land, the node, its API, pairing, source links, updates, content packs and installers. The flash limiter failing to act while it's on, or turning off without the user, counts as a security problem.

## Out of scope

- Vulnerabilities in Hyperion, WLED, Philips Hue bridges or other third-party software: report them to their projects. A way FOLO makes them exploitable is in scope.
- Attacks that already need administrator rights or physical access to the machine.
- Sniffing plain HTTP UI traffic on the LAN (see above), unless it goes beyond what [ADR-0009](docs/decisions/ADR-0009-client-access.md) accepts.
