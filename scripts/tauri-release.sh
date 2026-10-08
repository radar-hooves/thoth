#!/usr/bin/env bash
# The Mac release build: tauri-action runs this in place of `tauri`. Compiles in
# the flake devShell (rustc from the rustup toolchain cannot load proc macros
# once tauri sets MACOSX_DEPLOYMENT_TARGET on macOS 27) but links with Xcode's
# own clang and ld (nix's linker cannot read the Xcode SDK's .tbd stubs). Then
# refuses to return until every bundle carries the certificate
# APPLE_SIGNING_IDENTITY declares, so nothing unsigned is packaged or uploaded.
set -euo pipefail
cd "$(dirname "$0")/.."

nix develop --command bash -c '
    set -euo pipefail
    xcode=$(env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcode-select -p)
    export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/cc
    export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=$xcode/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld"
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
