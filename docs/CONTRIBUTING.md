# Contributing to AnchorKit

Thank you for contributing to AnchorKit! This guide explains our coding standards and how to ensure your code meets project requirements before submitting a pull request.

## Code Quality Standards

AnchorKit enforces consistent code formatting and quality through automated tools:

- **rustfmt** — Automatic code formatting (configured in `rustfmt.toml`)
- **clippy** — Linting and best practices (configured in `.clippy.toml`)
- **cargo test** — Unit and integration tests

## Pre-Commit Validation

Before committing or pushing code, run the complete validation suite:

```bash
make check
```

This runs:
1. `cargo fmt --all -- --check` — Verify formatting
2. `cargo clippy --all-targets --all-features -- -D warnings` — Lint checks
3. `cargo test` — All tests

### Individual Checks

If you prefer to run checks individually:

```bash
# Check formatting (without modifying files)
make fmt-check

# Auto-fix formatting issues
make fmt

# Run linting checks
make lint

# Run tests
make test

# Build release binary
make build

# Build WASM target
make wasm
```

## Formatting Rules

AnchorKit uses `rustfmt` with the following key rules (see `rustfmt.toml` for full config):

- **Line length:** 100 characters max
- **Indentation:** 4 spaces (no tabs)
- **Imports:** Grouped and reordered (std, external crates, internal)
- **Trailing commas:** Vertical style
- **Comments:** Wrapped at 80 characters

### Auto-formatting

To automatically fix formatting issues:

```bash
cargo fmt --all
```

Or use the Makefile:

```bash
make fmt
```

## Linting Standards

AnchorKit uses `clippy` with strict warnings-as-errors policy. All clippy warnings must be resolved before merging.

Key linting rules:

- **Cognitive complexity:** Max 30 (default: 25)
- **Type complexity:** Max 500 (default: 250)
- **Function arguments:** Max 8 (default: 7)
- **Unsafe blocks:** Discouraged; must be justified with comments
- **Documentation:** Public items should have doc comments

### Common Clippy Warnings

| Warning | Fix |
|---------|-----|
| `clippy::too_many_arguments` | Refactor function to use a struct parameter |
| `clippy::cognitive_complexity` | Break function into smaller helpers |
| `clippy::type_complexity` | Use type aliases or extract to a struct |
| `clippy::needless_borrow` | Remove unnecessary `&` or `&mut` |
| `clippy::match_like_matches_macro` | Use `matches!()` macro |

Run linting checks:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Or use the Makefile:

```bash
make lint
```

## Documentation Standards

All public functions and types should have doc comments:

```rust
/// Brief description of what this function does.
///
/// More detailed explanation if needed. Explain the purpose, behavior,
/// and any important side effects.
///
/// # Arguments
///
/// * `param1` - Description of param1
/// * `param2` - Description of param2
///
/// # Returns
///
/// Description of return value
///
/// # Errors
///
/// Panics or returns errors with:
/// - `ErrorCode::SomeError` - when this condition occurs
///
/// # Examples
///
/// ```rust
/// let result = my_function(arg1, arg2);
/// assert_eq!(result, expected);
/// ```
pub fn my_function(param1: Type1, param2: Type2) -> ReturnType {
    // implementation
}
```

## Architecture Decision Records

Important architectural decisions are captured as Architecture Decision Records
(ADRs) in [`docs/adr/`](adr/). Before making a significant design change, review
the existing ADRs and consider whether a new ADR is needed.

- [ADR-0001](adr/ADR-0001-record-architecture-decisions.md): Record Architecture Decisions
- [ADR-0002](adr/ADR-0002-dual-build-strategy.md): Dual-Build Strategy
- [ADR-0003](adr/ADR-0003-contract-lifecycle-and-schema-versioning.md): Contract Lifecycle and Schema Versioning

## Git Workflow

1. **Create a feature branch:**
   ```bash
   git checkout -b feature/description-of-change
   ```

2. **Make your changes** and commit regularly:
   ```bash
   git add .
   git commit -m "feat: description of change"
   ```

3. **Run validation before pushing:**
   ```bash
   make check
   ```

4. **Push to remote:**
   ```bash
   git push -u origin feature/description-of-change
   ```

5. **Create a pull request** with a clear description

## Commit Message Format

Follow conventional commits:

```
<type>(<scope>): <subject>

<body>

<footer>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `chore`

Example:
```
feat(contract): add new attestation verification

Implement Ed25519 signature verification for attestations
with replay attack detection.

Closes #123
```

## Testing

All new features should include tests:

```bash
cargo test
```

Run tests with output:

```bash
cargo test -- --nocapture
```

Run a specific test:

```bash
cargo test test_name
```

## CI/CD Pipeline

The project uses GitHub Actions for continuous integration. All checks must pass before merging:

- ✓ Formatting check (`cargo fmt -- --check`)
- ✓ Linting (`cargo clippy -- -D warnings`)
- ✓ Tests (`cargo test`)
- ✓ WASM build (`cargo build --target wasm32-unknown-unknown`)
- ✓ Dependency audit (`dependency-audit.yml`)
- ✓ Feature matrix (`feature-matrix.yml` — all feature-flag combinations)

## Security Review

Changes that touch authentication, cryptography, authorization, HTTP
integrations, or the deployment process require a security review in addition
to the standard code review. Use the repository's
[Security Review Checklist](SECURITY_REVIEW_CHECKLIST.md) as a guide.

See [Governance and Security](governance-and-security.md) for the full policy,
including which changes require two maintainer approvals.

## Troubleshooting

### Formatting conflicts

If `cargo fmt` and `clippy` disagree, `rustfmt` takes precedence. Run `cargo fmt` first, then `cargo clippy`.

### Clippy false positives

If you believe a clippy warning is a false positive, you can suppress it with:

```rust
#[allow(clippy::lint_name)]
fn my_function() {
    // implementation
}
```

Always add a comment explaining why the lint is suppressed.

### Performance issues

If `cargo clippy` is slow, you can run it with fewer features:

```bash
cargo clippy --lib -- -D warnings
```

## Questions?

- Check existing issues and PRs
- Review the project README and documentation
- Ask in PR comments or discussions

Thank you for helping make AnchorKit better!
