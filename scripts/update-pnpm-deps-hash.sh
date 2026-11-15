#!/usr/bin/env bash
#
# Regenerate flake.nix's pnpmDeps hash after pnpm-lock.yaml changes.
#
# pnpmDeps is a fixed-output derivation: Nix needs its content hash up front, a
# lockfile change invalidates it, and nothing derives it from the lockfile the
# way cargoLock derives Cargo's from Cargo.lock. An app's own workflow runs this
# on a pull request and pushes the correction, where it wants a dependency bump
# to need nobody to run Nix by hand.
#
# It always builds against a deliberately wrong hash first. A fixed-output path
# is addressed by hash and name, so a store already holding the old hash's path
# skips the fetch and reports success against a stale hash; the wrong hash has
# no path, which forces a real fetch.
#
# A flake that packages no pnpm dependencies (a first stamp's devShell-only
# flake) has nothing to refresh.
#
# Usage: scripts/update-pnpm-deps-hash.sh [--check]
# Exit:  0 correct, updated or absent · 1 stale (--check) · 2 could not run

set -euo pipefail

# The enclosing worktree before the script's own location: CI runs the base
# branch's copy from a file beside this one.
if ! REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"; then
  REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
FLAKE="${REPO_ROOT}/flake.nix"

# lib.fakeHash: recognisable in a diff if the script dies midway.
FAKE_HASH="sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="

CHECK_ONLY=0
if [ "${1:-}" = "--check" ]; then
  CHECK_ONLY=1
elif [ -n "${1:-}" ]; then
  echo "usage: $0 [--check]" >&2
  exit 2
fi

# Every edit stays inside the pnpmDeps block: a flake carries other `hash =`
# lines (a git dependency's, the cargo outputHashes) that a file-wide
# substitution would corrupt.
readonly BLOCK_START='pnpmDeps = pkgs.fetchPnpmDeps {'

if ! grep -qF "$BLOCK_START" "$FLAKE"; then
  echo "flake.nix packages no pnpm dependencies; nothing to refresh."
  exit 0
fi

if ! command -v nix >/dev/null 2>&1; then
  echo "error: nix is not on PATH" >&2
  exit 2
fi

current_hash() {
  sed -n "/${BLOCK_START}/,/};/ s|.*hash = \"\\(sha256-[^\"]*\\)\";.*|\\1|p" "$FLAKE"
}

set_hash() {
  sed -i "/${BLOCK_START}/,/};/ s|hash = \"sha256-[^\"]*\";|hash = \"$1\";|" "$FLAKE"
}

ORIGINAL="$(current_hash)"
if [ -z "$ORIGINAL" ]; then
  echo "error: no hash inside the '${BLOCK_START}' block of $FLAKE" >&2
  exit 2
fi

# Any exit from here on puts the recorded hash back, so a failed run never
# leaves the fake one in the tree.
trap 'set_hash "$ORIGINAL"' EXIT

echo "Recorded pnpmDeps hash: $ORIGINAL"
set_hash "$FAKE_HASH"

# The build fails by design; the hash it reports is the payload.
build_log="$(nix build "${REPO_ROOT}#default.pnpmDeps" --no-link 2>&1 || true)"
ACTUAL="$(printf '%s\n' "$build_log" | sed -n 's|.*got: *\(sha256-[A-Za-z0-9+/=]*\).*|\1|p' | head -1)"

if [ -z "$ACTUAL" ]; then
  echo "error: the build reported no hash; flake.nix keeps $ORIGINAL." >&2
  printf '%s\n' "$build_log" >&2
  exit 2
fi

if [ "$ACTUAL" = "$ORIGINAL" ]; then
  echo "The hash is correct."
  exit 0
fi

echo "The hash is stale: recorded $ORIGINAL, actual $ACTUAL."
if [ "$CHECK_ONLY" -eq 1 ]; then
  echo "Run scripts/update-pnpm-deps-hash.sh to fix it." >&2
  exit 1
fi

trap - EXIT
set_hash "$ACTUAL"
echo "Updated $FLAKE to $ACTUAL."
