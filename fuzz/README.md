# AnchorKit Fuzzing Suite

This directory contains fuzzing harnesses for the AnchorKit parser and validator modules. Fuzzing helps uncover edge cases and security issues by feeding randomized input to critical parsing functions.

## Fuzz Targets

### `json_response_parsing`
Tests the robustness of JSON parsing for API responses (SEP-6, SEP-24, SEP-38).
Feeds arbitrary byte sequences and verifies the parser doesn't panic or produce invalid state.

### `config_parsing`
Tests configuration file parsing (JSON and TOML formats).
Ensures the runtime configuration loader handles malformed input gracefully.

### `jwt_parsing`
Tests JWT component parsing and base64url decoding.
Critical for SEP-10 authentication flows where untrusted JWTs are processed.

### `domain_validation`
Tests domain validation logic against arbitrary input.
Ensures domain validators reject invalid input without panicking.

## Running Fuzzing

### Prerequisites
Install `cargo-fuzz`:
```bash
cargo install cargo-fuzz
```

### Run All Fuzz Targets
```bash
# Run fuzzing with default settings (interactive, until interrupted)
cd fuzz
cargo fuzz run fuzz_target_json_response_parsing

cargo fuzz run fuzz_target_config_parsing

cargo fuzz run fuzz_target_jwt_parsing

cargo fuzz run fuzz_target_domain_validation
```

### Run with Options
```bash
# Run for a fixed number of iterations
cargo fuzz run fuzz_target_json_response_parsing -- -max_len=4096 -runs=10000

# Run with a corpus seed
cargo fuzz run fuzz_target_json_response_parsing -- corpus/

# Run in debug mode (slower, more information)
RUSTFLAGS="-g" cargo fuzz run fuzz_target_json_response_parsing
```

### Continuous Integration
Add to your CI pipeline to run fuzzing periodically:
```bash
cd fuzz
for target in fuzz_target_*; do
    echo "Fuzzing $target..."
    cargo fuzz run "$target" -- -max_len=8192 -runs=100000
    if [ $? -ne 0 ]; then
        echo "Fuzzing failure in $target"
        exit 1
    fi
done
```

## Interpreting Results

### Crash Artifacts
When a crash is found, libfuzzer saves the input to `fuzz/artifacts/`:
- Examine the crashing input
- Add it as a regression test in the test suite
- Fix the underlying parsing bug

### Corpus Building
libfuzzer automatically maintains a corpus of interesting inputs:
- Located in `fuzz/corpus/fuzz_target_*/`
- Commit the corpus to git to preserve coverage gains
- Share across team members for faster fuzzing runs

## References
- [cargo-fuzz Guide](https://rust-fuzz.github.io/book/cargo-fuzz.html)
- [libfuzzer Documentation](https://llvm.org/docs/LibFuzzer/)
- [Rust Security Guidelines](https://anssi-fr.github.io/rust-guide/)
