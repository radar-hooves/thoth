#!/usr/bin/env bash
# The Mac release build: tauri-action runs this in place of `tauri`. Builds with
# huginn's own Xcode toolchain (nix's linker cannot read the Xcode SDK's .tbd
# stubs), then refuses to return until every bundle carries the certificate
# APPLE_SIGNING_IDENTITY declares, so nothing unsigned is packaged or uploaded.
set -euo pipefail
cd "$(dirname "$0")/.."

# The runner is a launchd agent with a bare PATH.
export PATH="$HOME/.cargo/bin:/etc/profiles/per-user/$(id -un)/bin:$PATH"

pnpm install --frozen-lockfile
pnpm tauri "$@"

[[ "${1:-}" == build ]] || exit 0
target=
for ((i = 1; i <= $#; i++)); do
    [[ "${!i}" == --target ]] && { j=$((i + 1)); target=${!j}; }
done
bash scripts/cargo-codesign.sh verify-bundles "src-tauri/target/${target:+$target/}release/bundle"
