# ADR-0001: Record Architecture Decisions

- **Status:** Accepted
- **Date:** 2026-07-29
- **Author:** Project Maintainers

## Context

As the AnchorKit project grows, contributors need to understand why the system
is designed the way it is. Key architectural decisions were being lost or
rediscovered through PR reviews. Without explicit records, new contributors
cannot tell whether a design choice was deliberate or accidental.

The project needed a lightweight process to capture design decisions so that:

- Future contributors can understand the rationale behind past decisions.
- Reviewers have a shared reference when evaluating new proposals.
- Decisions are searchable and linked from the relevant code and docs.

## Decision

We adopt the Architecture Decision Record (ADR) format popularised by
Michael Nygard. Each ADR is a one-page Markdown document stored in
`docs/adr/`.

### Format

Each ADR contains:

- **Title** — ADR number and short name
- **Status** — Proposed, Accepted, Deprecated, or Superseded
- **Date** — when the decision was made
- **Context** — the problem or forces that motivated the decision
- **Decision** — the chosen approach
- **Consequences** — trade-offs, risks, and follow-up work

### Process

1. Anyone may propose an ADR by creating a PR that adds a new file to
   `docs/adr/`.
2. The PR follows the standard review process (one maintainer approval for
   routine decisions, two for significant architectural changes).
3. Once merged the ADR is considered Accepted.
4. An ADR may be deprecated or superseded by a later ADR, which must
   reference the superseded record.

### Numbering

ADRs are numbered sequentially (ADR-0001, ADR-0002, …). The leading zeros
keep lexicographic sorting consistent.

## Consequences

**Positive:**
- Design rationale is captured explicitly and survives staff changes.
- New contributors can catch up on the project's architectural history.
- PR discussions can reference ADRs as shared context.

**Negative:**
- Maintaining ADRs requires discipline; they can become stale if not kept in
  sync with the code.
- The process adds a small overhead to significant changes.

**Mitigation:**
- ADR reviews are lightweight and piggy-back on the existing PR process.
- Code reviewers check whether a change that contradicts an ADR should update
  or supersede it.
