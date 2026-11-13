#!/usr/bin/env bash
set -euo pipefail

# Tests for scripts/bump-version.sh's release guards.
#
# Each case builds a throwaway git repo with the same layout the real script
# expects, so the guards are exercised for real rather than mocked. Run:
#
#   ./scripts/test-bump-version.sh

SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/bump-version.sh"
[ -x "$SCRIPT" ] || {
    echo "bump-version.sh not found or not executable at $SCRIPT" >&2
    exit 1
}

pass=0
fail=0

# Build a fixture repo at $1 carrying version $2, with a tag $3 (empty for none).
make_fixture() {
    local dir="$1" version="$2" tag="$3"
    mkdir -p "$dir/src-tauri"
    printf '[package]\nname = "thoth"\nversion = "%s"\n' "$version" >"$dir/src-tauri/Cargo.toml"
    printf '{\n  "productName": "Thoth",\n  "version": "%s"\n}\n' "$version" >"$dir/src-tauri/tauri.conf.json"
    printf '{\n  "name": "thoth-tauri",\n  "version": "%s"\n}\n' "$version" >"$dir/package.json"

    git -C "$dir" init -q
    git -C "$dir" config user.email test@example.com
    git -C "$dir" config user.name test
    git -C "$dir" add -A
    git -C "$dir" commit -qm init
    [ -n "$tag" ] && git -C "$dir" tag "$tag"
    return 0
}

# run_case <name> <expected-exit: ok|fail> <version> <tag> [extra-file] [extra-content]
run_case() {
    local name="$1" expect="$2" new_version="$3" tag="$4" extra_file="${5:-}" extra_content="${6:-}"
    local dir
    dir=$(mktemp -d)
    # shellcheck disable=SC2064
    trap "rm -rf '$dir'" RETURN

    make_fixture "$dir" "2026.6.7" "$tag"

    if [ -n "$extra_file" ]; then
        mkdir -p "$dir/$(dirname "$extra_file")"
        printf '%s\n' "$extra_content" >"$dir/$extra_file"
        git -C "$dir" add -A
        git -C "$dir" commit -qm extra
    fi

    local out status
    set +e
    out=$(cd "$dir" && bash "$SCRIPT" "$new_version" 2>&1)
    status=$?
    set -e

    local ok=false
    if [ "$expect" = "ok" ] && [ $status -eq 0 ]; then ok=true; fi
    if [ "$expect" = "fail" ] && [ $status -ne 0 ]; then ok=true; fi

    if $ok; then
        printf '  ok   %s\n' "$name"
        pass=$((pass + 1))
    else
        printf '  FAIL %s (expected %s, exit %d)\n' "$name" "$expect" "$status"
        printf '       %s\n' "$out"
        fail=$((fail + 1))
    fi
}

echo "Guard 1: monotonicity"
run_case "rejects a lower version than the last tag" fail 2026.6.2 v2026.6.7
run_case "rejects the same version as the last tag" fail 2026.6.7 v2026.6.7
run_case "rejects a lower month" fail 2026.5.9 v2026.6.7
run_case "rejects a lower year" fail 2025.9.9 v2026.6.7
run_case "accepts the next patch" ok 2026.6.8 v2026.6.7
run_case "accepts a new month" ok 2026.7.0 v2026.6.7
run_case "compares numerically, not lexically (10 > 9)" ok 2026.6.10 v2026.6.9
run_case "skips the tag check when no tags exist" ok 2026.6.8 ""

# The June 2026 regression was in-tree only: the tag lagged behind while the
# working tree had run ahead and was then reset. A tag-only guard accepts it.
echo "Guard 1: in-tree regression (the June 2026 case)"
run_case "rejects a reset below the in-tree version even when the tag is older" \
    fail 2026.6.2 v2026.6.1

echo "Guard 2: stray version declarations"
run_case "rejects an unlisted file declaring the outgoing version" \
    fail 2026.6.8 v2026.6.7 "flake.nix" '  version = "2026.6.7";'
run_case "rejects an unlisted JSON file declaring the outgoing version" \
    fail 2026.6.8 v2026.6.7 "some/meta.json" '  "version": "2026.6.7",'
run_case "allows CHANGELOG.md to mention the outgoing version" \
    ok 2026.6.8 v2026.6.7 "CHANGELOG.md" '## [2026.6.7] - 2026-06-25'
run_case "allows a lockfile to restate the outgoing version" \
    ok 2026.6.8 v2026.6.7 "src-tauri/Cargo.lock" 'name = "thoth"
version = "2026.6.7"'
run_case "allows prose mentioning the outgoing version" \
    ok 2026.6.8 v2026.6.7 "docs/notes.md" 'It sat at 2026.6.3 while the app shipped 2026.6.7.'
run_case "allows an unrelated dependency version" \
    ok 2026.6.8 v2026.6.7 "flake.nix" '  version = "1.13.2";'

echo "Rewrite still works"
run_case "accepts a valid bump with a clean tree" ok 2026.6.8 v2026.6.7

echo "Cargo.lock follows the bump"
lockdir=$(mktemp -d)
make_fixture "$lockdir" "2026.6.7" v2026.6.7
printf '[[package]]\nname = "other"\nversion = "2026.6.7"\n\n[[package]]\nname = "thoth"\nversion = "2026.6.7"\n' >"$lockdir/src-tauri/Cargo.lock"
git -C "$lockdir" add -A && git -C "$lockdir" commit -qm lock
check() { if "$@"; then printf '  ok   %s\n' "$name"; pass=$((pass + 1)); else printf '  FAIL %s\n' "$name"; fail=$((fail + 1)); fi; }
name="bump rewrites thoth's lock entry"
check bash -c "cd '$lockdir' && bash '$SCRIPT' 2026.6.8 >/dev/null && bash '$SCRIPT' --check >/dev/null && grep -A1 'name = \"thoth\"' src-tauri/Cargo.lock | grep -q 2026.6.8"
name="bump leaves other packages' versions alone"
check bash -c "cd '$lockdir' && grep -A1 'name = \"other\"' src-tauri/Cargo.lock | grep -q 2026.6.7"
name="--check fails when the lock is stale"
sed -i.bak 's/^version = "2026.6.8"/version = "2026.6.9"/' "$lockdir/src-tauri/Cargo.toml"
check bash -c "! (cd '$lockdir' && bash '$SCRIPT' --check >/dev/null 2>&1)"
rm -rf "$lockdir"

echo
echo "passed: $pass, failed: $fail"
[ "$fail" -eq 0 ]
