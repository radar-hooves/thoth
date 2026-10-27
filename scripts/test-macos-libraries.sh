#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
sdk=$(env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun --sdk macosx --show-sdk-path)
xcode=$(env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcode-select -p)
export DEVELOPER_DIR="$xcode" SDKROOT="$sdk"
mkdir -p "$tmp/Thoth.app/Contents/MacOS" "$tmp/Thoth.app/Contents/Frameworks"
printf 'int main(void) { return 0; }\n' > "$tmp/main.c"
printf 'int helper(void) { return 1; }\n' > "$tmp/helper.c"
binary="$tmp/Thoth.app/Contents/MacOS/thoth"
/usr/bin/cc -fuse-ld="$xcode/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld" -isysroot "$sdk" -Wl,-headerpad_max_install_names "$tmp/main.c" -o "$binary"
bash "$root/scripts/verify-macos-libraries.sh" "$tmp/Thoth.app"
helper="$tmp/Thoth.app/Contents/Frameworks/helper.dylib"
/usr/bin/cc -fuse-ld="$xcode/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld" -isysroot "$sdk" -dynamiclib -Wl,-headerpad_max_install_names "$tmp/helper.c" -Wl,-install_name,@rpath/helper.dylib -o "$helper"
bash "$root/scripts/verify-macos-libraries.sh" "$tmp/Thoth.app"
for path in /nix/store/fixture/lib.dylib /opt/homebrew/lib/lib.dylib /Users/fixture/lib.dylib /usr/libevil/lib.dylib \
    @loader_path/../../../../../../../../nix//store/fixture/lib.dylib \
    @rpath/../../nix//store/fixture/lib.dylib /usr/lib/../../nix//store/fixture/lib.dylib; do
    /usr/bin/install_name_tool -id "$path" "$helper"
    if bash "$root/scripts/verify-macos-libraries.sh" "$tmp/Thoth.app"; then
        printf 'Incorrectly accepted %s\n' "$path" >&2; exit 1
    fi
done
/usr/bin/install_name_tool -id @loader_path/helper.dylib "$helper"
/usr/bin/install_name_tool -add_rpath /nix/store/fixture/lib "$binary"
if bash "$root/scripts/verify-macos-libraries.sh" "$tmp/Thoth.app"; then
    printf 'Incorrectly accepted external rpath\n' >&2; exit 1
fi
/usr/bin/install_name_tool -delete_rpath /nix/store/fixture/lib "$binary"
/usr/bin/install_name_tool -add_rpath @executable_path/../Frameworks "$binary"
bash "$root/scripts/verify-macos-libraries.sh" "$tmp/Thoth.app"
# An unsafe load command in the other architecture must not escape inspection.
/usr/bin/cc -fuse-ld="$xcode/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld" -arch x86_64 -isysroot "$sdk" -dynamiclib -Wl,-headerpad_max_install_names "$tmp/helper.c" -Wl,-install_name,/nix/store/fixture/lib.dylib -o "$tmp/x86.dylib"
/usr/bin/lipo -create "$helper" "$tmp/x86.dylib" -output "$tmp/universal.dylib"
cp "$tmp/universal.dylib" "$helper"
if bash "$root/scripts/verify-macos-libraries.sh" "$tmp/Thoth.app"; then
    printf 'Incorrectly accepted unsafe universal slice\n' >&2; exit 1
fi
mkdir "$tmp/empty.app"
if bash "$root/scripts/verify-macos-libraries.sh" "$tmp/empty.app"; then exit 1; fi
printf 'macOS library gate tests passed\n'
