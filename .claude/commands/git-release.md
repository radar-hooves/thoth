---
description: Thoth Release Workflow
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# Thoth Release Workflow

Execute a complete Thoth release using CalVer versioning and GitHub Actions CI/CD.

## Core Principles

A Thoth release is:

1. **CalVer-based**: Version is `YYYY.M.P` (current date in AEST)
2. **CI-built**: GitHub Actions builds, signs, and creates draft release
3. **Draft-first**: Review artifacts and edit release notes before publishing
4. **Auto-updatable**: Published releases trigger auto-updates for users

## Execution Steps

### 1. Determine New Version

**CRITICAL**: Always use current date in AEST (Australia/Brisbane, UTC+10)

Compute the next version from the last tag — never count the patch number by hand. Hand-counting is what produced the June 2026 regression, where the in-tree version ran to `2026.6.10` and was then reset to `2026.6.2`.

```bash
# Last released version, and the current calendar month in AEST
LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "v0.0.0")
LAST_VERSION="${LAST_TAG#v}"
THIS_MONTH=$(TZ=Australia/Brisbane date +"%Y.%-m")

# Same month as the last release -> patch bump; new month -> reset patch to 0
if [[ "$LAST_VERSION" == "$THIS_MONTH".* ]]; then
  NEXT_VERSION="$THIS_MONTH.$(( ${LAST_VERSION##*.} + 1 ))"
else
  NEXT_VERSION="$THIS_MONTH.0"
fi
echo "Last: $LAST_VERSION  ->  Next: $NEXT_VERSION"
```

- **Patch bump** (fixes only, same month): `2026.2.0` → `2026.2.1`
- **Month bump** (first release of a new month): `2026.2.1` → `2026.3.0`

Ask the user to confirm `$NEXT_VERSION`. The bump script re-checks monotonicity and refuses anything that is not strictly greater than both the last tag and the current in-tree version, so a miscount fails loudly rather than shipping.

### 2. Review Changes Since Last Release

```bash
# Find most recent tag
git describe --tags --abbrev=0 2>/dev/null || echo "No tags"

# Show changes since last tag (or all if no tags)
LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || git rev-list --max-parents=0 HEAD)
git log $LAST_TAG..HEAD --oneline --no-merges

# Show full diff
git diff $LAST_TAG..HEAD
```

Analyse changes and summarise for release notes:

- Breaking changes (lead with these)
- New features
- Bug fixes
- Internal changes (optional)

### 3. Rename the CHANGELOG's `[Unreleased]` heading

`bump-version.sh` deliberately never touches `CHANGELOG.md` (release notes are hand-written prose, not a mechanical rewrite) — so this step is manual, and skipping it is not caught until `cargo test` fails on `changelog::tests::releases_come_back_newest_first`, which asserts the CHANGELOG's newest entry equals the crate's own version.

In `CHANGELOG.md`, rename `## [Unreleased]` to `## [<VERSION>] - <today, AEST>` using the entries already filed under it since the last release (do this in the same commit as the version bump, since both need the same version and date), then add a fresh empty `## [Unreleased]` above it for whatever lands next.

### 4. Run Bump Script

```bash
./scripts/bump-version.sh <VERSION>
```

The script is the single authority on which files carry a version — do not restate the list here, or it drifts (see the "Single source of truth" rule in `.claude/CLAUDE.md`). `flake.nix` rotted for exactly this reason: it carried a version, was in nobody's list, and sat at `2026.6.3` while the app shipped `2026.6.7`.

The script refuses to run if:

- the new version is not strictly greater than both the last tag and the current in-tree version; or
- any file it does not rewrite declares the outgoing version — which means a new version-bearing file was added without teaching the script about it.

Both guards run before anything is written, so a failure never leaves a half-bumped tree.

### 5. Review Version Changes

```bash
git diff
```

Verify only version fields changed, in the files the script reports.

### 6. Commit and Tag

```bash
# Stage changes
git add -u

# Commit with standard message
git commit -m "chore(release): bump version to <VERSION>"

# Create tag
git tag v<VERSION>

# Push everything
git push origin main
git push origin v<VERSION>
```

