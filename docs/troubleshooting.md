# Troubleshooting

Practical fixes for the most common Thoth problems, grouped by what you actually see go wrong.

Each entry has a short diagnosis (why it happens) and a fix (what to do). If you are new to Thoth, start with [getting started](getting-started.md); for the related guides see [the dictionary](dictionary.md), [custom prompts](custom-prompts-guide.md), and [automation](automation.md).

A note on terms used below. _macOS permissions_ are the privacy switches under System Settings > Privacy & Security. _Wayland_ and _X11_ are the two display servers Linux uses to draw windows and route input; Wayland is the newer, stricter one and most modern desktops default to it. _The portal_ (XDG Desktop Portal) is the safe, sandboxed bridge a Wayland app must go through to register a global hotkey, because Wayland deliberately forbids apps from grabbing keys directly.

## The recording hotkey does nothing

You press the record key (right Shift on macOS, F13 elsewhere by default) and nothing happens; no recording starts, no indicator appears.

### macOS

Diagnosis: Thoth starts listening for the key once it has the **Accessibility** permission (macOS 27: **Device Control and Data Access**).

Fix: on Thoth's Overview page, click **Grant** next to Permissions and switch Thoth on when System Settings opens. The key works as soon as the switch is on; no restart is needed.

### Linux (X11 vs Wayland)

Diagnosis: on **X11** global hotkeys generally just work. On **Wayland** they only work if your desktop provides the portal's Global Shortcuts service. KDE, wlroots-based compositors (such as Sway and Hyprland), and GNOME 48 or newer implement it; older GNOME and some minimal compositors do not. On a compositor without it, Thoth shows a notification and you will need a workaround.

Fix, in order of preference:

- Pick a **function-key shortcut** (F13 to F20). These are the most reliable to register and least likely to clash with something else.
- If hotkeys still do not fire on Wayland, log into an **X11 session** instead (most login screens offer this as a gear or session menu).
- If a hotkey registers but the wrong thing happens when you press it, your **compositor may have reassigned that key** to one of its own actions; choose a different key in Thoth's settings, or clear the conflicting binding in your desktop's keyboard settings.

Two Wayland specifics worth knowing:

- **Modifier-only shortcuts** (for example double-tapping Right Shift) are refused on Wayland; the portal does not support them. Use a normal key or a function key instead.
- After you change a shortcut on Wayland, **restart Thoth**. Changing the binding in Settings does not currently re-register it with the portal live.

## Text does not appear at my cursor, or paste does nothing

Thoth transcribes fine (you can see it in history), but the text never lands where you were typing.

### macOS

Diagnosis: typing into another app, or sending a paste keystroke, counts as controlling your computer, which needs the **Accessibility** permission.

Fix: click **Grant** on Thoth's Overview page and switch Thoth on (the same switch as for the hotkey).

### Linux

Diagnosis: how Thoth inserts text depends on whether a small helper called `wtype` is installed. `wtype` is the native Wayland tool for simulating keystrokes, but it only works on compositors that expose the Wayland virtual-keyboard protocol. With it, insertion is seamless; without it, Thoth falls back to keyboard simulation through XWayland (the X11 compatibility layer), and on **GNOME** that fallback triggers an **"Allow Remote Interaction"** prompt the first time each session.

Fix:

- Install **`wtype`** for smooth, prompt-free insertion on **Sway, Hyprland, and other wlroots-based compositors** (for example `sudo apt-get install wtype`), then restart Thoth. Note that **GNOME and KDE do not implement the protocol `wtype` needs**, so on those desktops installing it will not help; grant the prompt below instead.
- If you prefer not to install it, grant the **"Allow Remote Interaction"** prompt when GNOME shows it; you will see it once per session.
- **In a terminal**, paste only works with Ctrl+Shift+V (terminals reserve plain Ctrl+V for something else). Thoth already sends **Ctrl+Shift+V** on Linux, which also pastes correctly in normal apps, so terminals are handled for you.

## Transcription is wrong or comes out as garbage

The words are recorded but the text is inaccurate, scrambled, or full of misheard names and jargon.

Diagnosis and fix, from most to least common:

