//! iOS / iPadOS counterpart of `platform.rs` (which calls into Java).
//!
//! UIKit work happens here through objc2: the system document picker
//! (`UIDocumentPickerViewController`, copying picked files into the app's
//! `Documents/roms`), safe-area insets and CoreMotion gravity for tilt.
//! Everything is called from egui's `update`, which winit runs on the main
//! thread, so a `MainThreadMarker` is always available.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use eframe::egui::{self, Rect};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_core_motion::CMMotionManager;
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString, NSURL};
use objc2_ui_kit::{
    UIApplication, UIDocumentPickerDelegate, UIDocumentPickerViewController, UIInterfaceOrientation, UIWindow,
};
use objc2_uniform_type_identifiers::UTType;

const MAX_ROM_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SKIN_BYTES: u64 = 16 * 1024 * 1024;

static FILES_DIR: OnceLock<PathBuf> = OnceLock::new();
static IMPORTED_ROM: Mutex<Option<String>> = Mutex::new(None);
static IMPORTED_SKIN: Mutex<Option<String>> = Mutex::new(None);
static IMPORT_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq)]
enum PickKind {
    Rom,
    Skin,
}

thread_local! {
    /// The picker only holds its delegate weakly; keep ours alive here.
    static DELEGATE: RefCell<Option<Retained<PickerDelegate>>> = const { RefCell::new(None) };
    static PICK_KIND: Cell<PickKind> = const { Cell::new(PickKind::Rom) };
    static MOTION: RefCell<Option<Retained<CMMotionManager>>> = const { RefCell::new(None) };
}

/// Remember the app's data directory (`Documents`). Call once at startup.
pub fn init(files_dir: &Path) {
    let _ = FILES_DIR.set(files_dir.to_path_buf());
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CrabBoyPickerDelegate"]
    struct PickerDelegate;

    unsafe impl NSObjectProtocol for PickerDelegate {}

    unsafe impl UIDocumentPickerDelegate for PickerDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _controller: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            let paths: Vec<PathBuf> =
                urls.iter().filter_map(|u| u.path()).map(|p| PathBuf::from(p.to_string())).collect();
            let kind = PICK_KIND.with(|k| k.get());
            // Copy off the main thread: a 32 MB ROM from iCloud can be slow.
            std::thread::spawn(move || match kind {
                PickKind::Rom => import_roms(&paths),
                PickKind::Skin => {
                    if let Some(p) = paths.first() {
                        import_skin(p)
                    }
                }
            });
        }
    }
);

impl PickerDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn key_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let app = UIApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    let key = app.keyWindow();
    #[allow(deprecated)]
    key.or_else(|| app.windows().firstObject())
}

fn present_picker(kind: PickKind) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(root) = key_window(mtm).and_then(|w| w.rootViewController()) else {
        set(&IMPORT_ERROR, "No window to show the file picker in".into());
        return;
    };
    // ROMs have no registered type; accept any data and check names after.
    let Some(ty) = UTType::typeWithIdentifier(&NSString::from_str("public.data")) else { return };
    let types = NSArray::from_retained_slice(&[ty]);
    let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(
        UIDocumentPickerViewController::alloc(mtm),
        &types,
        true,
    );
    picker.setAllowsMultipleSelection(kind == PickKind::Rom);
    let delegate = DELEGATE.with(|d| d.borrow_mut().get_or_insert_with(|| PickerDelegate::new(mtm)).clone());
    picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    PICK_KIND.with(|k| k.set(kind));
    root.presentViewController_animated_completion(&picker, true, None);
}

/// Open the system file picker; results arrive via `take_imported_rom`.
pub fn pick_rom() {
    present_picker(PickKind::Rom);
}

/// Open the file picker for a skin pack; the result arrives via
/// `take_imported_skin`.
pub fn pick_skin() {
    present_picker(PickKind::Skin);
}

fn set(slot: &Mutex<Option<String>>, v: String) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
}

fn take(slot: &Mutex<Option<String>>) -> Option<String> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect()
}

