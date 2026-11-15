#!/usr/bin/env bash
#
# Set flake.nix's kitHash to the source tree of the app-factory revision
# Cargo.lock pins. The three kit crates (desktop-shell, telemetry,
# tauri-plugin-telemetry) are git dependencies of one repository, so their
# outputHashes share this one value; run it after every kit bump.
#
# Usage: scripts/update-kit-hash.sh [--check]
# Exit:  0 correct or updated · 1 stale (--check) · 2 could not run

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
flake="$root/flake.nix" lock="$root/src-tauri/Cargo.lock"

grep -qE '^[[:space:]]*kitHash = ' "$flake" || { echo "flake.nix has no kitHash; nothing to refresh."; exit 0; }
[ -f "$lock" ] || { echo "error: $lock missing" >&2; exit 2; }
command -v nix >/dev/null || { echo "error: nix is not on PATH" >&2; exit 2; }

# A stamp pins `rev =` and an app moving onto the stamp may still pin `tag =`;
# either way the commit is the hash after `#`.
rev="$(sed -n 's|^source = "git+https://github.com/radar-hooves/app-factory?[^#]*#\([0-9a-f]*\)"|\1|p' "$lock" | sort -u)"
[ "$(printf '%s\n' "$rev" | grep -c .)" = 1 ] || { echo "error: Cargo.lock pins no single app-factory revision" >&2; exit 2; }

want="$(nix flake prefetch --json "git+https://github.com/radar-hooves/app-factory?rev=$rev" | jq -r .hash)"
have="$(sed -n 's|^[[:space:]]*kitHash = "\(sha256-[^"]*\)";|\1|p' "$flake")"

[ "$want" != "$have" ] || { echo "kitHash is correct ($rev)."; exit 0; }
echo "kitHash is stale: recorded $have, $rev is $want."
if [ "${1:-}" = "--check" ]; then exit 1; fi
sed -i.bak "s|^\([[:space:]]*\)kitHash = \"sha256-[^\"]*\";|\1kitHash = \"$want\";|" "$flake" && rm "$flake.bak"
echo "Updated flake.nix."
