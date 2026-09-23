//! Bluetooth / USB controller input, fed by the patched winit gamepad hook.

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use winit::platform::android::gamepad::{set_gamepad_hook, GamepadEvent};

use crate::keymap::{map_axes, map_keycode, KEYCODE_BACK};
use crate::touch::Buttons;

/// Buttons held via controller keys.
static KEY_BITS: AtomicU16 = AtomicU16::new(0);
/// Directions currently held via the hat / left stick.
static AXIS_BITS: AtomicU16 = AtomicU16::new(0);
/// Set when Select+Start (or the controller's Mode/Home key) asks for the menu.
static MENU_REQUEST: AtomicBool = AtomicBool::new(false);
/// Set when the system Back key is released.
static BACK_REQUEST: AtomicBool = AtomicBool::new(false);
/// Milliseconds timestamp of the last controller event.
static LAST_USED_MS: AtomicU64 = AtomicU64::new(0);

/// Hide the touch overlay for this long after the last controller input.
const OVERLAY_HIDE_MS: u64 = 10_000;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn install() {
    set_gamepad_hook(|event| {
        // The phone's Back key (or a controller's B-as-back) opens the menu.
        if let GamepadEvent::Key { keycode: KEYCODE_BACK, pressed } = event {
            if !pressed {
                BACK_REQUEST.store(true, Ordering::Relaxed);
            }
            return;
        }
        LAST_USED_MS.store(now_ms(), Ordering::Relaxed);
        match event {
            GamepadEvent::Key { keycode, pressed } => {
                let bit = map_keycode(keycode);
                if bit == Buttons::MENU {
                    if pressed {
                        MENU_REQUEST.store(true, Ordering::Relaxed);
                    }
                    return;
                }
                let bits = if pressed {
                    KEY_BITS.fetch_or(bit, Ordering::Relaxed) | bit
                } else {
                    KEY_BITS.fetch_and(!bit, Ordering::Relaxed) & !bit
                };
                let combo = Buttons::SELECT | Buttons::START;
                if pressed && bits & combo == combo {
                    MENU_REQUEST.store(true, Ordering::Relaxed);
                }
            }
            GamepadEvent::Axes { hat_x, hat_y, x, y } => {
                AXIS_BITS.store(map_axes(hat_x, hat_y, x, y), Ordering::Relaxed);
            }
        }
    });
}

pub fn buttons() -> Buttons {
    let bits = KEY_BITS.load(Ordering::Relaxed) | AXIS_BITS.load(Ordering::Relaxed);
    // While Select+Start is the menu chord, don't also send it to the game.
    let combo = Buttons::SELECT | Buttons::START;
    Buttons(if bits & combo == combo { bits & !combo } else { bits })
}

pub fn take_menu_request() -> bool {
    MENU_REQUEST.swap(false, Ordering::Relaxed)
}

pub fn take_back_request() -> bool {
    BACK_REQUEST.swap(false, Ordering::Relaxed)
}

/// True if a controller was used recently, so the touch overlay can hide.
pub fn recently_used() -> bool {
    let last = LAST_USED_MS.load(Ordering::Relaxed);
    last != 0 && now_ms().saturating_sub(last) < OVERLAY_HIDE_MS
}
