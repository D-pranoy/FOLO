# ADR-0001: Documentation is the source of truth

Status: Accepted
Date: 2026-10-13

## Context

FOLO is being rebuilt by a small team, with heavy use of AI-assisted development. Several components are written in different languages (Rust, Python, TypeScript) and must agree on contracts. Without one authoritative description, behaviour drifts and decisions get lost.

## Decision

- **D1.** Plans, specifications, decisions, run books and process documents in `docs/` are first-class parts of the codebase and **the source of truth**. The software exists as the compliant implementation of them.
- **D2.** Order of authority: decisions (ADRs) > specifications > architecture and integrations > run books > code ([docs index](../README.md#order-of-authority)).
- **D3.** A behaviour change starts with a documentation change, reviewed in the same pull request as the code or before it. Code that contradicts the documents is a defect.
- **D4.** Machine-readable contracts (JSON Schema, OpenAPI, AsyncAPI) are generated from or checked against the prose specifications. CI fails when they disagree with the implementation.
- **D5.** Documents contain no time estimates and no personal details about who owns which hardware or who will do which task.
- **D6.** `AGENTS.md` (imported by `CLAUDE.md`) tells coding agents to follow this ADR.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Code as the source of truth, docs describe it | Less writing | Drift; decisions are lost; agents have no contract to follow |
| A wiki outside the repository | Easy editing | Not versioned with the code; not reviewed in PRs |

## Consequences

- Every pull request checks documentation impact; the PR template includes it.
- Documentation reviews carry the same weight as code reviews.
- The `docs/` tree must be kept consistent; contradictions are fixed in the lower-authority document.
