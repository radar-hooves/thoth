#!/usr/bin/env bash
# Published assets are immutable; only a missing release or draft is writable.
set -euo pipefail
: "${GITHUB_REPOSITORY:?}" "${GITHUB_REF_NAME:?}" "${GITHUB_REF_TYPE:?}"
version=$(jq -er .version "$(dirname "$0")/../package.json")
[[ "$GITHUB_REF_TYPE" == tag && "$GITHUB_REF_NAME" == "v$version" ]] \
    || { printf 'Release requires the version tag v%s.\n' "$version" >&2; exit 1; }
error=$(mktemp)
trap 'rm -f "$error"' EXIT
if release=$(gh api "repos/$GITHUB_REPOSITORY/releases/tags/$GITHUB_REF_NAME" 2>"$error"); then
    jq -e '.draft == true' <<< "$release" >/dev/null \
        || { printf 'Published release is immutable: %s\n' "$GITHUB_REF_NAME" >&2; exit 1; }
else
    # A refused or failed API read is never evidence that no release exists.
    grep -Fq '(HTTP 404)' "$error" || { printf 'Cannot read release state.\n' >&2; exit 1; }
fi