/// Copy `src` to `dest` through a `.part` file, refusing anything over `max`.
fn copy_capped(src: &Path, dest: &Path, max: u64) -> Result<(), String> {
    let len = std::fs::metadata(src).map_err(|e| e.to_string())?.len();
    if len == 0 {
        return Err("that file is empty".into());
    }
    if len > max {
        return Err(format!("file is larger than {} MB", max / (1024 * 1024)));
    }
    let tmp = dest.with_extension("part");
    std::fs::copy(src, &tmp).and_then(|_| std::fs::rename(&tmp, dest)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Copy picked ROMs / `.sav` files into `Documents/roms`. A single ROM
/// starts right away; several are just added to the library.
fn import_roms(paths: &[PathBuf]) {
    let Some(dir) = FILES_DIR.get().map(|d| d.join("roms")) else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        set(&IMPORT_ERROR, "Cannot create ROM folder".into());
        return;
    }
    let (mut roms, mut saves, mut errors) = (Vec::new(), 0, Vec::new());
    for src in paths {
        let name = sanitize(&src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("game.gba".into()));
        let lower = name.to_lowercase();
        let is_save = lower.ends_with(".sav");
        if !(is_save || lower.ends_with(".gba") || lower.ends_with(".gb") || lower.ends_with(".gbc")) {
            let hint = if lower.ends_with(".zip") || lower.ends_with(".7z") { " (unzip it first)" } else { "" };
            errors.push(format!("\"{name}\" is not a .gba, .gb or .gbc ROM or a .sav file{hint}"));
            continue;
        }
        let dest = dir.join(&name);
        match copy_capped(src, &dest, MAX_ROM_BYTES) {
            Ok(()) if is_save => saves += 1,
            Ok(()) => roms.push(dest),
            Err(e) => errors.push(format!("Import of \"{name}\" failed: {e}")),
        }
        // The picker's copy lives in our tmp/Inbox; don't keep it around.
        let _ = std::fs::remove_file(src);
    }
    if roms.len() == 1 && saves == 0 && errors.is_empty() {
        set(&IMPORTED_ROM, roms[0].to_string_lossy().to_string());
        return;
    }
    let mut msg = Vec::new();
    if !roms.is_empty() || saves > 0 {
        msg.push(format!("Imported {} game(s) and {} save(s).", roms.len(), saves));
        // Nothing to auto-start; the library rescans when this is shown.
        set(&IMPORTED_ROM, String::new());
    }
    msg.extend(errors);
    set(&IMPORT_ERROR, msg.join("\n"));
}

/// Copy a picked skin pack to a temporary file; Rust checks and installs it.
fn import_skin(src: &Path) {
    let name = sanitize(&src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("skin.zip".into()));
    if !name.to_lowercase().ends_with(".zip") {
        set(&IMPORT_ERROR, format!("\"{name}\" is not a .zip skin"));
        return;
    }
    let dest = std::env::temp_dir().join(&name);
    match copy_capped(src, &dest, MAX_SKIN_BYTES) {
        Ok(()) => set(&IMPORTED_SKIN, dest.to_string_lossy().to_string()),
        Err(e) => set(&IMPORT_ERROR, format!("Skin import failed: {e}")),
    }
    let _ = std::fs::remove_file(src);
}

pub fn take_imported_skin() -> Option<String> {
    take(&IMPORTED_SKIN)
}

/// Path of a ROM the user just imported. An empty string means "several
/// files were added; refresh the library but don't start anything".
pub fn take_imported_rom() -> Option<String> {
    take(&IMPORTED_ROM)
}

pub fn take_import_error() -> Option<String> {
    take(&IMPORT_ERROR)
}

/// iPads run at the display's own rate; nothing to request.
pub fn set_game_refresh_rate(_on: bool) {}

/// iPads have no Taptic Engine for button clicks.
pub fn vibrate() {}

/// Rotation is left to iPadOS (all orientations are allowed).
pub fn set_orientation(_value: i32) {}

/// Start or stop CoreMotion for tilt controls.
pub fn set_tilt_sensor(on: bool) {
    MOTION.with(|m| {
        let mut m = m.borrow_mut();
        if on {
            let mgr = m.get_or_insert_with(|| unsafe { CMMotionManager::new() });
            unsafe {
                mgr.setDeviceMotionUpdateInterval(1.0 / 60.0);
                mgr.startDeviceMotionUpdates();
            }
        } else if let Some(mgr) = m.as_ref() {
            unsafe { mgr.stopDeviceMotionUpdates() };
        }
    });
}

/// Latest gravity reading in Android's convention (m/s², device axes, +z
/// out of the screen when lying flat) and the display rotation (0..=3).
pub fn tilt() -> Option<([f32; 3], i32)> {
    let g = MOTION.with(|m| {
        let m = m.borrow();
        let motion = unsafe { m.as_ref()?.deviceMotion()? };
        Some(unsafe { motion.gravity() })
    })?;
    // CoreMotion reports gravity in g, pointing *down*; Android's gravity
    // sensor reports the upward reaction in m/s².
    const G: f64 = -9.81;
    let v = [(g.x * G) as f32, (g.y * G) as f32, (g.z * G) as f32];
    Some((v, rotation()))
}

/// The interface orientation as Android's `Display.getRotation()` value.
fn rotation() -> i32 {
    let Some(mtm) = MainThreadMarker::new() else { return 0 };
    let Some(scene) = key_window(mtm).and_then(|w| w.windowScene()) else { return 0 };
    #[allow(deprecated)]
    match scene.interfaceOrientation() {
        UIInterfaceOrientation::LandscapeRight => 1,
        UIInterfaceOrientation::PortraitUpsideDown => 2,
        UIInterfaceOrientation::LandscapeLeft => 3,
        _ => 0,
    }
}

/// Screen areas covered by the status bar, home indicator or camera, in
/// physical pixels.
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
    let Some(mtm) = MainThreadMarker::new() else { return Insets::default() };
    let Some(window) = key_window(mtm) else { return Insets::default() };
    let i = window.safeAreaInsets();
    let scale = window.contentScaleFactor() as f32;
    Insets {
        left: i.left as f32 * scale,
        top: i.top as f32 * scale,
        right: i.right as f32 * scale,
        bottom: i.bottom as f32 * scale,
    }
}
