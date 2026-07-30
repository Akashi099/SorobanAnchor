# Release Checklist

This document describes the automated release checklist and how maintainers run it before every release.

## Quick start

```bash
./scripts/release-checklist.sh <version>
```

Example:

```bash
./scripts/release-checklist.sh 0.3.0
```

The script will walk through every prerequisite, report pass/fail for each step, and exit non-zero if any check fails. Fix all failures before tagging the release.

---

## What the checklist covers

| Step | Check |
|------|-------|
| Git state | Working tree is clean (no uncommitted changes) |
| Branch | On `main` and up to date with `origin/main` |
| Version | `Cargo.toml` version matches the supplied version argument |
| Changelog | `CHANGELOG.md` contains an entry for the new version |
| Formatting | `cargo fmt --all -- --check` passes |
| Linting | `cargo clippy --all-targets --all-features -- -D warnings` passes |
| Tests | `cargo test` passes |
| WASM build | `cargo build --target wasm32-unknown-unknown` succeeds |
| Dependency audit | `cargo audit` reports no vulnerabilities |
| API snapshot | No unexpected diff in `api_snapshots/` |

---

## Running individual steps

If you need to re-run a single step after fixing an issue:

```bash
# Format check only
cargo fmt --all -- --check

# Lint only
cargo clippy --all-targets --all-features -- -D warnings

# Tests only
cargo test

# WASM build only
cargo build --target wasm32-unknown-unknown

# Dependency audit only
cargo audit

# API snapshot diff only
./scripts/diff_api_snapshot.sh
```

---

## After the checklist passes

1. Tag the release:
   ```bash
   git tag -s v<version> -m "Release v<version>"
   git push origin v<version>
   ```

2. Package the release artifacts:
   ```bash
   ./scripts/package_release.sh
   ```

3. Sign and verify artifacts:
   ```bash
   ./scripts/sign_release.sh
   ./scripts/verify_release.sh
   ```

4. Publish the GitHub Release with the changelog entry and attach the signed artifacts.

For the full signing and verification process, see [release-signing.md](release-signing.md).  
For reproducible build verification, see [REPRODUCIBLE_BUILDS.md](REPRODUCIBLE_BUILDS.md).

---

## Troubleshooting

**`cargo audit` reports a vulnerability**  
Update the affected crate (`cargo update -p <crate>`) or add a temporary advisory ignore with justification in `deny.toml`. Do not release with unacknowledged vulnerabilities.

**API snapshot diff is unexpected**  
Review `api_snapshots/` with `./scripts/diff_api_snapshot.sh`. If the change is intentional, update the snapshot with `./scripts/snapshot_api.sh` and document the change in `CHANGELOG.md`.

**WASM build fails**  
Check that no `std`-only dependencies were introduced. Run `./scripts/validate_no_std_compliance.sh` for details.
