# Architecture Decision Records

This directory contains Architecture Decision Records (ADRs) for the AnchorKit project.

An ADR captures a significant architectural decision made by the project team, including the context, the decision, and the consequences.

## ADR Index

| ID | Title | Status |
|----|-------|--------|
| [ADR-0001](ADR-0001-record-architecture-decisions.md) | Record Architecture Decisions | Accepted |
| [ADR-0002](ADR-0002-dual-build-strategy.md) | Dual-Build Strategy | Accepted |
| [ADR-0003](ADR-0003-contract-lifecycle-and-schema-versioning.md) | Contract Lifecycle and Schema Versioning | Accepted |

## What is an ADR?

An Architecture Decision Record is a short document that captures:

- **Context** — the forces and circumstances that led to the decision
- **Decision** — what was decided and why alternatives were rejected
- **Consequences** — the trade-offs, risks, and follow-up work that result

## How to write a new ADR

1. Copy `ADR-0000-template.md` to `ADR-XXXX-title.md`
2. Fill in the sections
3. Update this index
4. Open a PR for review

## Status meanings

| Status | Meaning |
|--------|---------|
| Proposed | Under discussion |
| Accepted | Agreed and implemented |
| Deprecated | Superseded by a later ADR |
| Superseded | Replaced by a newer ADR |
