# Specifications

Status: Accepted

Specifications define contracts between components. They are the second-highest authority after the [decisions](../decisions/README.md).

## Prose and machine-readable forms

These documents are the human-readable specifications. Run book RB-02 turns them into machine-readable contracts in the repository's `specs/` directory:

| Contract | Machine-readable form | Generated code |
|---|---|---|
| Configuration and API models | JSON Schema (`specs/schema/*.json`) | Python (pydantic), Rust (serde, via typify), TypeScript |
| Client API | OpenAPI 3.1 (`specs/openapi.yaml`) | TypeScript client types; server conformance tests |
| WebSocket channels (live data, source link, ingest) | AsyncAPI 3.0 (`specs/asyncapi.yaml`) | Message types; conformance tests |

Once a machine-readable form exists, it and this prose must agree. A change to either goes through the [workflow](../process/workflow.md), together with an ADR when the change breaks compatibility.

## Index

| Specification | Implementation |
|---|---|
| [Configuration model](config-model.md) | Partial (JSON Schemas, generated types and clampers; nothing reads the documents yet) |
| [Client API](client-api.md) | Partial (the OpenAPI contract, generated TypeScript types and the conformance harness; no server yet) |
| [Source link](source-link.md) | Partial (the AsyncAPI contract, message fixtures, the conformance harness, and the effect engine's side of the local link) |
| [Pairing and connect](pairing-and-connect.md) | Not started |
| [Pack format](pack-format.md) | Partial (JSON Schemas for the manifest and the items; nothing reads packs yet) |
| [UI parity checklist](ui-parity-checklist.md) | Not started |
| [Visual design](visual-design.md) | Partial (the current studio implements the visual language) |

## Versioning

- Each contract has its own semantic version.
- The node reports the versions it supports in `GET /api/v1/info` and in the source-link handshake.
- A controller accepts sources of the same source-link major version and up to one minor version older.
