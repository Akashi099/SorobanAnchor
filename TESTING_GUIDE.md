# AnchorKit Testing Guide

This guide covers the comprehensive testing infrastructure for AnchorKit, including property-based testing, fuzzing, and feature-flag matrix testing.

## Overview

AnchorKit uses three layers of testing to ensure robustness:

1. **Property-Based Testing** — Automated generation of test inputs to uncover edge cases
2. **Fuzzing** — Continuous random input generation to find crashes and security issues
3. **Feature-Flag Matrix Testing** — Validation across all supported build configurations

## Property-Based Testing (#643)

Property-based testing generates randomized valid inputs and verifies the system behaves correctly under all conditions.

### Running Property-Based Tests

```bash
# Run all property-based tests
cargo test --test property_based_parsing_tests

# Run specific property test
cargo test --test property_based_parsing_tests prop_sep6_deposit_response_with_valid_fields

# Run with verbose output
cargo test --test property_based_parsing_tests -- --nocapture

# Run with multiple samples (default is 256)
PROPTEST_CASES=1000 cargo test --test property_based_parsing_tests
```

### Test Coverage

The property-based test suite covers:

- **SEP-6 Deposit Responses** — Transaction IDs, statuses, addresses, amounts
- **SEP-6 Withdrawal Responses** — Minimal and full field combinations
- **SEP-24 Transactions** — Memos, transaction states
- **SEP-38 Quotes** — Price representations, amounts
- **Domain Validation** — Arbitrary domains, case sensitivity
- **Response Schema Versioning** — Version resolution and ordering
- **JSON Payload Robustness** — Extra fields, idempotency, numeric bounds
- **Error Responses** — Simple and nested error structures

### Example Property Test

```rust
#[test]
fn prop_sep6_deposit_response_with_valid_fields() {
    proptest!(|(
        txn_id in valid_transaction_id(),
        status in valid_status(),
        address in valid_address(),
        how in valid_json_string(),
    )| {
        let response = json!({
            "id": txn_id,
            "type": "deposit",
            "status": status,
            "deposit_address": address,
            "how": how,
        });

        // Property: Parsing should never panic
        let serialized = serde_json::to_string(&response)
            .expect("must serialize");
        let _reparsed: Value = serde_json::from_str(&serialized)
            .expect("must deserialize");
    });
}
```

### When to Add Property Tests

Add property-based tests when:
- The function processes structured input with many valid variations
- There are invariants that should hold for ALL valid inputs
- Edge cases are hard to enumerate manually
- You want to verify round-trip behavior (serialize → deserialize)

### Proptest Documentation

