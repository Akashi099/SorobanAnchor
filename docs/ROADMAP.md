# SorobanAnchor Roadmap

This document tracks the project's milestone phases and strategic priorities. It is updated as the backlog is refined and issues are triaged.

For the issue triage and prioritization process, see [TRIAGE.md](TRIAGE.md).

## How to use this roadmap

- Each milestone groups related issues and features into a coherent release target.
- Issues are linked to milestones via [GitHub Milestones](../../milestones).
- To propose a new item, open an issue and a maintainer will triage and assign it to a milestone during the next review cycle.
- Status key: `[ ]` planned · `[~]` in progress · `[x]` done

---

## Milestone 0 — Foundation (current)

Goal: stable core library, reproducible builds, CI, and contributor infrastructure.

- [x] Core contract: attestations, sessions, SEP-10 JWT, SEP-6 normalization
- [x] Multi-asset routing across corridors
- [x] Rate limiting and retry/backoff
- [x] Reproducible WASM builds and dual-build strategy (ADR-0002)
- [x] Schema versioning and contract lifecycle (ADR-0003)
- [x] CI pipeline: fmt, clippy, tests, WASM build, dependency audit
- [x] Architecture Decision Records (ADRs)
- [x] API contract snapshots and changelog generation
- [x] Contributor documentation and security review checklist
- [~] Roadmap and milestone tracking (#696)
- [~] Issue triage and prioritization workflow (#697)
- [~] Release checklist automation (#698)
- [~] Branch and PR hygiene automation (#699)

---

## Milestone 1 — Reliability and Observability

Goal: production-grade reliability, structured observability, and expanded SEP coverage.

- [ ] Structured logging with trace propagation across all critical paths
- [ ] Live smoke tests against testnet anchors
- [ ] Ledger boundary and replay protection hardening
- [ ] SEP-31 cross-border payment normalization
- [ ] Vendor status mapping and circuit breaker
- [ ] Coverage thresholds enforced in CI (≥ 80 % line coverage)
- [ ] Fuzz targets for config parsing, domain validation, JWT, and JSON responses

---

## Milestone 2 — Developer Experience

Goal: lower the barrier to integration and contribution.

- [ ] CLI examples and interactive playground UI
- [ ] Generated API docs published to GitHub Pages
- [ ] Cross-language usage guide (JS/TS bindings)
- [ ] Mock mode and offline mode documented end-to-end
- [ ] Onboarding guide covering first-time contributor path
- [ ] Storybook for UI components

---

## Milestone 3 — Production Readiness (v1.0)

Goal: stable public API, security audit, and first tagged release.

- [ ] Stable public API surface with semver guarantees
- [ ] External security audit and remediation
- [ ] Release signing and artifact verification
- [ ] Upgrade playbook from pre-1.0 builds
- [ ] Migration guide for schema changes
- [ ] Full governance and security policy published

---

## Linking issues to milestones

1. Open or find the relevant issue on GitHub.
2. In the issue sidebar, click **Milestone** and select the target milestone.
3. Add the appropriate priority label (see [TRIAGE.md](TRIAGE.md)).
4. Reference the issue in commits with `closes #<number>` so it closes automatically on merge.

## Updating this document

Maintainers should update milestone status after each sprint review. Move items from `[ ]` to `[~]` when work starts and to `[x]` when merged. Add new items under the appropriate milestone as the backlog is refined.
