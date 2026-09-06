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
}
