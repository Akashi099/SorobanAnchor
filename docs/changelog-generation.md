# Changelog Generation

AnchorKit uses structured (conventional) commits so that release notes can be
generated automatically from repository activity rather than written by hand.

---

## Table of Contents

1. [Commit message format](#1-commit-message-format)
2. [Generating a changelog](#2-generating-a-changelog)
3. [Changelog format](#3-changelog-format)
4. [CI integration](#4-ci-integration)
5. [Manual editing](#5-manual-editing)
6. [Tooling options](#6-tooling-options)

---

## 1  Commit message format

All commits must follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

<body>        # optional

<footer>      # optional — "Closes #N", "BREAKING CHANGE: ..."
```

### Types

| Type | Changelog section |
|------|------------------|
| `feat` | Features |
| `fix` | Bug Fixes |
| `perf` | Performance |
| `docs` | Documentation |
| `refactor` | Refactoring |
| `test` | Tests |
| `chore` | Chores / maintenance |
| `ci` | CI / build |

Commits with `BREAKING CHANGE:` in the footer (or a `!` after the type, e.g.
`feat!`) appear in a separate **Breaking Changes** section regardless of type.

### Scope examples

`contract`, `sep6`, `sep10`, `sep38`, `rate-limiter`, `config`, `cli`, `docs`

### Examples

```
feat(sep38): add firm quote expiry validation

Closes #601

fix(rate-limiter): prevent zero-value config from being accepted

BREAKING CHANGE: set_rate_limit_config now rejects zero window_secs

chore(deps): bump soroban-sdk to 21.0.0
```

---

## 2  Generating a changelog

### Using git-cliff (recommended)

[git-cliff](https://git-cliff.org/) reads conventional commits and produces a
`CHANGELOG.md` without any external service.

#### Install

```bash
cargo install git-cliff
```

#### Generate for the next release

```bash
# All unreleased commits since the last tag
git cliff --unreleased --tag v0.2.0 --output CHANGELOG.md
```

#### Prepend to an existing CHANGELOG.md

```bash
git cliff --unreleased --tag v0.2.0 --prepend CHANGELOG.md
```

#### Full history from the beginning

```bash
git cliff --output CHANGELOG.md
```

### Using the helper script

```bash
# Generate changelog for the next version (reads version from Cargo.toml)
./scripts/generate_changelog.sh

# Specify a version explicitly
./scripts/generate_changelog.sh v0.2.0

# Preview without writing to file
./scripts/generate_changelog.sh --dry-run
```

The script wraps `git-cliff` and applies the project's `cliff.toml`
configuration (see below).

---

## 3  Changelog format

`CHANGELOG.md` is kept in the repo root. Each release section looks like:

```markdown
## [0.2.0] - 2026-07-29

### Breaking Changes

- **rate-limiter:** set_rate_limit_config now rejects zero window_secs (#612)

### Features

- **sep38:** add firm quote expiry validation (#601)
- **cli:** add --dry-run flag to deploy command (#598)

### Bug Fixes

- **rate-limiter:** prevent zero-value config from being accepted (#612)

### Documentation

- add changelog generation guide (#689)
- add API contract snapshot guide (#688)
- add maintainer onboarding guide (#691)
```

---

## 4  CI integration

Add a step to generate and validate the changelog on every PR targeting `main`:

```yaml
# .github/workflows/ci.yml  — changelog job
- name: Validate commit messages
  uses: wagoid/commitlint-github-action@v5
  with:
    configFile: commitlint.config.js

- name: Preview changelog
  run: |
    cargo install git-cliff --quiet
    git cliff --unreleased --tag $(grep '^version' Cargo.toml | \
        head -1 | sed 's/.*= *"\(.*\)"/v\1/') 2>&1
```

For release tags, commit the generated changelog back to the release branch:

```yaml
- name: Generate changelog for release
  if: startsWith(github.ref, 'refs/tags/v')
  run: |
    cargo install git-cliff --quiet
    git cliff --unreleased --tag ${{ github.ref_name }} \
        --prepend CHANGELOG.md
    git config user.name  "github-actions[bot]"
    git config user.email "github-actions[bot]@users.noreply.github.com"
    git add CHANGELOG.md
    git commit -m "chore(release): update changelog for ${{ github.ref_name }}"
    git push
```

---

## 5  Manual editing

Auto-generated changelogs are a starting point, not the final word. Before
tagging a release, review `CHANGELOG.md` and:

- Rewrite cryptic commit subjects into user-facing language.
- Group related fixes under a single entry when several commits address one
  issue.
- Add upgrade notes for breaking changes that need migration steps.
- Link to relevant docs, PRs, or issues where helpful.

Keep manual edits minimal — the goal is a readable summary, not a rewrite.

---

## 6  Tooling options

| Tool | Notes |
|------|-------|
| [git-cliff](https://git-cliff.org/) | Rust-native, configurable via `cliff.toml`, recommended |
| [conventional-changelog-cli](https://github.com/conventional-changelog/conventional-changelog) | Node.js, widely used, more plugins |
| [release-please](https://github.com/googleapis/release-please) | GitHub Action, automates PR + tag + changelog |
| Manual | Always an option; use the format in [§3](#3-changelog-format) |

### cliff.toml (project config)

Place this file at the repo root to configure git-cliff's output:

```toml
[changelog]
header = "# Changelog\n\n"
body   = """
{% if version %}\
## [{{ version | trim_start_matches(pat="v") }}] - {{ timestamp | date(format="%Y-%m-%d") }}
{% else %}\
## [Unreleased]
{% endif %}\
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group | upper_first }}
{% for commit in commits %}\
- {% if commit.scope %}**{{ commit.scope }}:** {% endif %}{{ commit.message }} \
(#{{ commit.id | truncate(length=7, end="") }})
{% endfor %}
{% endfor %}\n
"""
trim = true

[git]
conventional_commits    = true
filter_unconventional   = true
split_commits           = false
commit_parsers = [
  { message = "^feat",     group = "Features" },
  { message = "^fix",      group = "Bug Fixes" },
  { message = "^perf",     group = "Performance" },
  { message = "^docs",     group = "Documentation" },
  { message = "^refactor", group = "Refactoring" },
  { message = "^test",     group = "Tests" },
  { message = "^chore",    group = "Chores" },
  { message = "^ci",       group = "CI" },
]
filter_commits = false
tag_pattern    = "v[0-9].*"
```

---

## References

- [CONTRIBUTING.md](CONTRIBUTING.md) — commit message rules
- [governance-and-security.md](governance-and-security.md) — release approval policy
- [release-signing.md](release-signing.md) — signing artifacts after release
- [api-contract-snapshots.md](api-contract-snapshots.md) — API regression detection
- [ONBOARDING.md](ONBOARDING.md) — maintainer onboarding
