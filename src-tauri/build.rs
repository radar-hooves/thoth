fn main() {
    // whisper-rs Metal code uses @available() checks that require the Clang runtime
    // (___isPlatformVersionAtLeast). When sherpa-rs is excluded (no parakeet feature),
    // this runtime isn't linked transitively, so we must link it explicitly.
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("clang")
            .arg("--print-runtime-dir")
            .output()
        {
            let runtime_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !runtime_dir.is_empty() {
                println!("cargo:rustc-link-search={runtime_dir}");
                println!("cargo:rustc-link-lib=dylib=clang_rt.osx");
            }
        }

        // FluidAudio links Swift runtime (swiftCore) dynamically. On deployment targets
        // < macOS 15.0, libswift_Concurrency.dylib resolves via @rpath, so the binary
        // needs rpath entries pointing to Swift runtime library directories.
        #[cfg(feature = "fluidaudio")]
        {
            // Supported macOS versions ship Swift; never search the build host's Xcode.
            println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        }
    }

    tauri_build::build();
}
