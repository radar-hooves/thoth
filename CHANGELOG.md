# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added

- **Your word lists can now sync with a file on a WebDAV server, so other tools share the same corrections.** An optional toggle in Settings → Integrations (off by default) keeps the dictionary and the canonical terms in step with one shared JSON file — edits in Thoth reach the file within seconds, edits in the file reach Thoth within a minute, and when the same entry changed on both sides, Thoth's version wins. The format is documented in [docs/word-list-sync.md](docs/word-list-sync.md) for second consumers.
- **An assistant can now search your transcription history through Thoth's MCP server, not just page through the latest 100.** The `transcription` tool's `list` takes a `query`, matching what the History search box does, and a `limit`.

### Fixed

- **"Reset to defaults" in Output Filtering leaves Australian spelling on.** The button now resets to the defaults the app itself holds rather than a copy the window carried, which said Australian spelling was off when it defaults on — so resetting the filters no longer silently turns it off, and the button no longer shows when nothing has changed.

## [2026.10.1] - 2026-10-08

### Fixed

- **Ampersand terms now come out one way every time, and `<unk>` never reaches your text.** The Parakeet models cannot write `&`, so "S&P" came out as "S<unk>P", "S and P" or "S P", and "P&L" as "P and L" or "Pn L". Any two capital letters you join with "and" now read as one term (S&P, P&L, R&D, M&A, Q&A, V&V), and a stray `<unk>` is dropped. "Both A and B", "X and Y" and "you and I" stay as you said them. A term the model writes as two bare letters ("S P") carries no trace of the "and", so add a dictionary entry for it (`S P` → `S&P`).

## [2026.10.0] - 2026-10-05

### Changed

- **Thoth's telemetry now runs on the household's shared telemetry plugin, and every command still reports its timing.** If you send telemetry to your own collector, the traces, the failure events and the memory/CPU samples from the last release carry on as before, now including the 85 commands that had briefly dropped out of the trace. The telemetry fields in Settings → Integrations work the same way.
- **Thoth is built against the factory's latest shared kit** (app-factory v2026.9.34), so it picks up that kit's fixes.

### Fixed

- **The Linux build no longer needs a private repository to compile.** The telemetry dependency now comes from the public source, so building Thoth from a fresh checkout works without special access.

## [2026.9.6] - 2026-09-28

### Added

- **Thoth's telemetry now covers what actually goes wrong, not just the lifecycle events around it.** If you've pointed Thoth at your own collector, every failure a dictation can actually have — a transcription that errored, a model that wouldn't load, a microphone that dropped out, a paste or type that didn't land, an AI enhancement that was skipped or discarded, a config that failed to load or save, or the local Control API/MCP server refusing to start — now reports as its own event, carrying the real error message and its full cause chain rather than a bare category label. The update check reports too, since it's the one network call the webview itself makes.
- **Every one of Thoth's internal commands now shows up in your trace collector by name, with how long it took and whether it succeeded** — including every Settings pane, so a slow pane switch is something you can go look at rather than guess about. No argument ever leaves with it: what a command was called with stays off the wire by construction.
- **Thoth now samples its own memory and CPU use** — every minute, right after a transcription model finishes loading, and right after each dictation — so "is this thing a memory hog" is a question your own collector can answer from real use.
- Enhancement events now name which prompt ran (never its text), transcription events carry a word count alongside the existing character count, recording-started now names the input device, and startup now records the behaviour-shaping settings (recording mode, auto-paste, enhancement backend, filters) it loaded.

## [2026.9.5] - 2026-09-28

### Added

- **A skipped or discarded AI enhancement now tells you, instead of quietly pasting the plain transcript.** Both cases — a dictation over the length cap, or a reply that came back an implausible length — now show a brief toast saying so, alongside the same original text you'd have gotten anyway.

### Fixed

- **Thoth no longer crashes at launch on Linux when the system tray library is missing.** On a system without `libayatana-appindicator3` or `libappindicator3` — not bundled by the AppImage or the raw binary, only pulled in automatically by the `.deb` — the tray icon setup used to bring down the whole app before recording, shortcuts or transcription ever started. A missing tray icon is now cosmetic: Thoth logs it and keeps running without one.
- **A long dictation's AI enhancement no longer comes back gutted, or arrives after you've moved on.** A dictation past about 6,000 characters could be handed to your local model with no context-window size set — Ollama then silently drops the _start_ of a prompt that overruns it, so the model corrected a fragment and handed back a fraction of what you said. Enhancement above that length is now skipped outright (your filtered transcript still pastes, instantly), and for everything under it the context window is sized to fit the whole thing and generation is capped so a model that loops cannot run for minutes. A reply that comes back an implausible length for what you asked for is discarded in favour of your original text rather than pasted — "implausible" now depends on the prompt: Summarise is expected to come back much shorter and Expand much longer, so only a genuinely broken reply in either direction is rejected. The old three-attempt retry is gone too — one request, generously timed to allow for a model that has to load first, so a failure is known and handled once rather than after three attempts.
- **A model your build cannot run is no longer loaded, even though the Active badge never claimed it.** A config carried between machines, or just gone stale, could point Settings' model list at one (correctly unavailable) model while the actual startup and manual-selection paths tried to load a different, resolved one — so what warmup attempted and what the UI showed could disagree. Both now resolve the same way.
- **A config reload that fails no longer clears the way to overwrite your real settings with defaults.** A transient failure refreshing configuration replaced it with placeholder defaults in memory while still allowing a save — so the next change you made in Settings could write those defaults over your actual, working configuration. A failed refresh now keeps what was last loaded; defaults are used only before anything has loaded at all, and saving stays disabled until that first load succeeds.
- **The bundled MCP server's dependency no longer carries two publicly known high-severity flaws.** Upgraded to the fixed release; nothing about the ten MCP tools it serves changed, and the server was never reachable outside your machine either way.

### Changed

