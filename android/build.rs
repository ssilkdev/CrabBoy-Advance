//! cpal's Android audio backend (Oboe) is C++. Link the NDK's C++ runtime
//! statically so the APK needs no separate libc++_shared.so.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-lib=static=c++_static");
        println!("cargo:rustc-link-lib=static=c++abi");
    }
}
