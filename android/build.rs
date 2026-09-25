//! cpal's Android audio backend (Oboe) is C++. Link the NDK's C++ runtime
//! statically so the APK needs no separate libc++_shared.so.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        if let Ok(ndk) = std::env::var("ANDROID_NDK_HOME").or_else(|_| std::env::var("NDK_HOME")) {
            let target = std::env::var("TARGET").unwrap_or_default();
            let arch_dir = match target.as_str() {
                "aarch64-linux-android" => "aarch64-linux-android",
                "x86_64-linux-android" => "x86_64-linux-android",
                "armv7-linux-androideabi" => "arm-linux-androideabi",
                "i686-linux-android" => "i686-linux-android",
                _ => target.as_str(),
            };
            let ndk_path = std::path::Path::new(&ndk);
            let sysroot_lib = ndk_path.join(format!("toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/{arch_dir}"));
            if sysroot_lib.exists() {
                println!("cargo:rustc-link-search=native={}", sysroot_lib.display());
            }
        }
        println!("cargo:rustc-link-lib=static=c++_static");
        println!("cargo:rustc-link-lib=static=c++abi");
    }
}
