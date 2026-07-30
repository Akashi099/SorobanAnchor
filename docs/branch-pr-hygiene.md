# Branch and PR Hygiene

This document defines branch naming conventions, PR workflow, and the automation that supports them. Following these guidelines keeps the repository clean and makes reviews faster.

## Branch naming

All branches must follow this pattern:

```
<type>/<short-description>
```

| Type | When to use |
|------|-------------|
| `feat/` | New feature or enhancement |
| `fix/` | Bug fix |
| `docs/` | Documentation only |
| `chore/` | Maintenance, dependency updates, CI changes |
| `refactor/` | Code restructuring without behavior change |
| `test/` | Test-only changes |
| `release/` | Release preparation |

**Rules:**
- Use lowercase and hyphens, no underscores or spaces.
- Keep the description short (≤ 40 characters after the prefix).
- Include the issue number when one exists: `feat/add-roadmap-696`.

**Valid examples:**
```
feat/add-roadmap-696
fix/jwt-expiry-parsing
docs/triage-workflow-697
chore/update-cargo-deps
```

**Invalid examples:**
```
feature_new_thing        # wrong type prefix and uses underscores
Fix/JwtExpiry            # uppercase
my-branch                # no type prefix
```

To validate your branch name before pushing, run:

```bash
./scripts/validate-branch-name.sh
```

---

## Pull request workflow

### Before opening a PR

1. Run the full validation suite locally:
   ```bash
   make check
   ```
2. Ensure your branch is up to date with `main`:
   ```bash
   git fetch origin && git rebase origin/main
   ```
3. Verify your branch name follows the convention above.

### Opening a PR

- The [PR template](.github/pull_request_template.md) is filled in automatically — complete every section.
- Title format: `<type>(<scope>): <subject>` following [Conventional Commits](CONTRIBUTING.md#commit-message-format).
- Link the issue(s) being addressed using `closes #<number>` in the PR description so they close automatically on merge.
- Keep PRs focused: one logical change per PR. Split unrelated fixes into separate PRs.

### Review expectations

- A PR needs **one** maintainer approval to merge, or **two** for changes touching auth, crypto, or deployment (see [Governance and Security](governance-and-security.md)).
- Address all review comments or explain why they do not apply.
- Do not force-push to a PR branch that is under review — add fixup commits instead.

### Merging

- Prefer **squash and merge** for feature and fix branches to keep `main` history linear.
- Use **merge commit** only for release branches.
- Delete the source branch after merging — GitHub can be configured to do this automatically.

---

## Keeping branches clean

- **Delete merged branches** promptly. GitHub's "delete branch on merge" setting automates this.
- **Do not leave stale branches** open for more than 30 days without activity. Maintainers may close PRs with no activity after this period with a `stale` label warning.
- **Rebase, don't merge** `main` into your feature branch — this avoids noisy merge commits.

---

## Automation

### Branch name validator

`scripts/validate-branch-name.sh` checks the current branch name against the naming convention. Run it manually or add it to your local pre-push hook:

```bash
# Add to your local pre-push hook
echo './scripts/validate-branch-name.sh' >> .git/hooks/pre-push
chmod +x .git/hooks/pre-push
```

### PR template

`.github/pull_request_template.md` is loaded automatically when you open a PR on GitHub. It prompts for a summary, test plan, and issue references.

### Setup hooks

To install all recommended local git hooks at once:

```bash
./scripts/setup-hooks.sh
```

---

For the full contribution guide, see [CONTRIBUTING.md](CONTRIBUTING.md).
