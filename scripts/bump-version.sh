#!/usr/bin/env bash
#
# One version, bumped in one place. package.json is the source: flake.nix reads
# it, and this script copies it to the files that must hold a literal
# (src-tauri/Cargo.toml, Cargo.lock's entry for the app, tauri.conf.json, which
# release.yaml compares to the tag).
#
# Usage: scripts/bump-version.sh X.Y.Z   set the version everywhere
#        scripts/bump-version.sh --check fail when any copy differs from package.json
# Exit:  0 done or consistent · 1 copies differ · 2 bad usage or input

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
pkg=package.json conf=src-tauri/tauri.conf.json toml=src-tauri/Cargo.toml lock=src-tauri/Cargo.lock

name="$(sed -n 's/^name = "\(.*\)"/\1/p' "$toml" | head -1)"
toml_version() { sed -n 's/^version = "\(.*\)"/\1/p' "$toml" | head -1; }
lock_version() {
  [ -f "$lock" ] || return 0
  awk -v n="$name" '$0 == "name = \"" n "\"" { getline; if ($1 == "version") { gsub(/"/, "", $3); print $3; exit } }' "$lock"
}

case "${1:-}" in
  --check)
    want="$(jq -r .version "$pkg")"
    bad=0
    for pair in "$conf:$(jq -r .version "$conf")" "$toml:$(toml_version)" "$lock:$(lock_version)"; do
      file="${pair%%:*}" got="${pair#*:}"
      [ -f "$file" ] || continue
      [ "$got" = "$want" ] || { echo "$file is $got; $pkg is $want" >&2; bad=1; }
    done
    exit "$bad"
    ;;
  [0-9]*.[0-9]*.[0-9]*)
    version="$1"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "not a semantic version: $version" >&2; exit 2; }
    for file in "$pkg" "$conf"; do
      jq --tab --arg v "$version" '.version = $v' "$file" > "$file.new" && mv "$file.new" "$file"
    done
    awk -v v="$version" '!done && /^version = "/ { $0 = "version = \"" v "\""; done = 1 } { print }' "$toml" > "$toml.new" && mv "$toml.new" "$toml"
    if [ -f "$lock" ]; then
      awk -v n="$name" -v v="$version" '
        $0 == "name = \"" n "\"" { print; getline; if ($1 == "version") $0 = "version = \"" v "\""; print; next }
        { print }' "$lock" > "$lock.new" && mv "$lock.new" "$lock"
    fi
    bash "$0" --check
    ;;
  *)
    echo "usage: $0 X.Y.Z | --check" >&2
    exit 2
    ;;
esac