- **No model, or the wrong one, is loaded.** Transcription needs a speech model downloaded to `~/.thoth/models/`. Open Thoth's model management screen and confirm a model is downloaded and selected; download one if not.
- **The wrong microphone is selected.** If Thoth is listening to the wrong device (a built-in mic across the room, say) accuracy collapses. Check the selected input device in settings and pick the mic you actually speak into.
- **Background noise.** Fans, music, or a noisy room degrade recognition. Move somewhere quieter or use a closer microphone (a headset or lapel mic helps a lot).
- **Names, acronyms, and jargon are misheard.** Speech models are phonetic; they will spell unfamiliar words by sound. This is expected, not a bug. The fix is to teach Thoth the correct spellings once. See [the dictionary guide](dictionary.md): the personal dictionary does exact find-and-replace, and the canonical-term registry snaps phonetic variants of a term back to the right spelling so you register a name once and all its mishearings resolve to it.

## No audio, or my recording was silently discarded

You record, but nothing is captured, or the recording vanishes with no text and no error.

Diagnosis and fix:

- **No microphone, or the chosen one is gone.** If your selected mic is unplugged or unavailable, Thoth falls back to the system default device and shows a one-time notice ("Your selected microphone is unavailable; recording from the system default device"). If that notice appears, reselect your intended mic in settings. Thoth never changes your system's default device; it only reads from whichever device you pick.
- **The recording was actually silent.** When a recording produces no speech at all, Thoth treats it as "nothing was said": it discards the audio file rather than showing an error. If recordings keep disappearing, you are likely not being heard; check the mic selection and that you are unmuted, and watch the recording indicator for a level response while you talk.
- **Leading silence is trimmed automatically.** Thoth uses voice-activity detection to trim quiet at the start of a recording so transcription starts on your first word. This is normal and does not remove speech; the end of the recording is never trimmed.

## GPU acceleration is not being used (Linux)

Transcription works but is slower than expected, and you suspect it is running on the CPU.

Diagnosis: the GPU build of Thoth talks to your graphics card through Vulkan. It needs the **Vulkan loader** (`libvulkan1`) plus a working **GPU Vulkan driver** for your card. If either is missing, Thoth quietly falls back to the CPU rather than failing.

Fix:

- Install the Vulkan loader and a driver: `libvulkan1`, plus `mesa-vulkan-drivers` for AMD or Intel, or the Vulkan component of the NVIDIA driver for NVIDIA cards.
- A packaged `.deb` pulls these in for you; a raw binary or the AppImage does not bundle GPU drivers (they are specific to your machine), so install them on the host yourself.
- To confirm which backend is in use, read Thoth's startup lines: it records the compiled backend, for example "loaded with Vulkan GPU acceleration" or a line indicating the CPU backend, and that tells you whether the GPU path engaged. Thoth writes no log file, so the lines have to be caught where they are printed:
  - **Linux, launched from a terminal**: run the binary directly (`thoth`, or `./Thoth-x.y.z.AppImage`) and read them as they appear.
  - **Linux, launched from the applications menu**: the output goes to your user journal — `journalctl --user -b -t thoth`, or `journalctl --user -b` and search for it.
  - **macOS**: `open -a Thoth` will not show you anything, because it hands the app to launchd and nothing is attached to your terminal. Either run the binary inside the bundle from Terminal — `/Applications/Thoth.app/Contents/MacOS/thoth` — or open **Console.app**, pick this Mac under Devices, and filter the process to `thoth`.

## A permission is on in System Settings but Thoth says it is missing (macOS)

Diagnosis: macOS ties each grant to the signature of the app that received it. Releases are signed with one certificate, so updates keep their grants; a build signed differently (an old release, or one built on another machine) leaves a switch that looks on but is not honoured.

Fix: click **Grant** on the Overview page. Thoth clears its own stale entry and macOS asks again; switch Thoth on. No administrator password is involved.

## Where is my data, and how do I uninstall

Diagnosis: Thoth keeps everything in one folder in your home directory and stores nothing in the cloud.

Your data lives under **`~/.thoth/`**:

- `config.json`: your settings
- `dictionary.json` and `canonical_terms.json`: your personal dictionary and canonical terms
- `models/`: downloaded speech models (the largest files)
- `thoth.db`: transcription history (a SQLite database)
- `Recordings/`: saved audio files, subject to your retention setting

To uninstall:

- **macOS**: quit Thoth, then drag **Thoth.app** from your Applications folder to the Trash. To remove your data and models as well, also delete the `~/.thoth/` folder.
- **Linux**: remove the package (for example `sudo apt-get remove thoth`) or delete the AppImage, then delete `~/.thoth/` if you also want to clear your data and models.

Deleting `~/.thoth/` is permanent: it removes your history, dictionary, settings, and downloaded models. Keep a copy of the folder first if you might reinstall and want your history and dictionary back.
