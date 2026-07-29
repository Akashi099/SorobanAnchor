# Maintainer Onboarding Guide

Welcome to the SorobanAnchor / AnchorKit project. This guide covers everything
a new maintainer needs to get oriented, set up locally, and start contributing
effectively without stepping on anything important.

---

## Table of Contents

1. [Repo overview](#1-repo-overview)
2. [Prerequisites](#2-prerequisites)
3. [Local setup](#3-local-setup)
4. [Key concepts](#4-key-concepts)
5. [Day-to-day workflows](#5-day-to-day-workflows)
6. [Review and merge checklist](#6-review-and-merge-checklist)
7. [Release process](#7-release-process)
8. [Admin key access](#8-admin-key-access)
9. [Escalation and contacts](#9-escalation-and-contacts)

---

## 1  Repo overview

```
src/           Core library — contract, SEP handlers, utilities
tests/         Integration and unit tests
configs/       Example anchor configs (JSON + TOML)
examples/      Rust and shell usage examples
scripts/       Build, validation, CI, and deploy scripts
docs/          All project documentation (start here)
benches/       Criterion benchmarks
fuzz/          Fuzz targets
```

The on-chain entry point is `src/contract.rs`. The public Rust API surface is
exposed through `src/lib.rs`. All stable error codes live in `src/errors.rs`
and are documented in [`docs/error-codes.md`](error-codes.md).

---

## 2  Prerequisites

| Tool | Version | Why |
|------|---------|-----|
| Rust | 1.75+ | Build and test |
| `wasm32-unknown-unknown` target | — | WASM build |
| Python | 3.7+ | Config validation scripts |
| `soroban-cli` | latest | Contract deployment |
| Binaryen (`wasm-opt`) | optional | WASM size optimization |
| GPG or minisign | optional | Release signing |

Install the WASM target:

```bash
rustup target add wasm32-unknown-unknown
```

---

## 3  Local setup

```bash
# Clone
git clone https://github.com/abore9769/SorobanAnchor.git
cd SorobanAnchor

# Install the pre-commit hook (runs fmt + clippy + test before each commit)
./scripts/setup-hooks.sh

# Verify everything builds and tests pass
make check
```

`make check` runs formatting, clippy, and the full test suite. It should be
green on a clean checkout. If it isn't, open an issue before doing anything
else.

### Environment variables

For testnet work:

```bash
export SOROBAN_NETWORK=testnet
export SOROBAN_RPC_URL=https://soroban-testnet.stellar.org:443
export SOROBAN_NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
export ANCHOR_ADMIN_SECRET=<your-testnet-admin-key>
```

Never commit keys. The `.gitignore` already excludes `*.secret`, `*.key`,
`.env`, and `secrets/`.

---

## 4  Key concepts

### Build modes

The project compiles in two distinct modes with completely separate feature sets:

| Mode | Command | Output | Has CLI |
|------|---------|--------|---------|
| Native | `cargo build --release` | `target/release/anchorkit` | Yes |
| WASM (Soroban) | `cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm` | `*.wasm` | No |

Always test both before touching anything in `src/contract.rs` or `src/lib.rs`.

### Feature flags

| Flag | Purpose |
|------|---------|
| `std` | Filesystem config loading (default on) |
| `wasm` | Soroban WASM target — strips HTTP/CLI modules |
| `mock-only` | Pre-built fixtures for tests without a live anchor |
| `stress-tests` | High-concurrency load tests (excluded from normal CI) |

### Error codes

All contract errors are stable integer codes defined in `src/errors.rs` and
listed in [`docs/error-codes.md`](error-codes.md). Do not renumber existing
codes — add new ones at the end only.

### Schema versioning

The on-chain data schema is versioned. Every data-shape change needs a
migration step in `src/migration.rs`. See
[`docs/migration-guide.md`](migration-guide.md) for the full process.

### Admin model

The contract has a primary admin (set at `initialize`) plus a capability
delegation system. Capability codes and grant/revoke procedures are documented
in [`docs/RUNBOOK.md`](RUNBOOK.md#admin-capability-management).

---

## 5  Day-to-day workflows

### Making a change

```bash
git checkout -b feat/short-description
# ... edit ...
make check          # must be green before committing
git add <files>
git commit -m "feat(scope): description"
git push -u origin feat/short-description
# open a PR
```

Follow [conventional commits](https://www.conventionalcommits.org/):
`feat`, `fix`, `docs`, `refactor`, `test`, `chore`.

### Running tests

```bash
cargo test                              # standard suite
cargo test --features mock-only         # with mock fixtures
cargo test --test cli_integration_harness  # integration harness
cargo test --features stress-tests      # load tests (slow)
```

### Config validation

```bash
./scripts/validate_all.sh       # validates all configs in configs/
./scripts/pre_deploy_validate.sh  # full pre-deploy check
```

### API contract snapshots

To capture and compare the public API surface across versions, see
[`docs/api-contract-snapshots.md`](api-contract-snapshots.md).

### Generating a changelog

Changelog entries are generated from conventional commit history. See
[`docs/changelog-generation.md`](changelog-generation.md).

---

## 6  Review and merge checklist

Before merging any PR:

- [ ] `make check` passes (fmt + clippy + tests)
- [ ] Both native and WASM builds succeed
- [ ] New public API items have doc comments
- [ ] Error codes are not renumbered
- [ ] Config schema updated if new fields added
- [ ] Migration step added for any on-chain data shape change
- [ ] Security review completed for auth / crypto / HTTP changes
  (see [`docs/SECURITY_REVIEW_CHECKLIST.md`](SECURITY_REVIEW_CHECKLIST.md))

Routine changes need **one** maintainer approval. Breaking changes, contract
upgrades, or security model changes need **two** approvals and a 48-hour window.
Full policy: [`docs/governance-and-security.md`](governance-and-security.md).

---

## 7  Release process

```bash
make release           # build native + WASM + bundle under dist/
make release-validate  # verify bundle contents and checksums
make release-sign      # sign artifacts (requires GPG key)
```

Tag the release:

```bash
git tag -s v0.x.0 -m "Release v0.x.0"
git push origin v0.x.0
```

Full release runbook: [`docs/RUNBOOK.md`](RUNBOOK.md).
Signing and verification: [`docs/release-signing.md`](release-signing.md).

---

## 8  Admin key access

- Testnet keys: ask an existing maintainer; rotated after each major release.
- Mainnet keys: stored on offline hardware wallets with 2-of-N multi-sig.
  No single person holds all signing keys.
- Key operations follow the same two-maintainer approval process as contract
  upgrades.

If you need mainnet key access, open a governance discussion in the repository
before any key-sharing happens.

---

## 9  Escalation and contacts

- **Bugs / feature requests** — open a GitHub issue.
- **Security vulnerabilities** — do not open a public issue; use GitHub's
  private advisory reporting at
  `https://github.com/abore9769/SorobanAnchor/security/advisories/new`.
  See [`docs/governance-and-security.md`](governance-and-security.md#responsible-disclosure).
- **PR review requests** — tag `@maintainers` in the PR.
- **Urgent production issues** — contact the on-call maintainer directly via
  the contact method shared in the private maintainer channel.

---

## References

- [README.md](../README.md)
- [CONTRIBUTING.md](CONTRIBUTING.md)
- [RUNBOOK.md](RUNBOOK.md)
- [governance-and-security.md](governance-and-security.md)
- [error-codes.md](error-codes.md)
- [migration-guide.md](migration-guide.md)
- [api-contract-snapshots.md](api-contract-snapshots.md)
- [changelog-generation.md](changelog-generation.md)
