#!/usr/bin/env bash
# Inspect every architecture of every Mach-O, including nested frameworks.
set -euo pipefail
(($#)) || { printf 'Supply an application bundle.\n' >&2; exit 1; }
allowed() {
    local candidate resolved
    case "$1" in
        @rpath/*)
            # Rpaths are checked separately; a suffix must not escape one.
            [[ "$1/" != *'/../'* && "$1" != *'//'* && "$1" != *'/nix/store/'* ]]
            return ;;
        @executable_path/*) candidate="$bundle/Contents/MacOS/${1#@executable_path/}" ;;
        @loader_path/*) candidate="$(dirname "$binary")/${1#@loader_path/}" ;;
        /usr/lib/*|/System/*) candidate=$1 ;;
        *) return 1 ;;
    esac
    # Resolve dot segments, repeated separators and symlinks before allowing it.
    resolved=$(python3 -I -c 'import pathlib, sys; print(pathlib.Path(sys.argv[1]).resolve())' "$candidate")
    case "$resolved" in
        /usr/lib/*|/System/*|"$bundle_real"/*) return 0 ;;
        *) return 1 ;;
    esac
}
paths=$(mktemp)
trap 'rm -f "$paths"' EXIT
count=0
for bundle in "$@"; do
    [[ -d "$bundle" ]] || { printf 'Missing bundle: %s\n' "$bundle" >&2; exit 1; }
    bundle_real=$(cd "$bundle" && pwd -P)
    find "$bundle" -type f -print0 > "$paths"
    while IFS= read -r -d '' binary; do
        kind=$(/usr/bin/file -b "$binary")
        [[ "$kind" == *Mach-O* ]] || continue
        count=$((count + 1))
        libraries=$(/usr/bin/otool -arch all -L "$binary")
        while IFS= read -r line; do
            [[ "$line" == *' (compatibility version '* ]] || continue
            library=${line%% \(compatibility version *}
            library=${library#"${library%%[![:space:]]*}"}
            allowed "$library" || { printf 'Non-portable dependency: %s -> %s\n' "$binary" "$library" >&2; exit 1; }
        done <<< "$libraries"
        commands=$(/usr/bin/otool -arch all -l "$binary")
        rpath=false
        while IFS= read -r line; do
            [[ "$line" != *'cmd LC_RPATH' ]] || rpath=true
            if [[ "$rpath" == true && "$line" == *'path '* ]]; then
                path=${line#*path }
                path=${path% \(offset *}
                allowed "$path/" || { printf 'Non-portable rpath: %s -> %s\n' "$binary" "$path" >&2; exit 1; }
                rpath=false
            fi
        done <<< "$commands"
    done < "$paths"
done
((count > 0)) || { printf 'No Mach-O files found.\n' >&2; exit 1; }
printf 'Portable macOS linkage verified: %s Mach-O files\n' "$count"
