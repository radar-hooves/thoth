# Linux Development Setup

How to build and run Thoth from source on Linux. Thoth supports macOS (first-class) and Linux; this guide covers the Linux-specific build dependencies and the runtime packages an installed build needs.

## Build dependencies

Tauri needs the system GTK/WebKit/audio development libraries to compile, and the Whisper Vulkan backend needs the Vulkan toolchain to compile its compute shaders at build time. On Debian/Ubuntu:

```bash
sudo apt-get update
sudo apt-get install -y \
  libgtk-3-dev libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev \
  patchelf libasound2-dev \
  libvulkan-dev glslc spirv-headers
```

- `libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, `librsvg2-dev`, `patchelf` — Tauri's webview and bundler.
- `libappindicator3-dev` — system tray.
- `libasound2-dev` — ALSA, for cpal audio capture and for the synthesised recording start/stop cues.
- `libvulkan-dev`, `glslc`, `spirv-headers` — the Vulkan GPU backend. whisper.cpp's GGML Vulkan backend compiles its shaders at build time via CMake (`find_package(Vulkan COMPONENTS glslc REQUIRED)` and `find_package(SPIRV-Headers REQUIRED)`); `glslang-tools` does **not** satisfy this. Omit these only if you build CPU-only without `--features vulkan`.

The toolchain otherwise is the standard one: a recent stable Rust (via `rustup`), Node.js LTS, and `pnpm`. `direnv` loads the project environment from `.envrc` (run `direnv allow` once), and on Debian/Ubuntu you install the system packages above through apt. On NixOS — or any machine with Nix — the committed `flake.nix` provides the entire toolchain and every build dependency (Rust, Node, pnpm, GTK/WebKit, the Vulkan toolchain); run `nix develop` and build inside that shell (`nix develop .#cuda` adds the CUDA toolkit for `--features cuda`) instead of installing the apt packages.

## Building

```bash
pnpm install

# CPU-only (no GPU build deps needed beyond the GTK/audio set)
pnpm tauri build -- --no-default-features

# Vulkan GPU (vendor-neutral: NVIDIA, AMD, Intel) — what the Linux release ships
pnpm tauri build -- --no-default-features --features vulkan

# NVIDIA-specific (CUDA Toolkit 12.x + drivers)
pnpm tauri build -- --no-default-features --features cuda

# AMD-specific (ROCm 6.x)
pnpm tauri build -- --no-default-features --features hipblas
```

`--no-default-features` is required on Linux: the default features (Parakeet + FluidAudio) are off here. FluidAudio is Apple-Neural-Engine only (macOS/Apple Silicon). Parakeet (the k2-fsa `sherpa-onnx` crate) builds and links statically on Linux and is confirmed at runtime on NixOS: a `--no-default-features --features parakeet-cuda,vulkan` build loads the model and transcribes at roughly 50x real-time on an RTX 3080 (33.3s of audio in 0.61s). Headless smoke test: `cargo run --example parakeet_smoke --no-default-features --features vulkan,parakeet -- <model_dir> <audio.wav>`. The `.deb`/`.AppImage` runtime on Ubuntu is unconfirmed, so Parakeet stays opt-in on Linux: add `--features parakeet` (or `parakeet-cuda` for the NVIDIA path). If the build can't reach GitHub releases to download the prebuilt sherpa archive, point it at a local copy with `SHERPA_ONNX_LIB_DIR`. The default Linux transcription backend is Whisper (whisper.cpp).

Pick exactly one GPU feature; they are mutually exclusive. If GPU initialisation fails at runtime, Thoth falls back to CPU automatically.

## Runtime dependencies

A packaged `.deb` declares these; if you run a raw binary, install them yourself.

**Required:**

- `libvulkan1` — the Vulkan loader, needed by the GPU build at runtime to find the GPU's Vulkan driver. Without it the GPU build silently falls back to CPU. You also need a vendor Vulkan driver (`mesa-vulkan-drivers` for AMD/Intel, the NVIDIA driver's Vulkan ICD for NVIDIA).

**Recommended (the app works without them, with reduced functionality):**

- **A typing tool** — native text insertion. Without one, insertion falls back to XWayland emulation, which on GNOME triggers an "Allow Remote Interaction" permission prompt each session. Which one depends on your desktop, and installing the wrong one buys nothing:

  | Session | Install | Why |
  | --- | --- | --- |
  | Sway, Hyprland, river | `wtype` | Speaks `zwp_virtual_keyboard_manager_v1`, which these compositors implement |
  | KDE Plasma (Wayland) | `kwtype` | KWin does **not** implement the virtual-keyboard protocol, so `wtype` can never work there; kwtype uses KDE's own Fake Input protocol |
  | GNOME (Wayland) | `dotool` or `ydotool` | Mutter does not implement it either, and GNOME has no equivalent of its own; both of these go through `/dev/uinput` instead |
  | X11 | `xdotool` | Talks XTEST, which is X11-only |

  `dotool` and `ydotool` work under any compositor and are the fallback everywhere, at the cost of `/dev/uinput` permissions. Thoth tries them in this order automatically; Settings → Recording → Typing Tool pins one if the automatic choice behaves badly on your setup.
