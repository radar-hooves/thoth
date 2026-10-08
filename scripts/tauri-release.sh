#!/usr/bin/env bash
# The Mac release build: tauri-action runs this in place of `tauri`. Builds in the
# flake devShell, then refuses to return until every bundle carries the
# certificate APPLE_SIGNING_IDENTITY declares, so nothing unsigned is packaged
# or uploaded.
set -euo pipefail
cd "$(dirname "$0")/.."

nix develop --command bash -c '
    set -euo pipefail
    pnpm install --frozen-lockfile
    pnpm tauri "$@"
' bash "$@"

[[ "${1:-}" == build ]] || exit 0
target=
for ((i = 1; i <= $#; i++)); do
    [[ "${!i}" == --target ]] && { j=$((i + 1)); target=${!j}; }
done
nix develop --command bash scripts/cargo-codesign.sh verify-bundles \
    "src-tauri/target/${target:+$target/}release/bundle"