### 7. Monitor CI Build

```bash
# Open actions page
open https://github.com/radar-hooves/thoth/actions
```

Tell user:

1. CI workflow will run (~10-15 minutes)
2. Watch for completion
3. Workflow creates draft release

### 8. Draft Release Instructions

Once CI completes, instruct user to:

1. Go to: https://github.com/radar-hooves/thoth/releases
2. Find draft release for the version
3. Verify artifacts present:
   - `Thoth_<VERSION>_aarch64.dmg`
   - `Thoth_<VERSION>_aarch64.app.tar.gz`
   - `Thoth_<VERSION>_aarch64.app.tar.gz.sig`
   - `latest.json`
4. Download and test the `.dmg` locally
5. Edit release notes with the summary from step 2
6. Publish release when ready

### 9. Post-Release

Remind user:

- Published release triggers auto-updates for users
- Monitor GitHub Issues for update problems
- Consider announcement (README, discussions, etc.)

## Failure Checks

Before pushing (step 6):

- [ ] Version determined using AEST date
- [ ] **Version is strictly greater than the last tag** — `git describe --tags --abbrev=0` must report a version lower than the one being released. Never retag, delete or move a published tag to fix a numbering mistake: released tags are monotonic and rewriting one breaks auto-updates for everyone who already upgraded. Roll forward with the next patch instead.
- [ ] **Version is strictly greater than the previous in-tree version** — a source build (Nix) reports the in-tree version, so an in-tree regression outranks the real release even when tags look fine.
- [ ] Changes reviewed and summarized
- [ ] Bump script ran successfully (it enforces both checks above and exits non-zero otherwise)
- [ ] Only version fields changed (git diff clean)
- [ ] Commit message format correct
- [ ] Tag format is `v<VERSION>`

## When the release build fails

Two failure modes that are not obvious from the workflow log alone.

**No draft release appeared.** Check, in order: the `TAURI_SIGNING_PRIVATE_KEY` secret is present; `cargo check` passes locally; `pnpm build` passes locally; the sherpa-onnx dylibs downloaded (the `download-binaries` feature fetches them, so a network failure on the runner looks like a link error).

**`latest.json` is missing from the artefacts.** The updater manifest is only emitted when `bundle.createUpdaterArtifacts` is `true` in `tauri.conf.json`. Without it the build is green and auto-updates silently never arrive.

Neither is fixed by retagging — see Rollback below. Roll forward.

## Rollback

If critical issues discovered after publishing:

**Option 1: Hotfix Release (Recommended)**

1. Fix issue on `main`
2. Run this workflow again with next patch version
3. Publish immediately

**Option 2: Delete Release (NOT Recommended)** ⚠️ Breaks auto-updates for users who already upgraded Only if no users upgraded AND issue is critical:

```bash
# Delete tag
git push --delete origin v<VERSION>
# Then delete release in GitHub UI
```

## Example Session

```bash
# 1. Determine version (16 Feb 2026 in AEST) — derived, never counted by hand
LAST_VERSION=$(git describe --tags --abbrev=0 | sed 's/^v//')
THIS_MONTH=$(TZ=Australia/Brisbane date +"%Y.%-m")
if [[ "$LAST_VERSION" == "$THIS_MONTH".* ]]; then
  NEXT_VERSION="$THIS_MONTH.$(( ${LAST_VERSION##*.} + 1 ))"
else
  NEXT_VERSION="$THIS_MONTH.0"
fi
# Last: 2026.2.2 -> Next: 2026.2.3

# 2. Review changes
LAST_TAG=$(git describe --tags --abbrev=0)
git log $LAST_TAG..HEAD --oneline

# 3. Bump version
./scripts/bump-version.sh 2026.2.3

# 4. Review
git diff

# 5. Commit and tag
git add -u
git commit -m "chore(release): bump version to 2026.2.3"
git tag v2026.2.3
git push origin main
git push origin v2026.2.3

# 6. Monitor
open https://github.com/radar-hooves/thoth/actions

# 7. Wait for CI, then review draft release
# 8. Publish when ready
```
