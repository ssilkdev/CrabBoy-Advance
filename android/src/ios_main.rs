//! iOS / iPadOS executable: UIKit needs a real `main` on the main thread.
//! On every other target this binary does nothing (Android loads the cdylib).

#[cfg(target_os = "ios")]
fn main() {
    crabboy_android::ios_main();
}

#[cfg(not(target_os = "ios"))]
fn main() {}
