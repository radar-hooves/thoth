# Thoth

Privacy-first, offline-capable voice-to-text for macOS and Linux: record on a hotkey, transcribe locally, paste at the cursor — no cloud round-trip on the core path.

## Scope

- Does: capture audio from any input device, transcribe it locally and offline, optionally enhance the result with a local Ollama model, and paste it at the cursor in whatever app has focus.
- Does not: sync anything to the cloud, change the system's default audio device, or require network access for the transcription path itself.
- Paste-at-cursor must restore the user's prior clipboard contents afterward — never leave the transcription sitting in the clipboard. The sole exception is a _failed_ insertion: the restore is skipped so the transcription survives on the clipboard for a manual paste.

## Running it

- `direnv allow` once per checkout — `.envrc` provisions the pinned Rust/Node toolchain from `flake.nix`; skip it and `cargo`/`pnpm` fall back to whatever's on `PATH`.
- Rust commands (`cargo test`, `cargo clippy`) run from `src-tauri/`, not the repo root.

## Single source of truth

A version, default value, or capability flag must have exactly one definition. If another language or build system needs it, **derive it** — read the JSON, call a command, generate the constant. Do not retype it.

If a value genuinely must be duplicated, add an assertion that the copies match, and add the new location to the script that maintains it in the same commit.

`flake.nix`'s `pnpmDeps.hash` cannot be derived: `scripts/update-pnpm-deps-hash.sh` regenerates it, and CI runs it and pushes the correction to the PR branch. Never "verify" it with a plain `nix build`: a fixed-output derivation is addressed by hash + name, so a warm store skips the fetch and reports success against a stale hash.

Version bumps: `scripts/bump-version.sh`, the authority on which files carry a version.

## Governance Exceptions (Tauri Desktop App)

Thoth's divergences from `canonical-app-shape.md` live in `.canonical-exceptions`.

### The app shell is deliberately not adopted

`@poodle64/ui` ships `AppShell`, and Thoth's settings window does not use it. This is an argued deviation, not drift: the settings window is **frameless**, and its 54px title bar is the window's chrome — `app-region: drag`, the live recording/processing status, and, on Linux with decorations disabled, the close/minimise controls. `AppShell`'s `<header>` has no drag-region concept, so hosting one means the compatibility shim `canonical-app-shape.md` forbids. The deviation covers the **shell only**.

## Pitfalls

- Linux has no Metal path: GPU acceleration is a choice of mutually exclusive Cargo features (`vulkan`/`cuda`/`hipblas`), each requiring `--no-default-features` — read `docs/development/linux-setup.md` before touching the Linux build.
- `pipeline.rs`, `audio/capture.rs` and `clipboard.rs` are timing-sensitive — even a 50ms sleep or an extra async step is perceptible on the recording toggle. Test rapid toggle-on/toggle-off before committing a change to any of them.
- Reinstalling a rebuilt `Thoth.app` changes its ad-hoc signature and resets macOS TCC (mic/accessibility), so the app hangs at the mic-permission dialogue before the MCP server binds: reinstall with `.claude/commands/deploy.md`, then have the operator grant Allow.
