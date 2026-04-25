# Release automation

Python CLI that drives the Commit-Boost release workflow. Called by CI on every release-request PR and tag push, and runnable locally via `lint`.

For cutting a release, see [`.releases/README.md`](../../../.releases/README.md).

## How a release happens

```
  1. PR adds .releases/v1.2.3.yml (commit SHA + reason)
           │
  2. validate-release-request.yml → release.py validate-pr
     Checks: filename, schema, commit exists, tag free, signatures
           │  (approve + squash-merge)
  3. release-gate.yml → release.py gate
     Re-validates everything, then creates signed tag via GitHub API
           │  (tag push)
  4. release.yml
     Builds linux/darwin binaries, pushes Docker images,
     signs with Sigstore, drafts GitHub Release
```

Steps 2-4 are independent workflow files with independent triggers. GitHub Actions has no cross-workflow `needs`, so the gate re-validates as defense in depth.

## Local usage

Requires `GH_TOKEN` and `REPO` in env. [uv](https://docs.astral.sh/uv/) recommended.

```bash
# Pre-flight check (same validations as CI)
export REPO=commit-boost/commit-boost-client
export GH_TOKEN=$(gh auth token)
uv run --with pyyaml python .github/workflows/release/release.py lint .releases/v1.2.3.yml

# Run tests
uv run --with pyyaml --with pytest pytest .github/workflows/release/test_release.py -v
```

Without uv: `pip install pyyaml pytest` in a venv.

## Subcommands

All exit 0 on success, non-zero on failure.

| Command | Purpose | Used by |
| --- | --- | --- |
| `validate-filename <name>` | Strict semver format check | `validate-pr` |
| `validate-yaml <path>` | Schema check (commit SHA + reason) | `validate-pr`, `gate` |
| `find-added --base <sha> --head <sha>` | List added `.releases/*.yml` files | `validate-pr`, `gate` |
| `check-modifications --base <sha> --head <sha>` | Reject edits/deletions of existing YAMLs | `validate-pr` |
| `check-commit-exists <sha>` | Verify commit exists via GitHub API | `validate-pr`, `gate` |
| `check-tag-free <tag>` | Verify tag does not exist | `validate-pr`, `gate` |
| `check-signatures <commit>` | Verify ancestor commits are signed | `validate-pr`, `gate` |
| `create-tag <tag> <commit>` | Create signed tag via GitHub API | `gate` |
| `is-latest <tag>` | Print `true`/`false` for `:latest` Docker tag | `release.yml` |
| `check-ci <sha>` | Verify CI checks passed | `release.yml` |
| `validate-pr` | Full PR validation (reads env) | `validate-release-request.yml` |
| `gate` | Full re-validation + tag creation (reads env) | `release-gate.yml` |
| `lint <path>` | Local pre-flight check | dev only |

## Layout

```
.github/workflows/release/
├── release.py       # CLI (~475 lines, PyYAML only)
├── test_release.py  # pytest suite (~600 lines)
└── README.md
```

## Troubleshooting

**`pip install pyyaml` fails with "externally-managed-environment"**: PEP 668 on Ubuntu 24.04+. Use uv or `pip install --user pyyaml`.

**`gh: not found`**: `brew install gh && gh auth login`. API subcommands need `GH_TOKEN` in env.

**"No prior tag ancestor found"**: shallow clone or first release. Run `git fetch --tags --unshallow`.
