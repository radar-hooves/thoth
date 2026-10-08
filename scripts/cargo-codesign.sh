#!/usr/bin/env bash
# Tauri's cargo runner: build, sign, verify, then execute without re-linking.
set -euo pipefail
fail() { printf 'codesign: %s\n' "$1" >&2; exit 1; }

if [[ "$(uname -s)" != Darwin ]]; then
    [[ "${1:-}" != check-bundle ]] || exit 0
    [[ "${1:-}" != verify-bundles ]] || fail 'Bundle verification requires macOS.'
    exec cargo "$@"
fi

command=${1:-}
case "$command" in
    build|run|check-bundle|verify-bundles) shift ;;
    *) exec cargo "$@" ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
config="$root/src-tauri/tauri.conf.json"
config_identity=$(jq -er '. * (env.TAURI_CONFIG // "{}" | fromjson) | .bundle.macOS.signingIdentity |
    if . == null or . == "" then "-" elif type == "string" then . else error("Invalid identity") end' "$config" 2>/dev/null) \
    || fail 'Invalid bundle.macOS.signingIdentity in tauri.conf.json.'
[[ "$config_identity" == - ]] \
    || fail 'Declare APPLE_SIGNING_IDENTITY; tauri.conf.json must not pin a certificate.'
# Tauri reads this same declaration when signing the final bundle and nested code.
identity=${APPLE_SIGNING_IDENTITY--}
[[ "$identity" == - || "$identity" =~ ^[[:xdigit:]]{40}$ ]] \
    || fail 'APPLE_SIGNING_IDENTITY must be a certificate SHA-1 fingerprint or local -.'
identifier=$(jq -er '. * (env.TAURI_CONFIG // "{}" | fromjson) | .identifier | select(type == "string" and . != "")' "$config" 2>/dev/null) \
    || fail 'Missing bundle identifier.'

if [[ "$command" == verify-bundles ]]; then
    [[ "$identity" != - ]] || fail 'Release builds require an existing stable certificate fingerprint.'
    (($#)) || fail 'Supply macOS bundle output directories.'
    verify_code() {
        codesign --verify --strict --verbose=2 "$1"
        local requirement
        requirement=$(codesign -dr - "$1" 2>&1)
        printf '%s\n' "$requirement" | grep -Fqi "certificate leaf = H\"$identity\"" \
            || fail 'Bundle code does not match APPLE_SIGNING_IDENTITY.'
        printf 'SIGNATURE_VERIFIED: %s; certificate leaf matches declaration\n' "$1"
    }
    paths=$(mktemp)
    trap 'rm -f "$paths"' EXIT
    for directory in "$@"; do
        find "$directory" -type d -name '*.app' -print0 > "$paths"
        [[ -s "$paths" ]] || fail 'No macOS application bundle found.'
        while IFS= read -r -d '' bundle; do verify_code "$bundle"; done < "$paths"
        find "$directory" -type f -print0 > "$paths"
        while IFS= read -r -d '' binary; do
            kind=$(file -b "$binary")
            [[ "$kind" == *Mach-O* ]] || continue
            verify_code "$binary"
        done < "$paths"
    done
    exit 0
fi

build_args=()
app_args=()
release=false
if [[ "$command" == check-bundle && "${TAURI_ENV_DEBUG:-false}" != true ]]; then
    release=true
fi
while (($#)); do
    if [[ "$1" == -- && "$command" == run ]]; then
        shift
        app_args=("$@")
        break
    fi
    case "$1" in
        --message-format*) fail 'The signing runner owns cargo --message-format.' ;;
        --release|-r) release=true ;;
        --profile) [[ "${2:-}" == dev ]] || release=true ;;
        --profile=*) [[ "$1" == --profile=dev ]] || release=true ;;
        -*) [[ ! "$1" =~ ^-[vq]*r ]] || release=true ;;
    esac
    build_args+=("$1")
    shift
done

if [[ "$identity" == - ]]; then
    [[ "$release" == false ]] || fail 'Release builds require an existing stable certificate fingerprint.'
    printf 'codesign: local ad-hoc signing; identity changes on rebuild, releases require a stable certificate.\n' >&2
fi

[[ "$command" != check-bundle ]] || exit 0

messages=$(mktemp)
trap 'rm -f "$messages"' EXIT
# Cargo supplies the actual paths, including custom targets, profiles and target-dir.
cargo build "${build_args[@]}" --message-format=json-render-diagnostics \
    | tee "$messages" \
    | jq --unbuffered -r 'select(.reason == "compiler-message") | .message.rendered // empty' >&2

binaries=()
while IFS= read -r binary; do
    binaries+=("$binary")
done < <(jq -rs 'map(select(.reason == "compiler-artifact" and .executable != null
    and (.target.kind | index("bin")) and .profile.test == false) | .executable)
    | unique[]' "$messages")
((${#binaries[@]})) || fail 'Cargo produced no application binary.'
if [[ "$command" == run && ${#binaries[@]} != 1 ]]; then
    fail 'Select one application binary with --bin for tauri dev.'
fi

for binary in "${binaries[@]}"; do
    codesign --force --sign "$identity" --identifier "$identifier" "$binary"
    codesign --verify --strict "$binary"
done

if [[ "$command" == run ]]; then
    rm -f "$messages"
    trap - EXIT
    # A second cargo run can re-link and undo the signature. Tauri watches this PID.
    exec "${binaries[0]}" "${app_args[@]}"
fi
