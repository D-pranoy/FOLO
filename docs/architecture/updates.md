# Updates (OTA)

Status: Accepted · Implementation: Not started

Decided in [ADR-0020](../decisions/ADR-0020-ota-updates.md).

## Release channels

- **Stable:** tags `vX.Y.Z`.
- **Beta:** tags `vX.Y.Z-beta.N`. Betas are checkpoints towards the same stable version.

## Artifacts and signatures

- Every release artifact is signed with the **update signing key**, in the minisign-compatible format used by Tauri's updater, and listed with its SHA-256.
- Two manifests:
  - `latest.json` (Tauri format)
  - `folo-update.json`: every artifact by install type and architecture
- The public key is built into the node and the desktop app. Verification can't be turned off.
- Releases also carry `SHA256SUMS` and build-provenance attestations ([ADR-0019](../decisions/ADR-0019-ci-cd-versioning-and-releases.md)).

## Flow

1. **Check.** Each node checks GitHub Releases every 24 hours (with jitter) and on demand. Sources report what they find to their owner controller.
2. **Approve.** The controller's update policy decides what needs approval:
   - manual per node (the default)
   - automatic for patch releases
   - a maintenance window

   Admins approve per node or in bulk, in the UI or through an admin-scoped client.
3. **Order.** When the source-link protocol version changes, the controller updates first. Controllers support sources of the same major version and up to one minor version older.
4. **Apply.** The node downloads, verifies the signature and checksum, and applies the update for its install type (table below).
5. **Health.** After restarting, the node must report healthy within 120 seconds. Otherwise:
   - archive-based installs roll back automatically
   - installer-based installs notify the admin
6. **Report.** Results appear in the update table on the controller.

## Apply step by install type

The install type is recorded by the packaging as a marker file.

| Install type | Applied by | Method |
|---|---|---|
| Windows installer (desktop or service) | Node | Runs the signed installer silently with `/UPDATE`. Hooks keep the service and start-at-sign-in choices. |
| Windows portable / headless archives | Node | A/B version folders behind a launcher pointer |
| AppImage | Node | Atomic replacement of the AppImage file, then re-launch |
| deb, rpm, AUR | Package manager | Notify only, with instructions |
| Docker | Container manager | Notify only (`docker compose pull && docker compose up -d`) |

- The node updates itself and its source workers as one versioned unit.
- The desktop app shows progress for its local node and is updated by the same flow.