- **Right Shift is the default record key on macOS.** A fresh install used F13, a key almost no keyboard has. A bare tap of right Shift is on every keyboard and clashes with nothing, so it is now the default there; F13 stays the default on Linux and Windows, where a modifier-only key cannot be read on Wayland. An existing configuration is untouched. On macOS the modifier key needs the Input Monitoring permission the setup guide already asks for; the Cmd+Shift+Space alternate works without it.

## [2026.9.2] - 2026-09-07

### Added

- **Telemetry now has a section in Settings, so any Mac can point Thoth at its own collector.** Where Thoth's own operational events and traces go was decided entirely by environment variables — and an app launched from the Dock inherits none of them, so on a machine nobody had specially configured there was no way to turn it on at all. Integrations now carries a Telemetry card: an endpoint (your collector's OTLP/HTTP base — Thoth appends `/v1/logs` and `/v1/traces`), and an authorisation helper, which is a command that prints the headers rather than a token you paste, so the credential itself never lands in Thoth's settings. A Test button sends one record and tells you whether the collector took it, before you save. Saving applies it straight away, with no restart. Where the environment already sets the endpoint it still wins: the fields show what it set, greyed out, and say so. The privacy boundary is unchanged — only events explicitly marked for export can leave, and transcript text cannot.

### Fixed

- **Telemetry no longer loses its last events when you quit.** Events and traces are batched in the background rather than sent one at a time, so a handful are always still in flight; Thoth was exiting without flushing them, which meant the end of every session — including whatever went wrong just before you quit — never arrived. Quitting now flushes what is queued before the process goes, within a bounded budget so it cannot delay shutdown. This only affects machines that export telemetry at all; nothing changes where it is off.
- **A wrong or unreachable AI-enhancement address now fails in about ten seconds instead of hanging.** Requests to your Ollama or OpenAI-compatible endpoint had no connect deadline of their own, so an address that silently swallows the connection — a mistyped LAN IP, a machine that is off — held the enhancement step for the full request timeout before giving up. Connecting now has its own ten-second limit, so you get the error while the transcription is still fresh rather than a minute later. A server that accepts the connection and takes its time to answer is unaffected.
- **Running the test suite from a source checkout no longer wipes your settings.** One test handed the config migration a synthetic configuration, and the migration saved its result — over `~/.thoth/config.json`. Every `cargo test` therefore reset the selected input device, the transcription model and the keyboard shortcuts to their defaults, without a word. Migration no longer writes; loading from disk does. The released app was never affected, because it does not run tests.

## [2026.9.1] - 2026-09-07

### Removed

- **The Logging & Telemetry settings panel, and everything it configured.** Thoth no longer asks you for a Loki URL, a bearer token, a tenant, extra labels or a retention window, and there is no "forward telemetry" toggle or "Test connection" button. Where telemetry goes is no longer an app setting (see Changed below), so there was nothing left for the panel to set.
- **Local log files, and the `~/.thoth/logs/` folder.** Thoth wrote a rotating daily log file to your disk and pruned it on a retention window. It writes no file now. Diagnostic output goes to the app's standard error, where it can be read live, so nothing accumulates on your machine and no folder of transcript-adjacent text sits there waiting to be found. Any files already in `~/.thoth/logs/` are yours to keep or delete; nothing writes to them.
- **The Storage pane's Logs section.** With no log files there is nothing to measure or clear, so the Logs row, its share of the usage bar and its Delete button are gone, along with the `delete_all_logs` command behind them. Models, Recordings, Database and Config are unchanged.
- **The `test_loki_connection` MCP tool.** It pushed one synthetic event to verify a Loki URL, token and tenant before you saved them. There is nothing left to save, so nothing left to test. Every other MCP tool is unchanged.

### Changed

- **Telemetry is now the environment's decision, not a setting — and off unless the environment sets it.** Thoth reads the standard OpenTelemetry variables (`OTEL_EXPORTER_OTLP_ENDPOINT` and its siblings) at startup: set them, and content-free operational events and traces are pushed over OTLP to that endpoint; leave them unset, as they are on any machine that has not deliberately configured them, and Thoth writes to standard error and sends nothing anywhere. The privacy boundary is unchanged and still structural: only events and traces explicitly marked for export can leave, so timings, model names, byte counts and error reasons can, and transcript text cannot. Outbound requests to your own AI-enhancement endpoint now carry a trace header, and what is recorded about them is the method, host, port and status code — never the URL, the path, the query or the error text.

## [2026.9.0] - 2026-09-04

### Fixed

- **The MCP dictionary tool now tells a caller the shape it wants, and its errors name what went wrong.** `import` accepted only the `{"entries": [...]}` wrapper that `export` returns, and nothing said so — the tool description called it "a JSON string of dictionary entries" — while a bare array was refused with `invalid type: map, expected a sequence`, which describes the opposite of what happened. An agent lost two round-trips to it. Import now takes a bare array as well as the wrapper, a wrong shape is answered with the shape it wanted, and the docstrings state the export/import symmetry with a literal example. The same defect class was swept across the other dispatchers: an unknown action lists the valid ones, a missing transcription echoes the id it looked up, and the `canonical` docstrings now say that `update` REPLACES a whole term rather than merging into it — omitting `aliases` clears them and omitting `policy` resets it.

- **Thoth no longer claims a model is loaded when it cannot actually transcribe.** The Neural Engine backend writes a small marker file the first time it initialises, but the models it really loads live in a separate CoreML cache — so once that cache was cleared, the marker lingered and every part of the app believed a working model was installed. The menu bar could read "No Model" while Settings showed the same model as "Active", recording started anyway, and the app then sat on "Processing..." for a full minute before giving up. Readiness now follows the CoreML cache itself, recording is blocked up front with a clear message when no model can actually load, and a model that is selected but not installed is labelled "Not installed" instead of "Active".

