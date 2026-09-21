// Build script: embeds the Windows PE resource (icon + version metadata).
//
// IMPORTANT: inside a build script, `#[cfg(windows)]` refers to the HOST that
// compiles the project, not the --target being built for. That is exactly the
// gate we want here, because:
//
//   * `winres` is declared under `[target.'cfg(windows)'.build-dependencies]`
//     in Cargo.toml, and build-dependency cfgs are likewise evaluated against
//     the HOST. So the crate only exists when building ON Windows.
//   * Both gates therefore agree: on a Linux/macOS host the resource step is
//     skipped entirely and `winres` is never fetched or compiled.
//
// Consequence: cross-compiling to Windows FROM Linux produces a working .exe
// with no embedded icon/version resource. Build on Windows to get those.
#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "CrabBoy Advance");
    res.set("FileDescription", "CrabBoy Advance - Game Boy Advance Emulator");
    res.set("LegalCopyright", "Copyright (c) 2026 CrabBoy Advance Contributors");
    if let Err(e) = res.compile() {
        eprintln!("Warning: Failed to compile Windows resource icon: {}", e);
    }

    // For GNU toolchains, pass resource.o directly as a linker argument to guarantee
    // the .rsrc PE section is preserved and not pruned from the archive.
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let res_obj = std::path::Path::new(&out_dir).join("resource.o");
        if res_obj.exists() {
            println!("cargo:rustc-link-arg={}", res_obj.display());
        }
    }
}

// Non-Windows hosts (Linux, macOS): nothing to embed. The application icon is
// loaded at runtime from assets/icon_256.png (see src/main.rs), and desktop
// integration is handled by packaging/linux/.
#[cfg(not(windows))]
fn main() {}