- `xdg-utils` — opens external links and settings panels (`xdg-open`).
- `libayatana-appindicator3-1` — the system tray icon (the modern AppIndicator runtime).

## NixOS and home-manager (Nix flake)

The flake exposes NixOS and home-manager modules (issue #117) so Thoth can be installed declaratively. The packaged binary is already wrapped with its runtime dependencies (wl-clipboard, wtype, the CUDA/Vulkan libraries and GTK resources), so no extra udev rules or system packages are needed.

### NixOS

```nix
{
  inputs = {
    thoth.url = "github:radar-hooves/thoth";
  };

  outputs = { self, nixpkgs, ... }@inputs: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        inputs.thoth.nixosModules.default
        {
          programs.thoth.enable = true;
        }
      ];
    };
  };
}
```

The `programs.thoth` module installs Thoth for every user on the system. No autostart option is provided at the system level — Thoth is a per-user tray application, so autostart belongs in the home-manager module (or Thoth's own "launch at login" setting).

### home-manager (Linux)

```nix
{
  homeConfigurations.myhost = home-manager.lib.homeManagerConfiguration {
    modules = [
      inputs.thoth.homeManagerModules.default
      {
        services.thoth.enable = true;
      }
    ];
  };
}
```

The `services.thoth` module runs Thoth as a systemd user service. Options:

- `services.thoth.package` — the Thoth package (defaults to the flake's build).
- `services.thoth.autostart` — whether to start Thoth with your graphical session (default `true`). Set it to `false` and the service stays available for manual start with `systemctl --user start thoth`.

### home-manager (macOS)

```nix
{
  homeConfigurations.myhost = home-manager.lib.homeManagerConfiguration {
    modules = [
      inputs.thoth.homeManagerModules.darwin
      {
        services.thoth.enable = true;
        services.thoth.package = /* your darwin build */;
      }
    ];
  };
}
```

The darwin module runs Thoth as a launchd agent in the GUI session (`services.thoth.autostart` maps to `RunAtLoad`). Note the flake does not yet build a darwin package (`meta.platforms` is `x86_64-linux` only), so on macOS you must supply `services.thoth.package` yourself — for example, a Nix package wrapping your `pnpm tauri build` output (e.g. via `pkgs.callPackage`).

## AppImage and GPU

The Linux AppImage is built with Vulkan enabled, but it does **not** bundle the Vulkan loader or GPU drivers (`bundleMediaFramework` bundles GStreamer media framework libraries, not GPU drivers — those are host-and-vendor-specific and cannot be portably bundled). For GPU-accelerated transcription from the AppImage, the host must have `libvulkan1` and a working GPU Vulkan driver installed. Without them the AppImage runs transcription on CPU.

## Display server notes

Thoth's behaviour differs between X11 and Wayland because Wayland restricts what an application may do:

- **Global shortcuts**: on Wayland these go through the XDG Desktop Portal, which KDE, wlroots-based compositors, and GNOME 48+ implement. On a compositor without it, Thoth tells you (a toast) and you use a function-key shortcut or an X11 session.
- **Recording indicator**: on Wayland the indicator cannot follow the cursor (no global cursor position) and uses a fixed on-screen position.
- **Modifier-only shortcuts** (e.g. double-tap Right Shift): unavailable on Wayland; use a function-key shortcut instead.

### Known risk to verify on Wayland

The portal's `Activated` D-Bus signals have been reported to stop arriving when the
app is launched from an application launcher (e.g. fuzzel) rather than a terminal —
the underlying zbus connection can be torn down in that environment. When testing,
**launch Thoth both from a terminal and from your normal app launcher/menu, and
confirm the global shortcut fires in both cases.** If it works from a terminal but
not from the launcher, that is this issue; report it (the fix is to hold the portal
proxy and session in process-lifetime state rather than a single task, and re-subscribe
on signal-stream end). Note also that changing a shortcut in Settings does not currently
re-bind the portal live — restart the app after changing a shortcut on Wayland.

## Continuous integration

`.github/workflows/ci.yaml` compiles, lints (`cargo fmt --check`, `cargo clippy -D warnings`), and tests on both macOS and Linux on every pull request, building Linux with `--no-default-features --features vulkan` so the Linux-only code (the Wayland portal, X11 mouse tracking, the Vulkan backend) is compile-checked even though the primary dev machine is macOS. It also validates the `.desktop` entry and type-checks the frontend.
