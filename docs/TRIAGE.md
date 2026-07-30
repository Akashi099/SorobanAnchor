# Issue Triage and Prioritization Workflow

This document defines how issues are categorized, prioritized, and assigned in SorobanAnchor so that high-value work is addressed first and the backlog stays healthy.

## Overview

Triage runs on a rolling basis. Any maintainer can triage new issues. The goal is to give every issue a **type label**, a **priority label**, and a **milestone assignment** within five business days of opening.

---

## Step-by-step triage process

### 1. Is the issue actionable?

| Situation | Action |
|-----------|--------|
| Duplicate | Add `duplicate` label, link to the original, close |
| Not reproducible / missing info | Add `needs-info`, ask the reporter for details |
| Out of scope | Add `wontfix`, explain why, close politely |
| Valid — proceed | Continue to step 2 |

### 2. Assign a type label

| Label | When to use |
|-------|-------------|
| `bug` | Incorrect behavior, regression, or security flaw |
| `enhancement` | New feature or improvement to existing behavior |
| `docs` | Documentation gap or error |
| `chore` | Dependency updates, CI tweaks, tooling |
| `question` | Needs clarification before it can be acted on |

### 3. Assign a priority label

Use the rubric below to pick exactly one priority:

| Label | Definition | Examples |
|-------|------------|---------|
| `P0 — critical` | Production broken, data loss, or security vulnerability | Auth bypass, contract state corruption |
| `P1 — high` | Major feature broken or blocking a release | CI always red, WASM build fails |
| `P2 — medium` | Important improvement, not blocking | Missing SEP coverage, coverage gap |
| `P3 — low` | Nice-to-have, long-tail cleanup | Typo in docs, minor UX polish |

**Prioritization rubric — ask these questions in order:**

1. **Security impact?** → P0 immediately; follow the [security policy](governance-and-security.md).
2. **Blocking a release or another team?** → P1.
3. **Degraded functionality that has a workaround?** → P2.
4. **Everything else.** → P3.

### 4. Assign to a milestone

Consult the [ROADMAP.md](ROADMAP.md) and assign the issue to the earliest milestone where it fits. If it does not fit any current milestone, leave it unassigned and add the `backlog` label.

### 5. Assign an owner (optional)

If a maintainer is ready to work on the issue immediately, self-assign it. Otherwise, leave it unassigned so contributors can pick it up.

---

## Weekly triage review

Maintainers run a short (≤ 30 min) triage meeting or async thread to:

- Review all new issues opened since the last cycle.
- Promote or demote priorities based on changed circumstances.
- Move stale `P3` issues older than 90 days to `backlog` or close as `wontfix`.
- Confirm milestone assignments ahead of each release.

---

## Labels reference

| Label | Purpose |
|-------|---------|
| `P0 — critical` | Must fix immediately |
| `P1 — high` | Fix in current milestone |
| `P2 — medium` | Fix in next milestone |
| `P3 — low` | Fix when capacity allows |
| `bug` | Incorrect behavior |
| `enhancement` | New or improved feature |
| `docs` | Documentation work |
| `chore` | Maintenance |
| `question` | Needs clarification |
| `good first issue` | Suitable for new contributors |
| `help wanted` | Extra attention needed |
| `needs-info` | Awaiting reporter response |
| `duplicate` | Already tracked elsewhere |
| `wontfix` | Out of scope or intentional |
| `backlog` | Valid but unscheduled |

---

## Contributor guide

- **Reporting a bug:** Use the bug report template and include reproduction steps, expected vs. actual behavior, and environment details.
- **Requesting a feature:** Describe the use case and why existing behavior does not satisfy it.
- **Picking up an issue:** Comment "I'd like to work on this" so it can be assigned to you. Start from issues labeled `good first issue` or `help wanted`.

For the full contribution workflow, see [CONTRIBUTING.md](CONTRIBUTING.md).
