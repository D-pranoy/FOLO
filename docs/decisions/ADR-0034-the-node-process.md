# ADR-0034: The node process: folders, launches and its command line

Status: Proposed
Date: 2026-10-22

## Context

RB-06 builds the node's core. The documents fix a lot of it:
- the settings layering, managed and foreground launches, deployment overrides and list settings that add up ([ADR-0010](ADR-0010-settings-persistence-and-migration.md) D1)
- a data folder per install type, following platformdirs conventions (ADR-0010 D2)
- the commands `folo run`, `status --json`, `reload`, `config import`, `token show|rotate`, `pair reset` and `join` ([RB-06](../runbooks/m3-controller-core.md#rb-06-node-core) step 8), and what `folo status --json` reports ([control plane](../architecture/control-plane.md#status-and-health))
- a single instance (RB-06 step 7)
- how the node starts its effect engine, and the loopback link the engine dials back on ([ADR-0032](ADR-0032-local-source-link.md))

They don't say:
- where `node.toml` lives, since its own `data_dir` setting can move the data folder; how the node knows which install type it is; or where its logs go
- what the `FOLO_*` variables and the flags are called, how a list setting is written in a variable, or how a node knows it was started by the desktop app
- what "single instance" covers: the testing policy runs experiments beside a live node, with a separate data folder and port
- how `folo status` learns about a running node's workers, or how `folo reload` reaches the node, when the client API only arrives in RB-07
- where the node serves the local link before RB-07, although RB-06 step 6 sends the engine its settings over that link and checks its heartbeat; RB-07 step 6 lists the endpoint
- the node's own log, and what collecting the engine's logs means
- where a first run looks for a legacy `studio.json` (ADR-0010 D5 says "when found")
- what the node does with a document the [configuration model](../specs/config-model.md#clamping-rules) rejects for a structural error

## Decision

- **D1.** **Home and data folders.** Each node has a **home** folder holding `node.toml`. Its **data** folder holds the documents, `secrets.json` and `library/`, and is the home unless `data_dir` moves it. The node takes the first of these that applies:

  | Launch | Home | Data, by default | Logs |
  |---|---|---|---|
  | `--home <folder>` or `FOLO_HOME` | that folder | the home | `logs` in the data folder |
  | Portable: `portable.ini` beside the executable | `data` beside the executable | the home | `logs` in the data folder |
  | Service: `--service` | `%ProgramData%\FOLO` on Windows, `/var/lib/folo` on Linux | the home | `logs` in the data folder |
  | User, Windows | `%LOCALAPPDATA%\FOLO` | the home | `logs` in the data folder |
  | User, Linux | `$XDG_CONFIG_HOME/folo` | `$XDG_DATA_HOME/folo` | `$XDG_STATE_HOME/folo/logs`, or `logs` in the data folder when `data_dir` is set |

  The XDG variables take their XDG Base Directory defaults when unset. The Docker image sets `FOLO_HOME=/data`.
- **D2.** **Managed launches.** The desktop app starts the node with `--managed`. A managed node takes no other flags, ignores every inherited `FOLO_*` variable and removes them from its environment, so its workers don't inherit them either. Every other launch is a foreground launch.
- **D3.** **Overrides.** Every setting in `node.toml` has a variable, `FOLO_` and the setting's path in upper case with `_` for `.` (`FOLO_LISTEN_PORT`), and a flag ([control plane](../architecture/control-plane.md#settings-layering) lists both).
  - A list setting takes a comma-separated list in its variable and a repeated flag. Its entries add up with the file's, without duplicates, up to the setting's limit.
  - An override the node can't read is ignored with a warning, so the layer below it stands. One out of range is clamped.
  - `folo status --json` lists every override with its source: the variable or the flag.
- **D4.** **One node per home.** A running node holds an operating-system lock on `folo.lock` in its home, which holds its process id. The lock ends with the process, also when it crashes. A second node on the same home exits with status 3. Experiments beside a live node use a home of their own (`--home`) and a port of their own.
- **D5.** **The command line reaches a running node through its loopback API,** at `127.0.0.1` and the port its settings give, as the desktop app does. Loopback requests need no key ([ADR-0009](ADR-0009-client-access.md) D4).
  - `folo status` reads `GET /api/v1/system/status`, and `folo reload` sends `POST /api/v1/system/reload`. Both routes answer loopback requests only.
  - With no node running (the lock is free), `folo status` reports from the home's files, with `running` false and no workers.
  - Commands that write the node's files themselves (`folo config import`) take the lock, so they run only while the node is stopped.
- **D6.** **The node's port from RB-06 on.** The node serves its port from RB-06: the [local link](../specs/source-link.md#the-local-link) with its launch token, and D5's two routes, with the client API's Host check and its JSON rule for mutating requests. RB-07 builds the rest of the client API on the same server. While remote access is off, the node binds `127.0.0.1` only (ADR-0009 D1).
- **D7.** **`folo reload`** re-reads `node.toml` and the documents. It applies what can change while the node runs: `log_level`, `remote_access.allowed_hosts`, `local_source.enabled` and the documents. It reports the rest as needing a restart: `role`, `listen`, `data_dir` and `remote_access.enabled`. Variables and flags stay as the node was started with them.
- **D8.** **Logs.** The node writes JSON lines in the effect engine's format ([control plane](../architecture/control-plane.md#supervision)) to stderr and to `folo.log` in its log folder. The file rotates at 10 MiB, and four older files are kept (`folo.log.1` to `folo.log.4`).
  - The engine's stderr lines go into the node's log as they are. A line that isn't a JSON object is logged at `error` with the logger `folo-source`.
  - `log_level` filters the engine's lines as well as the node's own.
- **D9.** **The local engine.** The node runs `folo-source` from its own executable's folder, or else from the `PATH`. The engine's environment is the node's without the `FOLO_*` variables, plus the link's URL and launch token.
  - **Restarts:** after 1 s, doubling up to 60 s, each delay cut by up to 25% at random. The delay starts over once the engine has stayed linked for 60 s.
  - **Stopping:** the node sends `restart {component: engine}`, which ends the engine with status 0, and kills it if it hasn't exited after 5 s.
  - **When it runs:** in the controller role while `local_source.enabled` is on. In the source role it always runs, and it gets no configuration until the node's owner controller sends one (RB-16).
- **D10.** **First run.** A node whose data folder has no `zones.json` imports the first `studio.json` it finds, in its home and then in its working directory, with `folo-source migrate-v1` ([RB-05](../runbooks/m2-engine.md#rb-05-v1-configuration-migration)). `folo config import [<studio.json>]` runs the same import on demand, from the given file or else from the same places.
- **D11.** **Documents the node can't read.** A document with a structural error (it isn't JSON, its root isn't an object, or its `schema_version` is newer than the node reads; [configuration model](../specs/config-model.md#clamping-rules)) stops the node from starting, with an error naming the file, which is left as it is. A missing document starts from its defaults.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| A local socket or named pipe for the command line | Works without the API | A second control channel beside the loopback API the desktop app uses; permissions differ between user and service modes |
| Signals for `folo reload` (SIGHUP) | Conventional on Linux | Nothing equivalent on Windows; can't report what needs a restart; no help for `folo status` |
| A lock per data folder | Keeps the testing policy's `--data-dir` experiments | Two nodes could write one `node.toml` |
| One folder for everything on Linux too | Simpler | Against the XDG directories ADR-0010 D2 names |
| Detecting Docker and services | No flag needed | Guesswork; an explicit variable and flag are predictable |
| The local link endpoint only from RB-07 | Matches RB-07 step 6 as written | RB-06 couldn't send the engine its settings or check its heartbeat |
| Searching more places for `studio.json` | Finds more setups | The legacy studio kept it in its own folder, which can be anywhere; `folo config import <path>` covers the rest |
| Starting on defaults when a document can't be read | The node always starts | The next write replaces the user's file, or one written by a newer version |

## Consequences

- **Contract:** the client API gains `GET /system/status` (`NodeStatus`) and `POST /system/reload` (`ReloadResult`), an additive change: version 1.1.0.
- **Documents:** the [control plane](../architecture/control-plane.md) (folders, overrides, one node per home, the command line, logs, the local engine and its configuration), the [configuration model](../specs/config-model.md) (where `node.toml` lives, and documents the node can't read), the [client API](../specs/client-api.md), [platform and packaging](../architecture/platform-and-packaging.md) (`FOLO_HOME` in Docker, `--service` for services), the [testing policy](../process/testing-policy.md) (experiments use `--home`) and [M3](../runbooks/m3-controller-core.md) (RB-06 steps 3, 6 and 8; RB-07 steps 1 and 6).
- **ADR-0032** names RB-07 step 6 for the token check. The check now arrives with RB-06, which serves the endpoint (D6).
- **Packaging** (M8) starts services with `--service`, the desktop app starts the node with `--managed`, and the Docker image sets `FOLO_HOME`.
