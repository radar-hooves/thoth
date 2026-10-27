#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# shellcheck disable=SC2016 # Literal script for the fixture process.
printf '%s\n' '#!/usr/bin/env bash' 'case "$RELEASE_FIXTURE" in' \
    'draft) printf "{\"draft\":true}\\n" ;;' \
    'published) printf "{\"draft\":false}\\n" ;;' \
    'missing) printf "gh: Not Found (HTTP 404)\\n" >&2; exit 1 ;;' \
    'refused) printf "gh: Forbidden (HTTP 403)\\n" >&2; exit 1 ;;' \
    'failed) printf "network error\\n" >&2; exit 1 ;;' \
    'invalid) printf "not json\\n" ;;' 'esac' > "$tmp/gh"
chmod +x "$tmp/gh"
export PATH="$tmp:$PATH" GITHUB_REPOSITORY=fixture/thoth GITHUB_REF_TYPE=tag
GITHUB_REF_NAME="v$(jq -r .version "$root/package.json")"
export GITHUB_REF_NAME
for fixture in draft missing; do
    RELEASE_FIXTURE=$fixture bash "$root/scripts/check-release-writable.sh"
done
for fixture in published refused failed invalid; do
    if RELEASE_FIXTURE=$fixture bash "$root/scripts/check-release-writable.sh"; then
        printf 'Incorrectly accepted %s release read\n' "$fixture" >&2; exit 1
    fi
done
if GITHUB_REF_TYPE=branch RELEASE_FIXTURE=missing bash "$root/scripts/check-release-writable.sh"; then exit 1; fi
if GITHUB_REF_NAME=v0.0.0 RELEASE_FIXTURE=missing bash "$root/scripts/check-release-writable.sh"; then exit 1; fi
printf 'Release state guard tests passed\n'
