//! Calls into the Java `MainActivity` (ROM picker, safe-area insets) over JNI.
//!
//! The Java side does the work that needs the Android framework: launching
//! the system document picker and copying the chosen file into the app's
//! private `files/roms` directory. Rust polls for the result each frame.

use eframe::egui::{self, Rect};
use jni::objects::{JObject, JString};
use jni::JavaVM;

use std::sync::OnceLock;

use winit::platform::android::activity::AndroidApp;

/// JavaVM and the `MainActivity` object, as raw JNI pointers.
struct Handles {
    vm: usize,
    activity: usize,
}
static HANDLES: OnceLock<Handles> = OnceLock::new();

/// Remember the activity. Must be called once from `android_main`.
///
/// `ndk_context` only exposes the *Application* context, which does not have
/// our `MainActivity` methods, so the activity pointer comes from winit's
/// `AndroidApp`, which keeps a global reference to it for the app's lifetime.
pub fn init(app: &AndroidApp) {
    let _ = HANDLES.set(Handles { vm: app.vm_as_ptr() as usize, activity: app.activity_as_ptr() as usize });
}

/// Run `f` with a JNI env attached to this thread and the activity object.
fn with_activity<R>(f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
    let h = HANDLES.get()?;
    // SAFETY: both pointers come from android-activity and stay valid (the
    // activity is held as a JNI global reference) for the process lifetime.
    let vm = unsafe { JavaVM::from_raw(h.vm as *mut jni::sys::JavaVM) }.ok()?;
    let activity = unsafe { JObject::from_raw(h.activity as jni::sys::jobject) };
    let mut env = vm.attach_current_thread().ok()?;
    let result = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    match result {
        Ok(r) => Some(r),
        Err(e) => {
            log::error!("JNI call failed: {e}");
            None
        }
    }
}

/// Open the system file picker; the result arrives via `take_imported_rom`.
pub fn pick_rom() {
    with_activity(|env, act| env.call_method(act, "pickRom", "()V", &[]).map(|_| ()));
}

/// Prefer a ~60 Hz display mode while playing (see MainActivity).
pub fn set_game_refresh_rate(on: bool) {
    with_activity(|env, act| env.call_method(act, "setGameRefreshRate", "(Z)V", &[on.into()]).map(|_| ()));
}

/// A short click for an on-screen button press (see MainActivity).
pub fn vibrate() {
    with_activity(|env, act| env.call_method(act, "vibrate", "()V", &[]).map(|_| ()));
}

/// Apply an `ActivityInfo.SCREEN_ORIENTATION_*` value.
pub fn set_orientation(value: i32) {
    with_activity(|env, act| env.call_method(act, "setOrientation", "(I)V", &[value.into()]).map(|_| ()));
}

fn take_string(method: &str) -> Option<String> {
    with_activity(|env, act| {
        let obj = env.call_method(act, method, "()Ljava/lang/String;", &[])?.l()?;
        if obj.is_null() {
            return Ok(None);
        }
        let s: String = env.get_string(&JString::from(obj))?.into();
        Ok(Some(s))
    })
    .flatten()
}

/// Open the file picker for a skin pack; the result arrives via
/// `take_imported_skin`.
pub fn pick_skin() {
    with_activity(|env, act| env.call_method(act, "pickSkin", "()V", &[]).map(|_| ()));
}

/// Path of a skin pack the user just picked (a temporary copy), if any.
pub fn take_imported_skin() -> Option<String> {
    take_string("takeImportedSkin")
}

/// Absolute path of a ROM the user just imported, if any.
pub fn take_imported_rom() -> Option<String> {
    take_string("takeImportedRom")
}

/// A human-readable import notice (an error, or a note about an imported save).
pub fn take_import_error() -> Option<String> {
    take_string("takeImportError")
}

/// Screen areas covered by the status bar, navigation bar or a camera cutout,
/// in physical pixels.
#[derive(Debug, Clone, Copy, Default)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Insets {
    pub fn to_points(self, ctx: &egui::Context) -> Self {
        let ppp = ctx.pixels_per_point();
        Self { left: self.left / ppp, top: self.top / ppp, right: self.right / ppp, bottom: self.bottom / ppp }
    }

    pub fn shrink(self, r: Rect) -> Rect {
        Rect::from_min_max(
            egui::pos2(r.left() + self.left, r.top() + self.top),
            egui::pos2(r.right() - self.right, r.bottom() - self.bottom),
        )
    }

    pub fn margin(self, extra: f32) -> egui::Margin {
        let m = |v: f32| (v + extra).round().clamp(0.0, 127.0) as i8;
        egui::Margin { left: m(self.left), right: m(self.right), top: m(self.top), bottom: m(self.bottom) }
    }
}

pub fn safe_insets() -> Insets {
    with_activity(|env, act| {
        let arr = env.call_method(act, "getSafeInsets", "()[I", &[])?.l()?;
        if arr.is_null() {
            return Ok(Insets::default());
        }
        let arr = jni::objects::JIntArray::from(arr);
        let mut buf = [0i32; 4];
        env.get_int_array_region(&arr, 0, &mut buf)?;
        Ok(Insets { left: buf[0] as f32, top: buf[1] as f32, right: buf[2] as f32, bottom: buf[3] as f32 })
    })
    .unwrap_or_default()
}
