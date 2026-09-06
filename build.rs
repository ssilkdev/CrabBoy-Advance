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

#[cfg(not(windows))]
fn main() {}