- **The bundled MCP server can now change or remove a dictionary entry by its own text, not just by its position.** `update` and `delete` took a zero-based `index` only, and `list` did not report one — so on a three-hundred-entry dictionary there was no way to act on a known entry except by counting the list by eye, and a count that landed one row off silently rewrote or deleted a different, working entry. Both actions now also accept `from`, the entry's own find-text: it is matched case-insensitively against the whole value, and if it matches more than one entry the call is refused (naming the candidates) rather than picking one. `list` now reports each entry's `index` alongside it, and passing `index` works exactly as before.

- **Recording start/stop sounds now play on Linux.** The sound module only ever had a macOS backend, so the "play sounds" setting was visible on Linux but silently did nothing. The cues are now synthesised in-process and played through the default output device — a rising two-tone for start and the same interval falling for stop, mirroring the macOS dictation on/off pair. Nothing needs to be installed: no sound-theme package, no `paplay`/`canberra` helper, no bundled audio file, so the cues behave identically on every distribution and desktop, including minimal window-manager setups.
- **A failed auto-paste on macOS no longer fails silently, and no longer loses the transcription.** Pasting synthesises Cmd+V through Core Graphics, which macOS silently discards when the app is not Accessibility-trusted — and Thoth resets that permission after every update, so an updated-but-not-yet-re-granted app pasted nothing while reporting success. Worse, the clipboard was then restored to its previous contents a second later, so the transcription was gone too. Thoth now checks the Accessibility permission before pasting and tells you (with a toast and the exact System Settings path) when it is missing, and a failed insertion skips the clipboard restore so the transcription stays on the clipboard and can be pasted manually with Cmd+V.
- **The bundled MCP server no longer echoes the whole dictionary/canonical list on every edit.** Adding, updating or deleting a dictionary entry — or a canonical term — returned the entire list (~150 entries) in the tool response each time, spending the agent's context on data it never asked for. These actions now return a compact acknowledgement (`{ok, action, index, count}`); use the `list` action when you actually want the full list back.

## [2026.6.7] - 2026-06-25

### Added

- **The bundled MCP server can now test the Loki connection and control recording.** Two new tools: `test_loki_connection` verifies your Loki URL, token and tenant by pushing one content-free event (the same as the Settings "Test connection" button), and `recording` starts, stops, or toggles recording exactly as the global hotkey does — honouring your saved filter, spelling and enhancement settings.

### Fixed

- **Thoth no longer crashes on dictations with accented or non-Latin characters.** A word like "café" or "sí" reaching the canonical-term phonetic matcher could panic it (the matcher sliced text by byte, cutting a multi-byte character in half); and because release builds aborted on any panic, the whole app went down, which felt random. Accented input is now folded to its ASCII form before phonetic matching. As defence in depth, the transcription pipeline now contains panics, so a single bad input fails only that one transcription (with an error toast and sound) instead of aborting the app.
- **Live telemetry forwarding to Loki now works.** A saved Loki URL that already ended in `/loki/api/v1/push` was doubled (the app appended the push path again), so every forwarded event silently returned 404. You can now save just the base URL (e.g. `https://loki.example`) and Thoth adds the push path itself; a full-path URL is also handled. The historical-log backfill script now reads the endpoint and token from Thoth's own settings, so no separate env file is needed.
- **Your Loki URL and token are no longer wiped when you update the app.** On launch the app re-saved the whole config just to record the version, and that round-trip could blank the saved Loki URL (and exposed the token to the same fragile path), so after an update remote forwarding silently stopped. The version is now recorded without rewriting the rest of the config, the URL and tenant get the same protection the token already had, and saving with forwarding enabled but no URL is now blocked (an empty token warns) instead of silently saving a broken configuration.

### Note

First published build since 2026.6.5. It also includes everything from the never-published 2026.6.6: the Insights dashboard, the opt-in content-free Loki telemetry export, and Linux / Wayland support (detailed in the [2026.6.6] entry below).

## [2026.6.6] - 2026-06-24

### Added

