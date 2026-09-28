{
  description = "Thoth - Privacy-first, offline-capable voice transcription application";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
          config = {
            allowUnfree = true;  # Required for CUDA packages
            allowBroken = true;  # webkitgtk for Tauri on Linux
            cudaSupport = true;
          };
        };

        # Rust toolchain with Tauri prerequisites
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        # Build the package with this newer toolchain too — nixpkgs' default rustc
        # (from the pinned nixpkgs) is too old for some deps (e.g. libsqlite3-sys
        # uses the `cfg_select!` macro).
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        # CUDA packages for whisper.cpp GPU acceleration
        cudaPackages = pkgs.cudaPackages_12;

        # CUDA-enabled sherpa-onnx prebuilt (k2-fsa release) for GPU Parakeet.
        # The `parakeet-cuda` cargo feature links sherpa-onnx as `shared`, and
        # SHERPA_ONNX_LIB_DIR points it here instead of downloading the CPU build.
        # This archive ships libsherpa-onnx-c-api.so + libonnxruntime.so with the
        # CUDA execution provider (libonnxruntime_providers_cuda.so). cuDNN/cudart
        # are supplied via LD_LIBRARY_PATH in the `cuda` dev shell below.
        sherpaOnnxCuda = pkgs.stdenvNoCC.mkDerivation {
          pname = "sherpa-onnx-cuda";
          version = "1.13.2";
          src = pkgs.fetchurl {
            url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-cuda-12.x-cudnn-9.x-linux-x64-gpu.tar.bz2";
            hash = "sha256-vRE8k6GLoPm24MrEaramoXYGvde3cbbq7gy9b5bOY/4=";
          };
          dontConfigure = true;
          dontBuild = true;
          installPhase = "mkdir -p $out && cp -r lib $out/lib";
        };

        # Dev-shell packages (shared by both shells).
        commonPackages = with pkgs; [
          # Rust / Tauri
          rustToolchain
          cargo
          rustc
          rust-analyzer

          # Tauri dependencies (platform-specific)
          openssl
          pkg-config
        ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
          # Linux-only Tauri dependencies
          webkitgtk_4_1
          libappindicator-gtk3
          librsvg
          alsa-lib
          # whisper.cpp needs libclang for bindgen
          llvmPackages.libclang
          # X11 development libraries for x11rb (mouse tracking, display detection)
          libx11
          libxcursor
          libxrandr
          libxi
          # Vulkan for whisper.cpp GPU acceleration (AMD & NVIDIA)
          vulkan-loader
          vulkan-headers
          vulkan-tools
          # Shader compiler for Vulkan
          shaderc
          # CUDA toolkit for whisper.cpp CUDA acceleration (NVIDIA GPUs)
          cudaPackages.cudatoolkit
          cudaPackages.cuda_nvcc
          cudaPackages.cuda_cudart
          cudaPackages.cuda_cccl
          cudaPackages.libcublas
          # GCC for CUDA compilation
          gcc
        ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
          # macOS: applesoft libraries (via Xcode) are used automatically
          libiconv
          # libclang for bindgen (whisper.cpp)
          llvmPackages.libclang
          # Scoped `swift` shim (issue #93). FluidAudio's vendored build.rs runs a
          # bare `swift build`, which inherits the nixpkgs apple-sdk-14.4 setup-hook's
          # leaked SDKROOT/DEVELOPER_DIR (a Swift 5.10 SDK) — but /usr/bin/swift is
          # Xcode's swiftc 6.2.x, which rejects the 5.10 SDK ("no such module
          # 'SwiftShims'" / "SDK not supported by the compiler"). This shim, placed
          # ahead of /usr/bin on PATH, peels the leaked vars off only for the Swift
          # build and points it at the real Xcode toolchain, leaving the nix
          # cc-wrapper / rustc paths (which want the Nix SDK) untouched. The `unset`
          # before xcode-select and the explicit PATH prepend are both load-bearing.
          (pkgs.writeShellScriptBin "swift" ''
            if [ -d /Applications/Xcode.app ]; then
              unset SDKROOT DEVELOPER_DIR
              export DEVELOPER_DIR="$(/usr/bin/xcode-select -p 2>/dev/null || echo /Applications/Xcode.app/Contents/Developer)"
              export PATH="/usr/bin:/bin:$PATH"
              export MACOSX_DEPLOYMENT_TARGET=14.0
            fi
            exec /usr/bin/swift "$@"
          '')
        ] ++ [
          # Frontend
          nodejs_22
          pnpm

          # Build tools
          cmake
        ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
          glib
          libsecret
          # Native Wayland keyboard simulation (alternative to X11-based enigo)
          wtype
        ];

        # whisper-rs-sys runs bindgen over ggml-vulkan.h. bindgen invokes libclang
        # directly, bypassing the nix cc-wrapper, so it cannot find the libc headers
        # (stdio.h) or clang's own builtin headers (stddef.h). bindgen then errors and
        # whisper-rs-sys SILENTLY falls back to its bundled no-Vulkan bindings, so the
        # ggml_backend_vk_* symbols go missing and whisper-rs fails to compile its
        # Vulkan module (issue #64). Feed bindgen the cc-wrapper's libc flags plus
        # clang's resource dir. A standard apt system finds these in /usr/include and
        # lib/clang, so CI does not need this.
        bindgenHook = pkgs.lib.optionalString pkgs.stdenv.isLinux ''

          export BINDGEN_EXTRA_CLANG_ARGS="$(< ${pkgs.stdenv.cc}/nix-support/libc-cflags) -idirafter ${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.lib.versions.major pkgs.llvmPackages.libclang.version}/include"
        '';

        # macOS (issue #93): the `swift` shim builds FluidAudio's objects against
        # the real Xcode runtime, but the FINAL Rust link still fails for two
        # reasons, both because the nix linker is pointed at the apple-sdk-14.4
        # (Swift 5.10) SDK via the leaked SDKROOT:
        #   1. `ld: library not found for -lswiftCore` — the 14.4 SDK has no Swift
        #      runtime stubs; add `-L <xcode-sdk>/usr/lib/swift`.
        #   2. `_OBJC_CLASS_$_MLState` / CoreML `MLModel.makeState()` undefined —
        #      FluidAudio's streaming ASR uses the stateful CoreML API, which is
        #      `API_AVAILABLE(macos 15.0)` and exists ONLY in the Xcode SDK's
        #      CoreML.framework (it is entirely absent from apple-sdk-14.4's
        #      CoreML.tbd); add a framework search path (`-L framework=<xcode-sdk>/
        #      System/Library/Frameworks`) so `framework=CoreML` resolves there. ld
        #      weak-imports the 15.0 symbols against the 14.0 deployment target,
        #      matching the runtime backend gate.
        # Both paths MUST be computed with the leaked apple-sdk vars unset —
        # otherwise `xcrun` resolves to the Nix 14.4 SDK (no swiftCore, no MLState)
        # instead of Xcode's. Done in the shellHook (not a static mkShell attr) so
        # `xcrun` runs at shell entry, and appended to (not clobbering) RUSTFLAGS.
        darwinSwiftLinkHook = pkgs.lib.optionalString pkgs.stdenv.isDarwin ''

          __thoth_xcode_sdk="$(env -u SDKROOT -u DEVELOPER_DIR /usr/bin/xcrun --sdk macosx --show-sdk-path 2>/dev/null)"
          if [ -n "$__thoth_xcode_sdk" ] && [ -d "$__thoth_xcode_sdk/usr/lib/swift" ]; then
            export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }-L $__thoth_xcode_sdk/usr/lib/swift -L framework=$__thoth_xcode_sdk/System/Library/Frameworks"
          fi
          unset __thoth_xcode_sdk
        '';

        # One dev-shell definition, optionally wired for GPU Parakeet (CUDA).
        mkThothShell = { gpuParakeet ? false }: pkgs.mkShell ({
          # Platform-specific library paths (Linux). With gpuParakeet, also expose
          # the CUDA sherpa-onnx libs + cuDNN so the CUDA execution provider loads.
          LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux
            (pkgs.lib.makeLibraryPath ([
              pkgs.libappindicator-gtk3
              pkgs.vulkan-loader
              cudaPackages.cuda_cudart
              cudaPackages.cuda_cccl
              cudaPackages.libcublas
            ] ++ pkgs.lib.optionals gpuParakeet [
              sherpaOnnxCuda           # libsherpa-onnx-c-api.so + onnxruntime CUDA EP
              cudaPackages.cudnn       # libcudnn.so.9
              # The onnxruntime CUDA execution provider dlopen()s the full CUDA
              # math-library set; a single missing one makes it abort (no CPU
              # fallback), so provide all of them.
              cudaPackages.libcurand   # libcurand.so.10
              cudaPackages.libcufft    # libcufft.so.11
              cudaPackages.libcusparse # libcusparse.so.12
            ]) + ":/run/opengl-driver/lib");  # NVIDIA driver (libcuda.so)

          # Workaround for webkit2gtk Wayland issues (Linux only)
          # See: https://github.com/tauri-apps/tauri/issues/9460
          WEBKIT_DISABLE_COMPOSITING_MODE = pkgs.lib.optionalString pkgs.stdenv.isLinux "1";

          # libclang for whisper.cpp bindgen
          LIBCLANG_PATH = pkgs.lib.makeLibraryPath [ pkgs.llvmPackages.libclang ];

          # CUDA environment variables for whisper.cpp
          CUDA_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux "${cudaPackages.cudatoolkit}";
          CUDA_HOME = pkgs.lib.optionalString pkgs.stdenv.isLinux "${cudaPackages.cudatoolkit}";

          # Linker search path for CUDA driver (libcuda.so)
          RUSTFLAGS = pkgs.lib.optionalString pkgs.stdenv.isLinux "-L /run/opengl-driver/lib";

          packages = commonPackages
            ++ pkgs.lib.optionals (gpuParakeet && pkgs.stdenv.isLinux) [ cudaPackages.cudnn ];

          shellHook = ''
            echo "𓅝 Thoth Development Environment${pkgs.lib.optionalString gpuParakeet " (GPU Parakeet / CUDA)"}"
            echo "================================"
            echo "  Rust: $(rustc --version)"
            echo "  Node: $(node --version)"
            echo "  pnpm: $(pnpm --version)"
            echo ""
          '' + (if gpuParakeet then ''
            echo "GPU Parakeet (NVIDIA CUDA) is wired up. Build/run with:"
            echo "  pnpm tauri dev --no-default-features --features parakeet-cuda,vulkan"
            echo "  pnpm tauri build --no-default-features --features parakeet-cuda,vulkan"
            echo ""
            echo "Then transcribe and watch 'nvidia-smi' to confirm the GPU engages."
            echo "Logs show 'Attempting CUDA provider...' / 'CUDA provider initialised'."
          '' else ''
            echo "Commands:"
            echo "  pnpm install        - Install dependencies"
            echo "  pnpm tauri dev      - Start development build"
            echo "  pnpm tauri dev -- --features cuda    - Dev with CUDA GPU acceleration"
            echo "  pnpm tauri build -- --features cuda  - Build with CUDA"
            echo "  cargo test          - Run Rust tests (from src-tauri/)"
            echo ""
            echo "GPU Acceleration (Linux):"
            echo "  --features cuda     - NVIDIA GPUs (Whisper)"
            echo "  --features vulkan   - Cross-platform (Whisper)"
            echo "  nix develop .#cuda  - GPU Parakeet (NVIDIA, via sherpa-onnx CUDA)"
          '') + bindgenHook + darwinSwiftLinkHook;
        } // pkgs.lib.optionalAttrs gpuParakeet {
          # Make sherpa-onnx-sys link the CUDA libs instead of downloading CPU ones.
          SHERPA_ONNX_LIB_DIR = "${sherpaOnnxCuda}/lib";
        });

        # Runtime libraries the wrapped binary loads (CUDA EP + Vulkan + tray).
        runtimeLibs = [
          pkgs.libappindicator-gtk3
          pkgs.vulkan-loader
          sherpaOnnxCuda
          cudaPackages.cudnn
          cudaPackages.libcurand
          cudaPackages.libcufft
          cudaPackages.libcusparse
          cudaPackages.cuda_cudart
          cudaPackages.libcublas
        ];

        # ---------------------------------------------------------------------
        # Installable, importable package:
        #   inputs.thoth.packages.${system}.default
        #
        # Builds GPU Parakeet (CUDA, via the prebuilt sherpa-onnx pinned above)
        # plus Whisper (Vulkan). The binary is wrapped with the Wayland runtime
        # tools (wl-clipboard, wtype) and the CUDA/Vulkan libraries it dlopen()s.
        # `hyprctl` is taken from the user's PATH (present on any Hyprland system).
        #
        # Refresh the two hashes whenever Cargo.lock / pnpm-lock.yaml change:
        # set both to lib.fakeHash, run `nix build`, paste the reported pnpmDeps
        # hash, run again, paste the cargoHash, run once more to compile.
        # ---------------------------------------------------------------------
        thothPackage = rustPlatform.buildRustPackage (finalAttrs: {
          pname = "thoth";
          # Read from tauri.conf.json rather than hardcoding: scripts/bump-version.sh
          # only rewrites Cargo.toml, tauri.conf.json and package.json, so a literal
          # here silently rots at every release (it sat at 2026.6.3 while the app
          # shipped 2026.6.7, naming the store path after a version that did not
          # match the binary). Deriving it means the two cannot disagree.
          version =
            (builtins.fromJSON (builtins.readFile ./src-tauri/tauri.conf.json)).version;
          src = ./.;

          cargoRoot = "src-tauri";
          buildAndTestSubdir = "src-tauri";
          # crates.io rate-limits the bulk vendor fetch (random 403s), so use
          # cargoLock instead of cargoHash: each crate becomes its own fetchurl
          # derivation — cached individually and retried by nix — so a throttled
          # `nix build --max-jobs 2` makes steady progress through the limit.
          # Registry checksums come from Cargo.lock; only the git dep needs a hash.
          cargoLock = {
            lockFile = ./src-tauri/Cargo.lock;
            outputHashes = {
              "fluidaudio-rs-0.10.0" = "sha256-z7c8tibtfevefrYAwh3hJM/sr/OWnbSrxjDS4Tda8+k=";
              # STILL fakeHash — blocked on an operator decision, not merely unrun.
              # `radar-hooves/full-stack-app-template` is a PRIVATE repo, unlike every other git
              # dependency here (fluidaudio-rs, and the telemetry-rs repo this crate moved out of)
              # which are public, and there is no credential this session holds or can mint that
              # closes that gap:
              #
              # - `nix build`'s `fetchgit` FOD runs the git fetch inside Nix's build sandbox, which
              #   strips `$HOME`/`~/.gitconfig`/credential helpers entirely — confirmed by testing
              #   `netrc-file` and `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_*` env vars against it (neither
              #   reaches the builder) and a plain public-repo control fetch (which succeeds).
              #   `cargoLock.outputHashes` always calls plain `fetchgit { url; rev; sha256; }` with
              #   no credential extension point — `fetchgit` itself supports `netrcPhase`/
              #   `netrcImpureEnvVars`, but nothing routes them through `outputHashes`.
              # - Plain `cargo build`/`test`/`clippy` in `.github/workflows/ci.yaml`'s hosted
              #   `macos-15`/`ubuntu-22.04` runners hit the same wall a different way: `actions/
              #   checkout` persists its per-job `GITHUB_TOKEN` as an `http.https://github.com/
              #   .extraheader` git-config entry, which only the system git CLI reads — added
              #   `../.cargo/config.toml`'s `net.git-fetch-with-cli = true` so cargo shells out to
              #   it instead of its default libgit2 backend, confirmed against a real CI run. That
              #   still was not enough: the default `GITHUB_TOKEN` is scoped to Thoth alone, and
              #   the repo-to-repo "Access" setting under full-stack-app-template's own Settings →
              #   Actions → General (tried 28/09/2026, `access_level=organization`, since reverted
              #   to `none`) turned out to govern reusable-*workflow* calls between repos, not git
              #   content access — it made no difference to the fetch and was the wrong lever.
              #
              # What actually closes this: (a) make the factory repo public — matches its own kit
              # README, which documents a plain unauthenticated `git = "https://github.com/..."`
              # dependency line with no credential step, the strongest signal this is a visibility
              # oversight rather than intent; or (b) mint a dedicated fine-grained PAT for
              # cross-repo read (GitHub's API has no endpoint to create one — it needs the web UI,
              # matching the `GHCR_PAT`-shaped secret mcp-servers and godswood already carry for
              # exactly this "clone a private sibling repo in CI" case) and store it as a Thoth repo
              # secret, consumed by both a `.github/workflows/ci.yaml` step (`git config --global
              # url."https://x-access-token:${SECRET}@github.com/".insteadOf "https://github.com/"`
              # — the CLI-backend switch above is what makes this reach cargo) and, for the Nix
              # tier specifically, a hand-written `fetchgit` call carrying `netrcPhase`, replacing
              # this dependency's entry in `cargoLock.outputHashes`. Once resolved, replace both
              # `pkgs.lib.fakeHash` values below with what `nix build` reports.
              "telemetry-0.6.0" = pkgs.lib.fakeHash;
              "tauri-plugin-telemetry-0.1.0" = pkgs.lib.fakeHash;
            };
          };

          # GPU Parakeet (sherpa-onnx CUDA via SHERPA_ONNX_LIB_DIR) + Whisper (Vulkan).
          # No default features (drops fluidaudio, a macOS-only git dep, and the
          # CPU `parakeet` link mode).
          buildNoDefaultFeatures = true;
          buildFeatures = [ "parakeet-cuda" "vulkan" ];

          pnpmDeps = pkgs.fetchPnpmDeps {
            inherit (finalAttrs) pname version src;
            fetcherVersion = 3;
            hash = "sha256-BmfIZTXKC4/DB1BfK5dsD7kF/JCZSb9Yc3coQxTB9J0=";
          };

          nativeBuildInputs = with pkgs; [
            cargo-tauri.hook
            nodejs
            pnpmConfigHook
            pnpm
            pkg-config
            cmake
            git
            llvmPackages.libclang
            shaderc # glslc — compiles whisper.cpp's Vulkan shaders
            wrapGAppsHook4
            makeWrapper
          ];

          buildInputs = with pkgs; [
            openssl
            webkitgtk_4_1
            glib
            glib-networking
            libsecret
            libappindicator-gtk3
            alsa-lib
            librsvg
            libx11
            libxcursor
            libxrandr
            libxi
            vulkan-loader
            vulkan-headers
            shaderc
            sherpaOnnxCuda # Parakeet C API, linked via SHERPA_ONNX_LIB_DIR
          ];

          env = {
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            WEBKIT_DISABLE_COMPOSITING_MODE = "1";
            SHERPA_ONNX_LIB_DIR = "${sherpaOnnxCuda}/lib";
          };

          # whisper-rs-sys bindgen needs the cc-wrapper libc flags + clang headers
          # (issue #64); `$(< … )` must run at build time, so it lives here.
          preConfigure = bindgenHook;

          # No updater signing key in the sandbox.
          preBuild = ''
            substituteInPlace src-tauri/tauri.conf.json \
              --replace-fail '"createUpdaterArtifacts": true' '"createUpdaterArtifacts": false'
          '';

          postFixup = ''
            wrapProgram $out/bin/thoth \
              --prefix PATH : ${pkgs.lib.makeBinPath [
                pkgs.wl-clipboard      # wl-copy / wl-paste
                pkgs.wtype             # Wayland keyboard simulation
                pkgs.glib.bin          # gsettings (theme detection)
                pkgs.libcanberra-gtk3  # canberra-gtk-play (sound feedback)
              ]} \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs}:/run/opengl-driver/lib \
              --set WEBKIT_DISABLE_COMPOSITING_MODE 1
          '';

          doCheck = false; # tests need audio hardware

          meta = with pkgs.lib; {
            description = "Privacy-first, offline-capable voice transcription (GPU Parakeet + Whisper)";
            homepage = "https://github.com/radar-hooves/thoth";
            license = licenses.mit;
            platforms = [ "x86_64-linux" ];
            mainProgram = "thoth";
          };
        });

      in {
        # `nix build` / `inputs.thoth.packages.${system}.default`
        packages.default = thothPackage;
        packages.thoth = thothPackage;

        devShells.default = mkThothShell { };
        devShells.cuda = mkThothShell { gpuParakeet = true; };

        # Module instantiation checks (issue #117).
        #
        # The modules below are the only outputs nothing else exercises: a
        # package break shows up in `nix build`, but a module that stopped
        # instantiating — a renamed package attribute, an option type that no
        # longer accepts its default — is invisible until a NixOS user tries
        # to rebuild. These make `nix flake check` catch that, which the Nix
        # workflow runs on every PR (.github/workflows/nix-check.yaml).
        #
        # Evaluation IS the assertion here, so these stay cheap and are still
        # meaningful under `nix flake check --no-build`.
        checks = nixpkgs.lib.optionalAttrs (system == "x86_64-linux") {
          # package.json's packageManager pin and the pnpm this flake supplies
          # are two copies of one version, and nothing used to assert they
          # agreed (the "Single source of truth" rule in .claude/CLAUDE.md).
          #
          # They must agree because they are used in the same build: CI's
          # pnpm/action-setup reads packageManager to pick a pnpm, while
          # fetchPnpmDeps and the dev shell use the nixpkgs one. A mismatch
          # means the lockfile is resolved by one pnpm and consumed by
          # another, and the pnpmDeps hash is computed against a store layout
          # the other may not reproduce.
          #
          # This is why the pnpm bump in #123 was held back rather than
          # applied to package.json alone: nixpkgs is the constraint, so the
          # two must move together. This check makes that non-optional — the
          # next attempt to bump one side fails here with both versions named,
          # instead of drifting quietly the way the app version, the shortcut
          # defaults, and the desktop-file-utils list each did.
          pnpm-version-matches =
            let
              declared =
                (builtins.fromJSON (builtins.readFile ./package.json)).packageManager;
              expected = "pnpm@${pkgs.pnpm.version}";
            in
            if declared != expected then
              throw ''
                pnpm version drift between package.json and nixpkgs.

                  package.json packageManager : ${declared}
                  nixpkgs pnpm                : ${expected}

                These must match. To change the pnpm version, move BOTH:
                update the packageManager field and bump the nixpkgs input to
                one carrying that pnpm (check with
                `nix eval --raw nixpkgs#pnpm.version`). Bumping package.json
                alone leaves the Nix build resolving the lockfile with a
                different pnpm than CI does.
              ''
            else
              pkgs.runCommand "thoth-pnpm-version-check" { } "touch $out";

          # Full NixOS evaluation. Proves the module imports, that
          # `programs.thoth.enable` wires the package default through
          # `self.packages`, and that the package lands in systemPackages.
          nixos-module =
            let
              sys = nixpkgs.lib.nixosSystem {
                inherit system;
                modules = [
                  self.nixosModules.default
                  {
                    programs.thoth.enable = true;
                    # Minimum a NixOS evaluation needs to be complete.
                    boot.loader.grub.devices = [ "/dev/sda" ];
                    fileSystems."/" = { device = "/dev/sda1"; fsType = "ext4"; };
                    system.stateVersion = "25.05";
                  }
                ];
              };
              hasThoth = builtins.any
                (p: (p.pname or "") == "thoth")
                sys.config.environment.systemPackages;
            in
            assert hasThoth;
            pkgs.runCommand "thoth-nixos-module-check" { } "touch $out";

          # The home-manager module, evaluated against stub option
          # declarations rather than real home-manager.
          #
          # Deliberate: taking home-manager as a flake input would drag it
          # into the lock of every downstream consumer of `inputs.thoth`,
          # purely for a test. The stubs cover what actually rots here — the
          # module's own option declarations, the package wiring, and the
          # generated systemd unit. They do NOT verify compatibility with
          # home-manager's real option types; that is the gap this trades
          # for not inflating consumers' dependency graphs.
          home-manager-module =
            let
              evaluated = nixpkgs.lib.evalModules {
                modules = [
                  self.homeManagerModules.default
                  {
                    # Stand-ins for the home-manager options the module sets.
                    options = {
                      home.packages = nixpkgs.lib.mkOption {
                        type = nixpkgs.lib.types.listOf nixpkgs.lib.types.package;
                        default = [ ];
                      };
                      systemd.user.services = nixpkgs.lib.mkOption {
                        type = nixpkgs.lib.types.attrsOf (nixpkgs.lib.types.attrsOf nixpkgs.lib.types.anything);
                        default = { };
                      };
                    };
                  }
                  { _module.args.pkgs = pkgs; }
                  { services.thoth.enable = true; }
                ];
              };
              unit = evaluated.config.systemd.user.services.thoth;
              # The module assigns ExecStart as a plain string; real
              # home-manager coerces it to a list, the stubs above do not.
              # Accept either so this does not depend on that coercion.
              execStart =
                if builtins.isList unit.Service.ExecStart
                then builtins.head unit.Service.ExecStart
                else unit.Service.ExecStart;
              startsThoth = nixpkgs.lib.hasSuffix "/bin/thoth" execStart;
              autostarts = builtins.elem "graphical-session.target" unit.Install.WantedBy;
            in
            assert startsThoth;
            assert autostarts;
            pkgs.runCommand "thoth-home-manager-module-check" { } "touch $out";
        };
      })
    // {
      # NixOS and home-manager modules (issue #117). Defined outside
      # eachDefaultSystem: they are system-agnostic, and the package default
      # is wired lazily via `self.packages` when a user's configuration
      # evaluates, so `inputs.thoth.nixosModules.default` gives a working
      # declarative install out of the box.
      nixosModules.default = {
        lib,
        pkgs,
        ...
      }: {
        imports = [ ./nix/module.nix ];
        programs.thoth.package =
          lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.thoth;
      };

      homeManagerModules.default = {
        lib,
        pkgs,
        ...
      }: {
        imports = [ ./nix/hm-module.nix ];
        services.thoth.package =
          lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.thoth;
      };

      # macOS home-manager module. Thoth does not yet build for darwin through
      # this flake (the package is meta.platforms x86_64-linux only), so there
      # is no package default to wire — darwin users must set
      # `services.thoth.package` to a darwin-capable build themselves.
      homeManagerModules.darwin = {
        ...
      }: {
        imports = [ ./nix/hm-module-darwin.nix ];
      };
    };
}
