# Reproducible Builds Guide

This document explains how to produce, verify, and compare AnchorKit release
artifacts in a way that is traceable and environment-independent.

---

## Contents

1. [Why reproducible builds matter](#why-reproducible-builds-matter)
2. [Prerequisites](#prerequisites)
3. [Build configuration](#build-configuration)
4. [Building a reproducible WASM artifact](#building-a-reproducible-wasm-artifact)
5. [Artifact checksums](#artifact-checksums)
6. [Verifying a release bundle](#verifying-a-release-bundle)
7. [Cross-build comparison](#cross-build-comparison)
8. [Scripts reference](#scripts-reference)
9. [Troubleshooting](#troubleshooting)

---

## Why reproducible builds matter

A reproducible build means that compiling the same source at the same commit
with the same toolchain always produces a bitwise-identical artifact. This
allows independent parties to:

- Confirm that a published WASM contract matches the source code at a tag.
- Detect tampering with CI-produced artifacts.
- Pin a specific binary on-chain by its SHA-256 hash with confidence.

---

## Prerequisites

| Requirement | Version |
|-------------|---------|
| Rust stable toolchain | ≥ 1.80.0 |
| `wasm32-unknown-unknown` Rust target | installed via `rustup` |
| `sha256sum` or `shasum` | standard on Linux/macOS |
| `wasm-opt` (binaryen) | optional, for optimized WASM comparison |
| `tar`, `rsync` | standard shell utilities |

Install the required Rust target if not already present:

```bash
rustup target add wasm32-unknown-unknown
```

---

## Build configuration

The following `Cargo.toml` release-profile settings contribute to
deterministic output:

| Setting | Value | Effect |
|---------|-------|--------|
| `codegen-units` | `1` | Prevents non-deterministic parallelism in code generation |
| `lto` | `true` | Link-time optimisation consolidates code before emission |
| `strip` | `"symbols"` | Removes debug symbols consistently |
| `opt-level` | `"z"` | Size-optimised code path with stable output |

Setting `SOURCE_DATE_EPOCH` eliminates embedded build timestamps:

```bash
export SOURCE_DATE_EPOCH=1717200000
```

The `Makefile` passes this value automatically via the `reproducible-wasm`
target.

---

## Building a reproducible WASM artifact

```bash
# Via Make (recommended — sets SOURCE_DATE_EPOCH automatically)
make reproducible-wasm

# Manually
export SOURCE_DATE_EPOCH=1717200000
cargo build --release \
    --target wasm32-unknown-unknown \
    --no-default-features \
    --features wasm
```

Output: `target/wasm32-unknown-unknown/release/anchorkit.wasm`

---

## Artifact checksums

Each release bundle (`dist/anchorkit-<VERSION>.tar.gz`) contains two checksum
files:

| File | Contents |
|------|----------|
| `dist/anchorkit-<VERSION>.sha256` | SHA-256 of the release tarball itself |
| `CHECKSUMS.sha256` (inside the tarball) | SHA-256 of every file in the bundle |

Both files are generated automatically by `scripts/package_release.sh`.

### Generating checksums for a local build

After running `make release`, generate and record checksums:

```bash
./scripts/verify_artifact_checksums.sh --generate
```

This writes `CHECKSUMS.sha256` into `dist/anchorkit-<VERSION>/` and
recalculates `dist/anchorkit-<VERSION>.sha256`.

### Verifying checksums

```bash
# Auto-detect version from Cargo.toml
./scripts/verify_artifact_checksums.sh

# Explicit version
./scripts/verify_artifact_checksums.sh 0.1.0
```

The script:

1. Verifies the tarball hash against `dist/anchorkit-<VERSION>.sha256`.
2. Extracts the bundle and compares every file against `CHECKSUMS.sha256`.
3. Confirms WASM magic bytes (`\0asm`) in `anchorkit.wasm`.
4. Confirms the CLI binary is executable.

Exit code `0` means all checks pass; exit code `1` means at least one failed.

---

## Cross-build comparison

To confirm two independent builds are bitwise-identical:

```bash
# Uses isolated CARGO_HOME and RUSTUP_HOME per build
make verify-reproducible
# or directly:
./scripts/verify_reproducible_build.sh
```

The script:

1. Validates the active Rust toolchain against the project toolchain file.
2. Copies the source to two temporary directories with separate Cargo homes.
3. Builds the WASM artifact in both.
4. Compares the SHA-256 hashes of both outputs.
5. Optionally runs `wasm-opt -Oz` on both and compares the optimised hashes.

Example output:

```
=== Reproducible Build Verification ===
Build A: a3f8c12d...
Build B: a3f8c12d...
✅ PASS: Both builds produce identical WASM (sha256: a3f8c12d...)
```

### Manual comparison

```bash
export SOURCE_DATE_EPOCH=1717200000

# First build
cargo build --release --target wasm32-unknown-unknown \
    --no-default-features --features wasm
cp target/wasm32-unknown-unknown/release/anchorkit.wasm build1.wasm

# Clean and rebuild
cargo clean
cargo build --release --target wasm32-unknown-unknown \
    --no-default-features --features wasm
cp target/wasm32-unknown-unknown/release/anchorkit.wasm build2.wasm

# Compare
sha256sum build1.wasm build2.wasm
```

Both lines must have the same hash.

---

## Scripts reference

| Script | Purpose |
|--------|---------|
| `scripts/verify_reproducible_build.sh` | Side-by-side build comparison |
| `scripts/verify_reproducible_build.ps1` | Same, for Windows PowerShell |
| `scripts/verify_artifact_checksums.sh` | Verify or generate artifact checksums |
| `scripts/package_release.sh` | Build and bundle release; generates `CHECKSUMS.sha256` |
| `scripts/validate_bundle.sh` | Structural validation of bundle contents |

### Make targets

| Target | What it does |
|--------|-------------|
| `make reproducible-wasm` | Build WASM with `SOURCE_DATE_EPOCH` set |
| `make verify-reproducible` | Run `verify_reproducible_build.sh` |
| `make release` | Full release build including checksums |
| `make release-validate` | Validate bundle structure |

---

## Troubleshooting

### Builds are not matching

1. Confirm the same Rust toolchain version is used: `rustup show`.
2. Set `SOURCE_DATE_EPOCH` to the same value in both builds.
3. Run `cargo clean` between builds to eliminate stale incremental cache.
4. Ensure `Cargo.lock` is identical in both build trees (it is committed to the
   repository; do not run `cargo update` mid-comparison).
5. Check that no `[patch]` entries in `Cargo.toml` point to local paths that
   differ between environments.

### Checksum file not found

The `CHECKSUMS.sha256` manifest is generated inside the bundle by
`scripts/package_release.sh`. If it is missing from an existing bundle,
regenerate it:

```bash
./scripts/verify_artifact_checksums.sh --generate
```

### `wasm-opt` outputs differ after matching pre-opt hashes

This is a known artefact of `wasm-opt` versions that differ between
environments. Install the same binaryen version (`apt-get install binaryen` on
Ubuntu gives a pinned version; check `wasm-opt --version`) and rerun. Pre-opt
hash equality is the authoritative reproducibility signal; `wasm-opt`
differences are a warning, not a failure.