- **Insights dashboard.** A new Insights pane visualises your dictation history — total words and recordings, time saved versus typing, an activity heatmap, transcription-speed and model-usage breakdowns, a recording-length histogram, a time-of-day chart, and a storage breakdown — plus a "cruft finder" that surfaces stray or short recordings for clean-up. (#90, #91)
- **Reversible Trash for recordings.** The cruft finder no longer deletes outright; recordings are moved to a recoverable Trash you can restore from, so clean-up is safe. (#91)
- **Opt-in, content-free telemetry export to Loki.** A new Logging & Telemetry settings card lets you forward operational events (recording lifecycle, model-load, timings, character counts — never transcript text) to a Loki endpoint you control, with a Test-connection button. Off by default; the auth token is stored locally and redacted in the UI. (#91)
- **Linux / Wayland support.** Native Hyprland global shortcuts (via `hyprctl` + a FIFO) and clipboard paste (`wl-copy` + `hyprctl`); an optional GPU (CUDA) Parakeet backend via the sherpa-onnx CUDA prebuilt; a window-decorations toggle with a custom close button; and an importable Nix package (GPU Parakeet + Whisper Vulkan).

### Changed

- **The main window is larger** (1100px wide, with a minimum size) to fit the Insights dashboard. (#91)
- **Developer builds now use the Nix flake toolchain** (micromamba retired). On macOS the Nix dev shell now builds the FluidAudio/Parakeet backend and runs the full Rust test suite for the first time, via a scoped Swift-toolchain fix for the leaked Nix-SDK-vs-Xcode mismatch. (#93)

### Fixed

- **Long dictations no longer get spurious sentence breaks at pauses.** On recordings long enough to be split internally (~14s+), the transcriber treated each split point as a sentence end — inserting a stray full stop and a capital letter mid-sentence (e.g. "take a look at this. Recording and see…"). Thoth now measures the pause length at each split: a brief in-sentence breath is stitched back together (stray full stop and capital removed), while a genuine between-sentence pause is preserved. (#96)
- **Sentence-initial filler words are cleaned up correctly** ("Um, anyway…" → "Anyway…").
- **Settings handling is more robust** — configuration patches tolerate camelCase/snake_case and apply as partial updates, and the transcription-init IPC argument casing was corrected. (#94, #95)
- **Smaller fixes:** the Insights time-of-day axis labels align with the hour bars; the bundled MCP server emits compact JSON; transcription readiness is polled so the status leaves "Loading…".

## [2026.6.5] - 2026-06-13

### Fixed

- **The MCP / Control-API bearer token no longer changes on every install or restart.** The token was only ever held in `config.json`, which is rewritten at startup, so in practice it was regenerated each launch and never durably saved — forcing you to re-point MCP clients at a new token each time. The token now lives in its own dedicated file (`~/.thoth/control_api_token`, owner-only) that survives reinstalls, updates and settings resets; it is generated once and never changes unless you explicitly rotate it from the Integrations pane. One-time note: because no token was previously saved to disk, this update sets a fresh token once — update your MCP client to it (Integrations pane) after installing, after which it is permanent.
- **The Overview pane (and general UI navigation) no longer lags.** A permission-status poll ran twice a second indefinitely whenever the optional Input Monitoring permission wasn't granted — which is the common case, and the case after every update (updates reset macOS permissions) — firing native permission checks continuously and janking the UI. The poll now stops once the required permissions (microphone, accessibility) are granted, treats Input Monitoring as optional, and is hard-capped so it can never run away.

## [2026.6.4] - 2026-06-13

### Changed

- **Dictated text is no longer chopped into paragraphs at arbitrary points.** Thoth used to force a blank-line paragraph break into the text it typed out roughly every 50 words, landing on whatever full stop happened to be nearest — so a long dictation came out split mid-thought, with breaks that had nothing to do with what you were saying. That word-counter behaviour is gone. Your transcription is already punctuated and capitalised by the transcriber, so it is now inserted as the continuous text you actually spoke. (The breaks were only ever added to the typed-out text, never to your saved history, which is why they appeared "as it printed out".)

### Added

- **Voice formatting commands.** Say "new paragraph" to start a new paragraph (a blank line) or "new line" for a single line break, while dictating — the same convention macOS Dictation, Dragon and Talon use. Commands are recognised only when spoken as a standalone instruction between sentences, so ordinary phrases like "a new line of code" are left untouched. On by default; toggle it under Transcribe → Output Filtering → "Voice formatting commands". The breaks now apply to both the inserted text and your saved history, so the two match.

## [2026.6.3] - 2026-06-08

### Fixed

- **Bluetooth headphone audio no longer drops to call quality when you start recording.** With no other microphone connected, macOS makes a Bluetooth headset the default input, and opening its mic forced the headset out of high-quality music mode (A2DP) into low-quality call mode (HFP) for the duration of the recording — your music would pause and come back distorted, then recover when recording stopped. The previous guard tried to record from the built-in mic instead but located it by an exact name match that failed on a headphones-only setup, so it fell back to the headset anyway. Thoth now records from the first non-Bluetooth input it finds (built-in or USB), leaving your headphones in A2DP so music keeps playing. If a Bluetooth headset is genuinely the only microphone available, it is used and you are told the audio will briefly drop to call quality.
- **The `transcription` MCP tool's `list` action returns your history instead of an empty list.** It always returned `[]` even with thousands of records, because it called a fetch-by-IDs path with an empty ID list (which means "fetch none", not "fetch all"). It now reads the 100 most recent records, newest first, from the same database path the `get` and `stats` actions use.
- **The "Update available" notification now renders correctly in dark mode.** The auto-updater's toast appeared in light colours (light background, near-invisible "Cancel" button) even with the app in dark mode, because the toast library's coloured-notification styles ignored the active theme. All coloured notifications (success, info, warning, error) now use their proper dark-mode colours.

## [2026.6.2] - 2026-06-07

### Added

- **Smarter correction for project and tool names.** The personal dictionary needed a separate entry for every way the transcriber mangled a word — "portcullis" alone had six. You can now register a term once and have its acoustic and spelling variants snap to it automatically, with a per-term safety setting: coined names you never actually say as ordinary words (portcullis, LiteLLM, Vaultwarden) snap aggressively, while names that collide with real words (for example a product called "immich" versus the word "image") stay conservative so genuine words are never over-corrected. Your existing dictionary is folded in on first run — one entry per term with all its variants attached — and nothing changes in behaviour until you opt a term into the smarter matching. Managed through the new `canonical` MCP tool. (Genuine decoder-level biasing was investigated and is not currently viable on either transcription backend, so this deterministic approach is the solution.)
- **Global shortcuts now work on Wayland** via the XDG Desktop Portal (`GlobalShortcuts`), which KDE, wlroots-based compositors, and GNOME 48+ implement. Previously the Tauri global-shortcut plugin was used on every Linux session, but it only works under X11, so on a Wayland session the recording hotkey silently did nothing with no explanation. On a compositor without the portal, Thoth now tells you (a notification) that global shortcuts are unavailable and to use a function-key shortcut or an X11 session. The portal assigns the actual key (the app can only request a preferred one), and the assigned binding is reported back to the UI.
- A Linux/macOS CI workflow (build, `rustfmt`, `clippy -D warnings`, tests, `.desktop` validation, frontend type-check) now runs on every pull request. Previously only the release workflow compiled the code, so Linux-only breakage was invisible until a release was cut. The Linux job builds with the same Vulkan feature set the release ships, so the Linux-only code is compiled on every change.
- A user notice when the configured microphone is unavailable and recording falls back to the system default device (e.g. an unplugged USB mic), instead of silently switching.
- On Linux/Wayland without `wtype`, a one-time notice explaining that installing it makes text insertion seamless (otherwise GNOME prompts for "Allow Remote Interaction" each session).
- Contributor guide for building on Linux ([docs/development/linux-setup.md](docs/development/linux-setup.md)), covering build dependencies, runtime packages, the AppImage GPU caveat, and display-server behaviour.

### Changed

- **Parakeet fallback backend moved to the official sherpa-onnx crate.** The third-party `sherpa-rs` binding it relied on is now deprecated upstream; migrated to the official k2-fsa `sherpa-onnx` 1.13 crate, which also bundles a newer native engine (1.13.2 vs 1.12.9). No user-facing change — the Apple Neural Engine (FluidAudio) path remains the default; this keeps the fallback engine current and off a dead dependency.
- **Dependencies brought fully up to date**, including major upgrades across the database (rusqlite), HTTP (reqwest, now on the Rustls TLS stack), audio-resampler (rubato 3.0) and decoder (symphonia 0.6) layers, the whisper.cpp bindings, the Wayland portal client, and the audio-capture crate (cpal 0.18 — the last remaining dependency on an old major), moving the project to the Rust 2024 edition. The end-of-recording resampler drain that preserves your final words was re-implemented for the new API and re-verified end to end.
- **Kept and hardened the warm-microphone optimisation.** Evaluated whether the warm-stream lifecycle earns its complexity and measured a ~390 ms cold device-open on the DJI MIC MINI — very noticeable on every record press — so it stays. Locked in the guard that prevents an idle teardown from closing the microphone stream mid-recording.
- The recording indicator degrades cleanly on Wayland: it no longer tries to follow the cursor (Wayland does not expose the global cursor position) and uses a fixed on-screen position instead.
- Modifier-only shortcuts (e.g. double-tap Right Shift) are now correctly refused on Wayland from the runtime re-registration path as well as at startup.
- The Linux `.deb` now depends on `libvulkan1` (needed by the Vulkan GPU build at runtime) and recommends `wtype`, `xdg-utils`, and the AppIndicator runtime.
- Whisper initialisation logs the actual compiled GPU backend (Metal/CUDA/ROCm/Vulkan/CPU) rather than always claiming "Metal GPU"; the CPU-only Linux build now tells you how to enable GPU acceleration.
- **Build, CI and dependency hardening.** The Rust lockfile is now committed so builds are reproducible (it was floating, which silently broke CI); the HTTP client's TLS crypto provider was switched from aws-lc to ring while staying on Rustls, so the macOS build compiles cleanly; the Linux Wayland global-shortcuts code was updated to the current XDG portal library; frontend and Rust dependencies were brought to their latest patch/minor versions; and the CI workflows were modernised. Continuous integration is green across macOS and Linux again.
- **Internal error handling is now typed end to end.** The ~100 Rust commands the interface calls used to return plain error strings (a Tauri v2 anti-pattern that loses error structure); they now return a single typed error enum that serialises to the same string. The error messages you see are unchanged — including the "no speech detected" and "Input Monitoring" cases the UI keys off — but the backend builds them through one consistent path.
- **Parakeet now links statically on Linux** (the old `sherpa-rs` no-static-archive blocker is gone with the move to `sherpa-onnx`). It is not yet runtime-verified on a Linux desktop — a headless smoke test currently faults at model load under the Nix toolchain — so it stays opt-in on Linux (`--features parakeet`); the default Linux transcription backend remains Whisper.
- **Linux now uses its real platform probes instead of stubs.** Microphone availability, permission, and text-caret detection on Linux were hardcoded stubs (always reporting "granted"/unavailable) even though working PulseAudio/PipeWire and X11 implementations already existed but were never called. The platform layer now dispatches to them, so Linux gets genuine microphone-availability detection rather than a constant "granted".
- **Less OS-specific code.** Incidental per-platform branches were collapsed behind cross-platform abstractions: one shared Wayland-session detector replaces three copies, external links open through the cross-platform `open` crate instead of hand-rolled `open`/`xdg-open`/`cmd` branches, and the duplicated compile-time GPU-backend `cfg` ladders used for log and label messages now resolve through a single helper. No behaviour change; the genuinely OS-specific code (macOS permissions and CoreAudio, the Wayland portal, X11 cursor tracking, the FluidAudio bridge) is untouched.

### Fixed

- **"folder" (and words like "filter") is no longer auto-corrected to "Vaultwarden" (#74).** The smarter term-matching above could snap a spoken word to a registered name on a phonetic match alone, and the phonetic code it uses is coarse enough that several common words ("folder", "filter", "falter") collide with "Vaultwarden" — so they were being silently rewritten. Matching now requires genuine spelling similarity in addition to the phonetic match, so unrelated everyday words are left alone while real mishearings of a registered term still snap.
- **Deleting a recording from history now also deletes its audio file (#75).** Removing a transcription left its WAV on disk, so the recordings folder grew without bound (hundreds of orphaned files had accumulated). Deletion now removes the audio too — keeping a file only while another history entry still references it — and a new reconcile action sweeps up orphans left by earlier versions.
- **A silent recording is discarded instead of saved (#76).** Pressing record, saying nothing, and pressing stop used to leave an orphaned audio file on disk; the silent recording is now removed and treated as a no-op rather than an error. A silent imported or re-transcribed clip shows a neutral "No speech detected" notice instead of an error.
- **The "Reset System Permissions" dialogue now closes after you confirm.** It was staying open through the admin-password prompt and looked like it had reappeared (the reset itself was working). It's now dismissed as soon as you confirm.
- **Your AI-enhancement API key no longer gets wiped when you open Settings after an app update.** The key is stored in ~/.thoth/config.json and always survived reinstalls, but it was the one setting with no "don't overwrite with a blank" protection, so opening Settings after an update could save an empty value over it. It's now guarded like every other sensitive setting, and changing or clearing the key goes through a dedicated path.
- **transcribe_file now accepts the audio formats it advertises.** Handing it an MP3, M4A, OGG, or FLAC (for example a phone voice memo) previously failed immediately with a misleading "Not a valid WAV file"; these are now transcoded automatically and transcribed. A genuinely unsupported or corrupt file now reports an accurate error instead of the generic WAV message.
- **Tail truncation on long recordings is fixed for good (#46).** Long dictations could lose their final words, and short ones could gain a phantom word at the end (a stray "Okay" you never said) — these turned out to be the same problem. The Parakeet (FluidAudio) engine transcribes audio longer than ~15 seconds by splitting it into ~15-second pieces internally, and the bundled engine decoded that final piece unreliably: depending on where its boundary fell it either dropped the closing words or invented a filler word on the trailing silence. Adjusting the silence padding only changed which recording lengths happened to land badly, which is why the problem kept coming back. Thoth now does the splitting itself — keeping every piece within the size the engine decodes reliably in a single pass, cutting only at natural pauses so no word is ever split, and joining the results. Short recordings are unchanged; long ones are split at silences.
- The last fraction of a second of every recording (and of imported audio files) is no longer lost. The resampler held a short internal delay that was never drained at end of stream, so the true tail never reached the file; it is now flushed properly on finalise.
- Long recordings no longer occasionally lose the back half of the transcription (#46). Audio was captured into a small fixed buffer (about 0.7 seconds at a typical microphone's rate) that was resampled in place by the same thread; whenever that thread briefly fell behind — for example while a previous transcription was still running on the GPU — the buffer filled and silently discarded incoming audio. Capture is now fully decoupled from resampling: the microphone callback hands raw samples to an unbounded queue and a separate thread resamples and writes the file at its own pace, so a slow moment can only delay the file, never shorten it.
- The FluidAudio model-cache path (a macOS `~/Library/...` location) is no longer constructed on Linux, where it would have produced a bogus path; storage accounting treats it as not applicable off macOS.
- Tray icon theme detection now has a KDE/Plasma fallback (reads `kdeglobals`), so the icon matches dark themes on KDE, not just GNOME.
- **The Nix flake now builds the Linux Vulkan GPU backend (#64).** Building on NixOS via `nix develop` could not produce the GPU build: the flake's pinned Rust toolchain predated a dependency in the committed lockfile, and — separately — `bindgen` (which generates the whisper.cpp bindings) runs outside Nix's compiler wrapper and so could not find the C standard headers, so the binding crate silently fell back to its non-GPU bindings and failed to compile the Vulkan module. The flake now ships a current toolchain and points `bindgen` at the right header paths, so `nix develop` builds and links the Vulkan backend. A standard apt-based build, as CI uses, was unaffected.

## [2026.6.1] - 2026-06-01

### Added

- The Local Control API and bundled MCP server now default to **on**. They bind `127.0.0.1` only and require the bearer token (auto-generated on first run), so they are not network-exposed; MCP-capable assistants work out of the box. Enabling MCP also starts the Control API automatically — previously toggling MCP on while the API was off silently did nothing, which is why it took several restarts to come up. Toggles now take effect live without an app restart, and a failure to bind the port (e.g. already in use) surfaces an error instead of failing silently.

### Changed

- After an update, Thoth now resets its macOS permissions (microphone, accessibility, input monitoring) once so they can be re-granted from a clean slate. macOS ties permission grants to the app's code signature, which changes on each build, so an update silently invalidated the previous grants and left recording/shortcuts broken until manually reset. The reset fires only when the version actually changes, never on a fresh install or a normal relaunch.
- Australian-spelling conversion is rebuilt on the canonical VARCON / English Speller Database word map (the same data behind the en_AU dictionary in browsers and office suites), replacing a hand-maintained word list, and now defaults on. The whole `-ise` family now converts (realise, institutionalise, modernise, hospitalise — not just the words someone happened to list), alongside `-our`, `-re`, `-ence`, `-ogue` and irregular forms, while false friends (size, capsize, seize, prize) and homograph hazards (tire, curb, story, practice) are left untouched. ~3000 verified pairs.
- Spoken-number conversion (words → digits) now defaults **off**. Rule-based conversion of dictated numbers is inherently ambiguous — a lone "one" may be a pronoun ("a new one"), and a counted sequence ("six seven eight nine ten") is not a sum — so it is opt-in for when you are dictating numeric content rather than prose. When enabled it reads explicit digit sequences ("one two three" → "123") and clear compounds ("twenty three" → "23", "two hundred" → "200").
- Pasting transcribed text now uses a Core Graphics keystroke (Cmd+V) instead of driving System Events through AppleScript. This removes a second macOS permission prompt (Automation, on top of Accessibility), drops a subprocess launch from the paste path, and fixes the underlying reason the old code needed AppleScript at all (a thread-safety crash). Only the Accessibility permission is now required to paste.

### Fixed

- Transcription no longer drops the final words of long recordings (#46). On recordings over ~20 seconds the silence trimmer used voice-activity detection to cut both the leading and trailing silence; on quiet input (e.g. a lapel mic) it regularly misjudged the trailing-off end of a sentence as silence and sliced real words away before either transcription engine saw them. This was why both backends truncated at the identical word. The trimmer now removes leading silence only and always keeps the audio through to the very end.
- The recording start tone now plays reliably, without clipping, and without interfering with other audio (#58). It was gated behind a model-readiness check the stop tone doesn't have (so a cold start could begin recording before the check passed and skip the tone), and it was played through NSSound, which shares the app's audio output and got clipped when the app opened the microphone on the first record after the ~45-second warm-stream teardown. Recording cues now play through `AVAudioPlayer`: a mixable CoreAudio client, so the cue plays cleanly regardless of when you last recorded AND does not duck or pause music or other audio. The start tone is also no longer gated on model-readiness — it fires whenever a new recording begins, like the stop tone.
- The "update available" notification can now be dismissed (#67, #52). The toast was shown with infinite duration but only an "Update Now" action, so it could not be cleared without installing the update. It now has a "Later" button that dismisses it, plus a description line. (The old full-width banner with overlapping buttons was already replaced by this corner toast in 2026.6.0; this completes the dismiss-ability the toast was missing.)
- Recording no longer hijacks Bluetooth headphones (e.g. AirPods) into low-quality "call" mode. When the default input is a Bluetooth device, macOS would switch it from high-quality stereo (A2DP) to mono call audio (HFP) the moment Thoth opened it as a microphone — cutting out the user's music and leaving it degraded until the app quit, because the mic stream was held open between recordings. Thoth now records from the built-in microphone instead whenever the default input is Bluetooth (so music keeps playing in the headphones), and if a Bluetooth mic is deliberately selected, its stream is released the instant recording stops rather than held warm. Built-in and USB mics (e.g. a lapel mic) are unaffected.
- CSV export of transcription history is now generated with a standard CSV writer and guards against spreadsheet formula injection: a transcription whose text begins with `=`, `+`, `-` or `@` is no longer interpreted as a formula when the file is opened in Excel, Sheets or LibreOffice.
- The database migration runner no longer treats a genuine read error as "schema version 0", which could have re-run non-idempotent migrations and broken startup; a real error now surfaces instead of being silently swallowed.
- Removed a dead, non-anti-aliased audio resampler that could have aliased non-16kHz input had it ever been reached; the bearer-token check on the loopback Control API now uses the standard request-validation layer; and several duplicated frontend helpers (byte/duration formatting) were unified so they no longer diverge.

## [2026.6.0] - 2026-06-01

### Added

- **Local Control API**: an opt-in, loopback-only (`127.0.0.1`) HTTP API that exposes Thoth's existing control surface to local automation. Protected by a bearer token, off by default. Endpoints cover pipeline state, GPU/system info, transcription history and quality stats, the personal dictionary (list/add/update/delete/import/export), settings (read/update), prompt templates, and asynchronous transcription of local audio files (submit + poll). No new capability — every endpoint mirrors what the GUI can already do. (#65)
- **Bundled MCP server**: a native Model Context Protocol server (rmcp) mounted at `/mcp` on the same loopback server, so MCP-capable assistants (Claude, etc.) can operate Thoth through task-centric tools with no user-written glue. Tools: `dictionary`, `setting`, `transcription` (dispatchers), `transcribe_file` + `transcribe_status`, `get_state`, `get_system`, `list_prompts`. Opt-in; shares the Control API's auth. (#66)
- **Integrations settings pane**: enable/disable the Control API and MCP server, view live status and the served endpoint, and manage the bearer token — masked display with reveal/copy and rotate-with-confirmation.
- **Dictionary table**: the personal dictionary is now a sortable table (click column headers) with a sticky header that scrolls independently.

### Changed

- **Frontend rebuilt on stock shadcn-svelte**: the UI now uses the canonical shadcn-svelte component set and token system, replacing a divergent custom theme layer that had been suppressing component styling.
- **API tokens** use the canonical secret-key shape `sk-thoth-<random>` (CSPRNG, base62), recognisable and greppable for secret scanners.

### Fixed

- **Toggle switches**: render and animate correctly (state styling now matches the installed component library).
- **Recording stuck on "Processing"**: the pipeline now emits its final state after processing completes, so the UI returns to idle.
- **Right Shift hotkey**: modifier-only shortcuts (e.g. Right Shift) register correctly via the keyboard service.
- **About dialogue / dropdowns / history list**: fixed broken modal sizing, raw-value dropdowns, and overlapping history rows introduced by the component migration.
- **History rows**: left-click selects (shows detail); right-click opens the context menu. Selected-row hover stays legible.
- **Audio device & enhancement dropdowns**: show friendly device/model names instead of raw identifiers.
- **Control API**: out-of-range dictionary index returns 404 instead of 500.
- **Output filter persistence**: "Apply sentence case", "Normalise whitespace", and "Clean up punctuation" now persist and are applied at transcription time (previously reverted on return and were never applied).
- **Rogue recording indicator**: the floating indicator no longer appears mid-screen at launch or after the displays wake from sleep; it is now genuinely hidden until recording starts.

## [2026.4.1] - 2026-04-04

### Fixed

- **ObjC crash**: Use proper `block2::RcBlock` completion handler instead of null pointer in microphone permission request (caused SIGABRT crashes)
- **Keyboard service crash**: Use `DeviceState::checked_new()` with inner permission check to prevent process abort when Input Monitoring permission is revoked
- **Microphone status**: Distinguish `not_determined` from `denied` so first-launch users see correct state
- **Stale permission detection**: Replace racy push-based event (fired before webview listener exists) with reliable pull-based check on mount
- **Keyboard service restart**: Auto-start keyboard monitoring when Input Monitoring permission is newly granted (no app restart needed)
- **TCC reset**: Remove broken non-admin `reset_tcc_permission` command; use admin-elevated `reset_tcc_permissions` everywhere
- **Permission reset UX**: Stop unconditionally opening Accessibility pane after TCC reset; let setup card guide user to correct pane

## [2026.4.0] - 2026-04-03

### Added

- **macOS permission reset wizard**: Guided 4-step troubleshooting UI for quarantine, TCC, and accessibility permissions (PR #37)
- **MIT licence**
- **Pre-commit hooks**: gitleaks secret scanning
- **CI**: Reusable auto-label workflow

### Changed

- **Frontend scaffolding**: Migrated toast system to sonner, added lucide-svelte icons
- **Filler word removal**: Only unambiguous hesitation sounds (um, uh, er, ah) are removed; "like" and "you know" preserved
- **Sound feedback**: Replaced afplay subprocess with native NSSound for instant audio feedback on recording start/stop
- **Dev server ports**: Migrated from 1420/1421 to 1422/1423 to avoid conflicts

### Fixed

- **Trailing punctuation**: Consecutive transcriptions no longer run together; a period and trailing space are always appended when needed
- **Clipboard preservation**: Original clipboard contents saved before paste and restored after a configurable delay
- **Global shortcuts suppressed while screen is locked** (closes #23)
- **F14 copy-last-transcription shortcut** not working

## [2026.2.7] - 2026-02-22

### Added

- **Retranscribe recordings**: Re-process existing recordings from history with current model settings
- **Configurable recording indicator style**: Three visual styles (cursor-dot, fixed-float, pill) selectable in Settings
- **Stale TCC permission detection**: Automatically detects stale macOS accessibility/microphone permissions with guided reset flow

### Fixed

- Trailing silence now consistently padded across all transcription backends, preventing truncation at end of speech
- Recording indicator mouse tracker handles macOS sleep/wake cycles gracefully
- Updater shows actionable error states with retry and manual download fallback

## [2026.2.5] - 2026-02-20

### Added

- **Linux GPU acceleration**: CUDA (NVIDIA), HIP/ROCm (AMD), and Vulkan backend support via configurable Cargo features
- **Linux platform support**: GPU detection (`nvidia-smi`, `rocm-smi`, `hipconfig`, `vulkaninfo`), Wayland keyboard capture fallback, `wtype` text insertion for Sway/Hyprland, recording indicator positioning
- **Audio file import**: Drag-and-drop or file picker to transcribe existing audio files via symphonia decoder
- **Toast notification system**: Centralised, non-blocking toast notifications replacing alert dialogues
- **Redesigned history pane**: Select-all, inline search, clear-all, and improved layout
- **First-run onboarding**: Stepped checklist with permission explanations, guided setup state, and model download card
- **Release CI for Linux**: Ubuntu 22.04 added to release build matrix with CUDA dependencies

### Changed

- Visual consistency enforced across all settings panes
- Tray menu shortcuts and overview pane layout updated
- Release workflow: fixed pnpm cache mechanism, platform-conditional CFLAGS, removed signing key env vars
- GPU info displayed in Settings Overview pane

### Fixed

- Transcriptions no longer auto-copied to clipboard by default
- Mouse tracking reliability improved for recording indicator
- Recording blocked when no transcription model is available

## [2026.2.2] - 2026-02-16

### Added

- **AI Enhancement tray integration**: Quick access to AI enhancement toggle and all prompt templates from system tray
- **In-app prompt writing guide**: Dedicated window with comprehensive guidance on writing effective custom prompts
  - Core principles: task specificity, constraints, output directives
  - Template patterns for length-preserving, reducing, and expanding transformations
  - Colour-coded good/bad examples
  - Model-specific guidance (7B+ vs 1.5B-3B models)
  - Troubleshooting table and checklist
- **"Speak Like a Pirate" prompt**: Fun demonstration of creative transformation with proper constraints
- Permission auto-polling: After clicking Grant Access, polls every 2s for up to 30s until macOS reflects permission changes

### Changed

- **Improved all built-in prompts**: Added explicit length constraints, scope limitations, and clear output directives to prevent over-elaboration
- Enlarged app icon glyph 10% (0.60 → 0.66 scale) for better visibility
- Regenerated tray icons with solid silhouettes using flood-fill algorithm instead of outline rendering

### Fixed

- AI prompt over-elaboration issue with small models (e.g., Qwen 2.5 3B producing 5 paragraphs from 2 sentences)
- Config preservation for prompt selection when changed from tray menu

### Removed

- Trigger words system from prompts (unused feature)

## [2026.2.1] - 2026-02-16

### Added

- Scribe's Amber theme and 𓅝 ibis hieroglyph branding (app icon, tray icons, favicon)
- About dialogue with version info and project links
- Audio input source submenu in system tray for quick device switching
- Settings UI consolidated into Overview and History panes
- Eager background model warmup on startup for faster first transcription
- Local-time timestamps in debug logs for readability
- Tray-to-settings sync for audio device changes

### Fixed

- Audio device persistence: dedicated config path prevents accidental overwrite by other settings saves
- Sherpa-ONNX dylibs now included in macOS app bundle (fixes crash on Parakeet models)
- Recording-indicator window added to Tauri capabilities (was missing permissions)
- Replaced stale `@tauri-apps/plugin-global-shortcut` with missing `@tauri-apps/plugin-clipboard-manager` in frontend dependencies

## [2026.2.0] - 2026-02-14

First calendar-versioned release. Covers all work since the Tauri 2.0 migration.

### Added

- History window with detail view, audio playback with waveform, and metadata panel
- Bulk selection and operations in history (delete, export multiple)
- Performance Analysis dashboard for transcription metrics
- Transcription metadata stored in database (duration, model, word count)
- Apple-style compact mic icon indicator positioned above the text cursor
- macOS dictation tones for recording start/stop feedback
- whisper.cpp with Metal GPU acceleration as primary transcription backend
- Native keyboard capture for shortcut recording
- System tray with recording state and quick actions
- JSON/CSV/TXT export formats with shared export logic
- Stable audio device IDs and silence detection before transcription
- Separate display of original and enhanced text in history

### Fixed

- F13 key bounce with debouncing and device preference preservation
- Shortcut recording race conditions and resume overwrite bugs
- Recording indicator fallback when main window is unavailable
- Recording indicator show delay eliminated via pre-warming

### Changed

- Recording indicator redesigned from wide pill to compact rounded mic icon
- Audio capture module split into AudioRecorder and VadRecorder
- Shared component styles extracted to app.css

## [2.0.0] - Tauri Migration

Complete rewrite from Swift/SwiftUI to Tauri v2 + Svelte 5.

### Added

- Cross-platform foundation (macOS now, Linux planned)
- Recording indicator overlay positioned near text cursor
- Real-time audio level metering during recording
- Voice activity detection for speech boundaries
- Hands-free recording mode with configurable timeout
- Remote model manifest for automatic updates
- SQLite database with migrations
- JSON/CSV/TXT export formats
- Shortcut conflict detection

### Changed

- Framework: Swift/SwiftUI to Tauri 2.0 + Rust
- Frontend: SwiftUI to Svelte 5 with runes
- Audio: Core Audio to cpal (cross-platform)
- Transcription: whisper.cpp to Sherpa-ONNX with Parakeet models
- Database: SwiftData to SQLite with rusqlite
- AI: Multi-provider to Ollama-focused

### Removed

- Swift/SwiftUI codebase
- whisper.cpp/Metal GPU acceleration
- macOS-only features (Notch Recorder, etc.)

## [1.0.0] - Swift Version (Archived)

Original Swift/SwiftUI implementation. See `archive/swift-v1` branch.
