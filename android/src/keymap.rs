//! Controller key / axis -> GBA button mapping (pure; host-testable).

use crate::touch::Buttons;

const STICK_DEADZONE: f32 = 0.45;

// android.view.KeyEvent key codes.
pub const KEYCODE_BACK: u32 = 4;
const KEYCODE_DPAD_UP: u32 = 19;
const KEYCODE_DPAD_DOWN: u32 = 20;
const KEYCODE_DPAD_LEFT: u32 = 21;
const KEYCODE_DPAD_RIGHT: u32 = 22;
const KEYCODE_BUTTON_A: u32 = 96;
const KEYCODE_BUTTON_B: u32 = 97;
const KEYCODE_BUTTON_X: u32 = 99;
const KEYCODE_BUTTON_Y: u32 = 100;
const KEYCODE_BUTTON_L1: u32 = 102;
const KEYCODE_BUTTON_R1: u32 = 103;
const KEYCODE_BUTTON_L2: u32 = 104;
const KEYCODE_BUTTON_R2: u32 = 105;
const KEYCODE_BUTTON_START: u32 = 108;
const KEYCODE_BUTTON_SELECT: u32 = 109;
const KEYCODE_BUTTON_MODE: u32 = 110;

/// Map an Android key code to GBA buttons.
///
/// Android reports a controller's *bottom* face button as `BUTTON_A` and the
/// *right* one as `BUTTON_B` (Xbox layout) regardless of what is printed on
/// it. The GBA has A on the right and B on the left, so the right button
/// (`B`) drives GBA A and the bottom one (`A`) drives GBA B, which puts both
/// in the physical spots a GBA player expects. X/Y double as B/A for
/// controllers whose face buttons are laid out differently.
pub fn map_keycode(keycode: u32) -> u16 {
    match keycode {
        KEYCODE_BUTTON_B | KEYCODE_BUTTON_Y => Buttons::A,
        KEYCODE_BUTTON_A | KEYCODE_BUTTON_X => Buttons::B,
        KEYCODE_BUTTON_L1 | KEYCODE_BUTTON_L2 => Buttons::L,
        KEYCODE_BUTTON_R1 | KEYCODE_BUTTON_R2 => Buttons::R,
        KEYCODE_BUTTON_START => Buttons::START,
        KEYCODE_BUTTON_SELECT => Buttons::SELECT,
        KEYCODE_DPAD_UP => Buttons::UP,
        KEYCODE_DPAD_DOWN => Buttons::DOWN,
        KEYCODE_DPAD_LEFT => Buttons::LEFT,
        KEYCODE_DPAD_RIGHT => Buttons::RIGHT,
        KEYCODE_BUTTON_MODE => Buttons::MENU,
        _ => 0,
    }
}

/// Directions from the hat (digital D-pad) and left stick.
pub fn map_axes(hat_x: f32, hat_y: f32, x: f32, y: f32) -> u16 {
    let mut bits = 0;
    if hat_x < -0.5 || x < -STICK_DEADZONE {
        bits |= Buttons::LEFT;
    }
    if hat_x > 0.5 || x > STICK_DEADZONE {
        bits |= Buttons::RIGHT;
    }
    if hat_y < -0.5 || y < -STICK_DEADZONE {
        bits |= Buttons::UP;
    }
    if hat_y > 0.5 || y > STICK_DEADZONE {
        bits |= Buttons::DOWN;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_buttons_follow_gba_positions() {
        // Right face button (Android BUTTON_B) is GBA A; bottom is GBA B.
        assert_eq!(map_keycode(KEYCODE_BUTTON_B), Buttons::A);
        assert_eq!(map_keycode(KEYCODE_BUTTON_A), Buttons::B);
        assert_eq!(map_keycode(KEYCODE_BUTTON_L1), Buttons::L);
        assert_eq!(map_keycode(KEYCODE_BUTTON_R2), Buttons::R);
        assert_eq!(map_keycode(KEYCODE_BUTTON_START), Buttons::START);
        assert_eq!(map_keycode(KEYCODE_DPAD_LEFT), Buttons::LEFT);
        assert_eq!(map_keycode(KEYCODE_BUTTON_MODE), Buttons::MENU);
        assert_eq!(map_keycode(KEYCODE_BACK), 0);
    }

    #[test]
    fn stick_has_a_deadzone_and_diagonals() {
        assert_eq!(map_axes(0.0, 0.0, 0.3, -0.3), 0);
        assert_eq!(map_axes(0.0, 0.0, 0.9, -0.9), Buttons::RIGHT | Buttons::UP);
        assert_eq!(map_axes(-1.0, 1.0, 0.0, 0.0), Buttons::LEFT | Buttons::DOWN);
    }
}
