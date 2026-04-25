# Release Requests

This directory contains immutable release-request files. Merging one on main requests a release.

## Filing a release request

1. Pick the commit SHA to release.
2. Create `.releases/<tag>.yml` where the file name is the exact tag to create.
3. Add:

```yaml
commit: <40-character SHA>
reason: "<one-line description>"
```

4. In the same PR:
   - update `CHANGELOG.md`
   - bump the root `Cargo.toml` workspace version to `<next>-dev`
5. Get approvals and merge the PR to main.

## Rules

- Full release: `v1.2.3.yml`
- Pre-release: `v1.2.3-rc1.yml`, `v1.2.3-rc2.yml`, etc.
- Must start with `v`
- Must be strict semver (optionally with `-rcN`)
- Must use `.yml`

## Constraints enforced by CI

- Exactly one release-request file may be added per PR
- Existing release-request files cannot be modified or deleted
- The referenced commit must exist in the repository
- The tag must not already exist
- The referenced commit may be on main or on an off-main hotfix branch

## Hotfix releases

A release commit does not need to be on main.

Typical flow:
1. Branch from the last release tag: `git checkout -b fix/<name> vX.Y.Z`
2. Land fixes on that branch
3. Open a PR on main adding `.releases/vA.B.C.yml` that points at the hotfix branch tip commit
4. Merge the release-request PR on main
5. After the release ships, reconcile the hotfix branch back into main separately

## After merge

1. `release-gate.yml` re-validates the request and creates the signed tag via the GitHub App
2. `release.yml` builds binaries, pushes Docker images, signs artifacts, and drafts the GitHub Release
3. GHCR `:latest` moves only if the new tag is the highest non-RC version

## Operational note

Release-request files are immutable after merge. If a release attempt is botched, use the next version number and explain the gap in the changelog if needed.
