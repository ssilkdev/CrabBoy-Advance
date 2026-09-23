//! GBA Keypad Input Handling (KEYINPUT / KEYCNT)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    A = 0,
    B = 1,
    Select = 2,
    Start = 3,
    Right = 4,
    Left = 5,
    Up = 6,
    Down = 7,
    R = 8,
    L = 9,
}

pub struct Keypad {
    /// 10-bit state (0 = pressed, 1 = released)
    pub keyinput: u16,
    pub keycnt: u16,
}

impl Default for Keypad {
    fn default() -> Self {
        Self::new()
    }
}

impl Keypad {
    pub fn new() -> Self {
        Self {
            keyinput: 0x03FF, // All buttons released initially
            keycnt: 0,
        }
    }

    pub fn set_key_state(&mut self, key: Key, pressed: bool) {
        let bit = 1 << (key as u16);
        if pressed {
            self.keyinput &= !bit;
        } else {
            self.keyinput |= bit;
        }
    }

    pub fn read_keyinput(&self) -> u16 {
        self.keyinput
    }

    pub fn is_key_pressed(&self, key: Key) -> bool {
        (self.keyinput & (1 << (key as u16))) == 0
    }

    /// Evaluates KEYCNT against the current key state.
    /// Bit 14 of KEYCNT enables the IRQ; bit 15 selects AND (all selected
    /// keys must be pressed) vs OR (any selected key pressed) condition mode.
    pub fn check_irq(&self) -> bool {
        if (self.keycnt & 0x4000) == 0 {
            return false;
        }
        let selected = self.keycnt & 0x03FF;
        if selected == 0 {
            return false;
        }
        let pressed = (!self.keyinput) & 0x03FF;
        if (self.keycnt & 0x8000) != 0 {
            (pressed & selected) == selected
        } else {
            (pressed & selected) != 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irq_disabled_by_default() {
        let kp = Keypad::new();
        assert!(!kp.check_irq());
    }

    #[test]
    fn or_mode_fires_on_any_selected_key() {
        let mut kp = Keypad::new();
        kp.keycnt = 0x4000 | (1 << Key::A as u16) | (1 << Key::B as u16); // OR mode
        assert!(!kp.check_irq());
        kp.set_key_state(Key::A, true);
        assert!(kp.check_irq());
    }

    #[test]
    fn and_mode_requires_all_selected_keys() {
        let mut kp = Keypad::new();
        kp.keycnt = 0x4000 | 0x8000 | (1 << Key::A as u16) | (1 << Key::B as u16); // AND mode
        kp.set_key_state(Key::A, true);
        assert!(!kp.check_irq(), "only one of two required keys pressed");
        kp.set_key_state(Key::B, true);
        assert!(kp.check_irq(), "both required keys pressed");
    }
}

/// Keypad registers (ROADMAP M2). KEYINPUT is saved so the restored frame
/// sees the same buttons; the front-end overwrites it on the next input poll.
impl crate::gba::state::Snapshot for Keypad {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        w.u16(self.keyinput); w.u16(self.keycnt);
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        self.keyinput = r.u16()?; self.keycnt = r.u16()?;
        Some(())
    }
}
