WASM_TARGET := wasm32-unknown-unknown
WASM_OUT    := target/$(WASM_TARGET)/release/anchorkit.wasm
VERSION     := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
DIST_DIR    := dist
SOURCE_DATE_EPOCH ?= 1717200000

.PHONY: build test wasm lint \
        doc-lint doc-lint-fix doc-check \
        integration-test integration-test-live \
        integration-test-pipeline \
        stress-test \
        bench bench-save bench-compare \
        release release-validate \
        clean-dist reproducible-wasm verify-reproducible

# ── Core build targets ────────────────────────────────────────────────────────

build:
	cargo build --release

test:
	cargo test

wasm:
	cargo build --release --target $(WASM_TARGET) --no-default-features --features wasm
	@ls -lh $(WASM_OUT)

# ── Reproducible build targets ─────────────────────────────────────────────────

reproducible-wasm:
	@SOURCE_DATE_EPOCH=$(SOURCE_DATE_EPOCH) cargo build --release --target $(WASM_TARGET) --no-default-features --features wasm
	@ls -lh $(WASM_OUT)

verify-reproducible:
	@bash scripts/verify_reproducible_build.sh

# Formatting
fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

# Linting
lint:
	cargo clippy -- -D warnings

# ── Documentation linting ─────────────────────────────────────────────────────

## Lint all documentation for broken links, heading consistency, and command hygiene.
doc-lint:
	@bash scripts/validate-docs.sh

## Auto-fix markdownlint issues where possible (safe formatting fixes only).
doc-lint-fix:
	@bash scripts/validate-docs.sh --fix

## Run all quality checks: code formatting, linting, tests, and documentation.
doc-check: fmt-check lint doc-lint
	@echo "All quality checks passed."

# ── Integration test harness ──────────────────────────────────────────────────

## Run the CLI integration test harness (local simulation, no network required).
integration-test:
	cargo test --test cli_integration_harness -- --nocapture

## Run the extended integration pipeline tests (local simulation, no network required).
integration-test-pipeline:
	cargo test --test integration_pipeline_tests -- --nocapture

## Run the CLI integration test harness against a live testnet.
## Requires: ANCHOR_CONTRACT_ID, ANCHOR_ADMIN_SECRET
integration-test-live:
	SOROBAN_ANCHOR_INTEGRATION=testnet cargo test --test cli_integration_harness -- --nocapture

## Run the dedicated live network smoke tests against a live testnet.
## Requires: ANCHOR_CONTRACT_ID, ANCHOR_ADMIN_SECRET
smoke-test-live:
	SOROBAN_ANCHOR_INTEGRATION=testnet cargo test --test live_smoke_tests -- --nocapture

## Run the full stress-test suite (excluded from normal CI).
stress-test:
	cargo test --features stress-tests -- --nocapture

## Run all performance benchmarks (results in target/criterion/).
bench:
	cargo bench --bench load_benchmarks

## Run benchmarks and save results as a named baseline for later comparison.
## Usage: make bench-save BASELINE=my-baseline
bench-save:
	cargo bench --bench load_benchmarks -- --save-baseline $(or $(BASELINE),main)

## Compare current benchmark results against a saved baseline.
## Usage: make bench-compare BASELINE=main
bench-compare:
	cargo bench --bench load_benchmarks -- --baseline $(or $(BASELINE),main)

# ── Release packaging ─────────────────────────────────────────────────────────

## Build and bundle all release artifacts into dist/anchorkit-<VERSION>.tar.gz
release:
	@bash scripts/package_release.sh $(VERSION)

## Sign the release artifacts (GPG by default; set ANCHORKIT_SIGNING_BACKEND=minisign for minisign).
release-sign:
	@bash scripts/sign_release.sh $(VERSION)

## Sign the release artifacts in dry-run mode (prints commands without executing).
release-sign-dry-run:
	@bash scripts/sign_release.sh --dry-run $(VERSION)

## Validate the release bundle produced by `make release`.
release-validate:
	@bash scripts/validate_bundle.sh $(DIST_DIR)/anchorkit-$(VERSION).tar.gz

## Verify artifact checksums for the current release bundle.
verify-checksums:
	@bash scripts/verify_artifact_checksums.sh $(VERSION)

## Generate artifact checksums for the current release bundle (run after `make release`).
generate-checksums:
	@bash scripts/verify_artifact_checksums.sh --generate $(VERSION)

## Remove the dist/ directory.
clean-dist:
	rm -rf $(DIST_DIR)
