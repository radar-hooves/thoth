#!/usr/bin/env bash
# The release's last step: refuse a draft whose latest.json would strand an
# installed copy, then publish it with the tag's CHANGELOG.md section as notes.
# Usage: publish-release.sh <tag> [file appended to the notes]
set -euo pipefail
tag=$1
extra=${2:-}
repo=${GITHUB_REPOSITORY:?}
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
gh release download "$tag" --repo "$repo" --dir "$dir" --pattern latest.json --pattern '*.sig'

# Each leg merges its platforms into latest.json in turn, so a write that lost a
# race drops a platform and a rerun leg can leave a stale signature: every signed
# artifact must be a platform of this tag, carrying that signature.
python3 -I - "$dir" "$repo" "$tag" <<'EOF'
import json, pathlib, re, sys

dir, repo, tag = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
prefix = f"https://github.com/{repo}/releases/download/{tag}/"
feed = {}
for key, platform in json.loads((dir / "latest.json").read_text())["platforms"].items():
    if not platform["url"].startswith(prefix):
        sys.exit(f"::error::latest.json {key} is not an asset of {tag}: {platform['url']}")
    feed.setdefault(platform["url"].removeprefix(prefix), set()).add(platform["signature"].strip())
signed = {s.name.removesuffix(".sig"): {s.read_text().strip()} for s in dir.glob("*.sig")}
for artifact in sorted(signed.keys() | feed.keys()):
    if feed.get(artifact) != signed.get(artifact):
        sys.exit(f"::error::latest.json on {tag} does not carry {artifact} with its .sig")

version = re.escape(tag.removeprefix("v"))
text = pathlib.Path("CHANGELOG.md").read_text()
section = re.search(rf"^## \[{version}\][^\n]*\n(.*?)(?=^## \[|\Z)", text, re.M | re.S)
if not section or not section.group(1).strip():
    sys.exit(f"::error::CHANGELOG.md has no section for {tag}")
(dir / "notes.md").write_text(section.group(1).strip() + "\n")
EOF
[[ -z "$extra" ]] || cat "$extra" >> "$dir/notes.md"

# A rerun of an older tag publishes without taking `latest`, which the updater's
# endpoint follows.
latest=$(gh release view --repo "$repo" --json tagName --jq .tagName 2>/dev/null || true)
flag=--latest
[[ -z "$latest" || "$(printf '%s\n%s\n' "$latest" "$tag" | sort -V | tail -n1)" == "$tag" ]] || flag=--latest=false
gh release edit "$tag" --repo "$repo" --notes-file "$dir/notes.md" --draft=false "$flag"