- [proptest Guide](https://docs.rs/proptest/latest/proptest/)
- [Strategy Documentation](https://docs.rs/proptest/latest/proptest/strategy/trait.Strategy.html)

---

## Fuzzing (#644)

Fuzzing feeds randomized data to parsers and validates they handle invalid input without crashing or entering undefined behavior.

### Fuzz Targets

Located in `fuzz/fuzz_targets/`:

1. **json_response_parsing** — Tests JSON parsing robustness
2. **config_parsing** — Tests configuration file parsing (JSON/TOML)
3. **jwt_parsing** — Tests JWT component parsing and base64url decoding
4. **domain_validation** — Tests domain validation against arbitrary input

### Running Fuzzing

```bash
# Install cargo-fuzz (one-time)
cargo install cargo-fuzz

# Run a specific fuzz target
cd fuzz
cargo fuzz run fuzz_target_json_response_parsing

# Run with specific options
cargo fuzz run fuzz_target_jwt_parsing -- -max_len=4096 -runs=100000

# Run until a crash is found (will interrupt)
cargo fuzz run fuzz_target_config_parsing

# Run all fuzz targets
for target in fuzz_target_*; do
    echo "Fuzzing $target..."
    cargo fuzz run "$target" -- -max_len=8192 -runs=50000
done
```

### Interpreting Fuzzing Results

When a crash is found, libfuzzer creates an artifact:

```
fuzz/artifacts/fuzz_target_json_response_parsing/<hash>
```

### Steps to Fix a Fuzzing Crash

1. **Identify the crashing input**
   ```bash
   cargo fuzz run fuzz_target_json_response_parsing -- fuzz/artifacts/fuzz_target_json_response_parsing/<hash>
   ```

2. **Add a regression test**
   ```rust
   #[test]
   fn regression_fuzzing_crash_xyz() {
       let input = b"<crashing input>";
       // Verify parser handles it gracefully
   }
   ```

3. **Fix the underlying bug**
   - Ensure the parser handles all inputs without panicking
   - Use proper error handling instead of unwrap()

4. **Verify the fix**
   ```bash
   cargo fuzz run fuzz_target_json_response_parsing
   ```

### Corpus and Artifacts

- **Corpus**: `fuzz/corpus/fuzz_target_*/` — Interesting inputs discovered during fuzzing
- **Artifacts**: `fuzz/artifacts/fuzz_target_*/` — Inputs that caused crashes

Commit the corpus to git to preserve coverage gains across team members.

### CI Integration

Add to your CI pipeline:

```bash
#!/bin/bash
cd fuzz
for target in fuzz_target_*; do
    cargo fuzz run "$target" -- -max_len=8192 -runs=100000 || exit 1
done
```

---

## Feature-Flag Matrix Testing (#642)

The AnchorKit project supports multiple build configurations. The feature-flag matrix test suite ensures all combinations compile and work correctly.

### Supported Configurations

| Configuration | Command | Use Case |
|---|---|---|
| **Default (std)** | `cargo build --release` | Native servers and CLIs |
| **Explicit std** | `cargo build --release --features std` | Native builds with explicit feature |
| **std + mock-only** | `cargo build --features std,mock-only` | Testing with fixtures |
| **std + stress-tests** | `cargo build --features std,stress-tests` | Load testing |
| **mock-only** | `cargo build --no-default-features --features mock-only` | Test fixtures only |
| **stress-tests** | `cargo build --no-default-features --features stress-tests` | Load testing without std |
| **WASM** | `cargo build --target wasm32-unknown-unknown --no-default-features --features wasm` | On-chain Soroban deployment |

### Running Feature Matrix Tests

```bash
# Run quick compilation checks for all combinations
./scripts/test_feature_matrix.sh --quick

# Run full tests (including cargo test for each combination)
./scripts/test_feature_matrix.sh --full

# Run matrix tests from cargo
cargo test --test feature_flag_matrix_tests --release

# Test a specific configuration
cargo build --release --features std,mock-only
cargo test --features std,mock-only
```

### Feature Constraints

| Feature | Incompatible With | Reason |
|---|---|---|
| **std** | wasm | Mutually exclusive (std requires native runtime) |
| **wasm** | std | Requires --no-default-features |
| **mock-only** | (none) | Safe to combine with any other feature |
| **stress-tests** | (none) | Safe to combine with any other feature |

### Feature Documentation

Each feature flag in `Cargo.toml` has clear documentation:

```toml
# std: enables standard library support (networking, filesystem, threads).
#      Enabled by default for native builds. Incompatible with wasm feature.
std = ["clap", "reqwest", "aes-gcm", "argon2", "rpassword", "rand/std"]

# wasm: targets wasm32-unknown-unknown for Soroban on-chain deployment.
#       Disables std, CLI, and any host-only modules (main.rs, examples).
#       Must be built with --no-default-features.
wasm = []

# mock-only — Pre-built response fixtures for testing without a live anchor.
mock-only = []

# stress-tests — Load-simulation integration test suite.
stress-tests = []
```

### Matrix Test Invariants

The feature_flag_matrix_tests.rs file verifies:

1. ✓ Default features are set correctly
2. ✓ std and wasm are mutually exclusive
3. ✓ mock-only is optional and available with all valid combinations
4. ✓ stress-tests is optional and available with all valid combinations
5. ✓ mock-only has no production use warnings

### Adding New Features

When adding a new feature flag:

1. Add to `Cargo.toml` with clear documentation
2. Add invariant tests in `feature_flag_matrix_tests.rs`
3. Update the feature matrix script in `scripts/test_feature_matrix.sh`
4. Add example build commands to the test file

---

## Integration with CI/CD

### GitHub Actions Example

```yaml
name: Test Suite

on: [push, pull_request]

jobs:
  property-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --test property_based_parsing_tests

  feature-matrix:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: dtolnay/rust-toolchain@stable
      - run: ./scripts/test_feature_matrix.sh --quick

  fuzzing:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo install cargo-fuzz
      - run: cd fuzz && for t in fuzz_target_*; do cargo fuzz run "$t" -- -max_len=4096 -runs=10000 || exit 1; done
```

---

## Best Practices

### For New Parsing/Validation Code

1. **Add property-based tests** when introducing new parsers
   - Define valid input strategies
   - Verify invariants hold across all generated inputs

2. **Add fuzz targets** for public parsing APIs
   - Create fuzz target in `fuzz/fuzz_targets/`
   - Ensure parser never panics on arbitrary input

3. **Update feature matrix** if the code is feature-gated
   - Add tests to `feature_flag_matrix_tests.rs`
   - Verify compilation for all configurations

### For Existing Code

1. **Property tests complement unit tests**
   - Unit tests: specific known cases
   - Property tests: general invariants and edge cases

2. **Fuzzing finds crashes and panics**
   - Run periodically in CI
   - Fix all crashes with regression tests

3. **Feature matrix ensures consistency**
   - Run before shipping new features
   - Prevents accidental feature conflicts

---

## Troubleshooting

### Property Test Failures

**Issue**: Test fails with "Strategy value generation"
**Solution**: Reduce `PROPTEST_CASES` or adjust strategy bounds

```bash
PROPTEST_CASES=100 cargo test --test property_based_parsing_tests
```

### Fuzzing Timeouts

**Issue**: Fuzzing runs too long or finds timeout crashes
**Solution**: Reduce max input length or increase timeout

```bash
cargo fuzz run fuzz_target_json_response_parsing -- -max_len=1024 -timeout=5
```

### Feature Compilation Errors

**Issue**: Feature combination doesn't compile
**Solution**: Check feature gate guards and dependencies

```bash
cargo check --no-default-features --features wasm
```

---

## References

- [proptest Documentation](https://docs.rs/proptest/)
- [cargo-fuzz Guide](https://rust-fuzz.github.io/book/cargo-fuzz.html)
- [LLVM libfuzzer](https://llvm.org/docs/LibFuzzer/)
- [Rust Book - Testing](https://doc.rust-lang.org/book/ch11-00-testing.html)

---

## Contact & Support

For issues or questions about the testing infrastructure:
1. Check this guide and linked documentation
2. Review existing test examples
3. Open an issue with `[testing]` tag
