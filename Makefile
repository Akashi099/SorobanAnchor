WASM_TARGET := wasm32-unknown-unknown
WASM_OUT    := target/$(WASM_TARGET)/release/anchorkit.wasm
VERSION     := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
DIST_DIR    := dist
SOURCE_DATE_EPOCH ?= 1717200000

.PHONY: build test wasm lint \
        integration-test integration-test-live \
        release release-validate verify-checksums generate-checksums \
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

# ── Integration test harness ──────────────────────────────────────────────────

## Run the CLI integration test harness (local simulation, no network required).
integration-test:
	cargo test --test cli_integration_harness -- --nocapture

## Run the CLI integration test harness against a live testnet.
## Requires: ANCHOR_CONTRACT_ID, ANCHOR_ADMIN_SECRET
integration-test-live:
	SOROBAN_ANCHOR_INTEGRATION=testnet cargo test --test cli_integration_harness -- --nocapture

# ── Release packaging ─────────────────────────────────────────────────────────

## Build and bundle all release artifacts into dist/anchorkit-<VERSION>.tar.gz
release:
	@bash scripts/package_release.sh $(VERSION)

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
