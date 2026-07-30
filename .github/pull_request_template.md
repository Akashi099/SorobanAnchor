## Summary

<!-- What does this PR do and why? One to three bullet points. -->

-

## Changes

<!-- List the key files or components changed. -->

-

## Testing

<!-- How was this tested? Check all that apply. -->

- [ ] `cargo test` passes
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] WASM build passes (`cargo build --target wasm32-unknown-unknown`)
- [ ] Manual testing (describe below)

<!-- Describe any manual testing steps or edge cases verified. -->

## Related issues

<!-- Use "closes #<number>" to auto-close issues on merge. -->

closes #

## Checklist

- [ ] Branch name follows the convention (`<type>/<description>`) — see [branch-pr-hygiene.md](../docs/branch-pr-hygiene.md)
- [ ] PR title follows Conventional Commits format (`feat(scope): subject`)
- [ ] Public API changes are reflected in `api_snapshots/` (run `./scripts/snapshot_api.sh` if needed)
- [ ] `CHANGELOG.md` updated (for user-facing changes)
- [ ] Documentation updated (for new behavior or config options)
- [ ] Security review done if touching auth, crypto, HTTP integrations, or deployment — see [SECURITY_REVIEW_CHECKLIST.md](../docs/SECURITY_REVIEW_CHECKLIST.md)
