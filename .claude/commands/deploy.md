# Deploy Thoth

Build, install, and launch Thoth locally. Permissions carry over: the build is signed with the same certificate as releases.

## Steps

1. **Quit running instance** - Gracefully quit Thoth if it's running:

   ```bash
   osascript -e 'tell application "Thoth" to quit' 2>/dev/null; sleep 1
   ```

2. **Build production binary** - Run the Tauri build:

   ```bash
   pnpm tauri build
   ```

   This produces `src-tauri/target/release/bundle/macos/Thoth.app`

3. **Install to /Applications** - Sync the built bundle into place (rsync --delete replaces all files cleanly, unlike cp -R which merges into existing .app bundles):

   ```bash
   rsync -a --delete src-tauri/target/release/bundle/macos/Thoth.app/ /Applications/Thoth.app/
   ```

4. **Clear quarantine** - Remove macOS quarantine extended attributes:

   ```bash
   xattr -cr /Applications/Thoth.app
   ```

5. **Launch** - Open the freshly installed app:

   ```bash
   open /Applications/Thoth.app
   ```

6. **Report** - Confirm the app launched and that `get_permissions` still reads both granted.

## Notes

- Bundle identifier: `com.poodle64.thoth`
- macOS keys TCC grants to the designated requirement (bundle id plus certificate leaf); a build signed with the stable certificate keeps every grant, an ad-hoc one does not
- The quarantine attribute (`com.apple.quarantine`) triggers Gatekeeper warnings on unsigned builds
- If the build fails, check `pnpm install` was run and Rust toolchain is current
