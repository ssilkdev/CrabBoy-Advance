//! mGBA-compatible debug-print registers (ROADMAP M1).
//!
//! Homebrew (and mGBA's own test suite) log text by writing to a small
//! register window at 0x04FFF600-0x04FFF7FF that only emulators implement:
//!
//! - `0x04FFF780` DEBUG_ENABLE (u16): write 0xC0DE to open; reads back
//!   0x1DEA while open. Write anything else to close.
//! - `0x04FFF600..0x04FFF700` DEBUG_STRING: a 256-byte message buffer.
//! - `0x04FFF700` DEBUG_FLAGS (u16): writing with bit 8 set emits the buffer
//!   as one message at log level `flags & 7` (0 fatal ... 4 debug).
//!
//! Real hardware has open bus here, and games never touch it, so enabling
//! it unconditionally is safe. Messages go to `log` and to `messages` so
//! tests and tools can read them.

/// First address of the debug window.
pub const BASE: u32 = 0x04FF_F600;
/// One past the last address of the debug window.
pub const END: u32 = 0x04FF_F800;

const STRING_END: u32 = 0x04FF_F700;
const FLAGS: u32 = 0x04FF_F700;
const ENABLE: u32 = 0x04FF_F780;

/// Keep at most this many messages (the oldest are dropped).
const MAX_MESSAGES: usize = 1024;

#[derive(Clone)]
pub struct DebugPort {
    enabled: bool,
    buffer: [u8; 0x100],
    /// Emitted messages, oldest first: (level, text).
    pub messages: std::collections::VecDeque<(u8, String)>,
}

impl Default for DebugPort {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugPort {
    pub fn new() -> Self {
        Self { enabled: false, buffer: [0; 0x100], messages: Default::default() }
    }

    #[inline(always)]
    pub fn contains(addr: u32) -> bool {
        (BASE..END).contains(&addr)
    }

    pub fn read8(&self, addr: u32) -> u8 {
        match addr {
            ENABLE if self.enabled => 0xEA,
            a if a == ENABLE + 1 && self.enabled => 0x1D,
            _ => 0,
        }
    }

    pub fn write8(&mut self, addr: u32, val: u8) {
        match addr {
            a if (BASE..STRING_END).contains(&a) => {
                self.buffer[(a - BASE) as usize] = val;
            }
            // Byte writes to the 16-bit registers: only the high byte of
            // FLAGS matters (bit 8 = send), and ENABLE needs a full 0xC0DE.
            a if a == FLAGS + 1 => {
                if val & 1 != 0 {
                    self.emit(0);
                }
            }
            _ => {}
        }
    }

    pub fn write16(&mut self, addr: u32, val: u16) {
        match addr & !1 {
            ENABLE => self.enabled = val == 0xC0DE,
            FLAGS => {
                if val & 0x100 != 0 {
                    self.emit((val & 7) as u8);
                }
            }
            a if (BASE..STRING_END).contains(&a) => {
                let off = (a - BASE) as usize;
                self.buffer[off] = val as u8;
                self.buffer[off + 1] = (val >> 8) as u8;
            }
            _ => {}
        }
    }

    fn emit(&mut self, level: u8) {
        if !self.enabled {
            return;
        }
        let len = self.buffer.iter().position(|&b| b == 0).unwrap_or(self.buffer.len());
        let text = String::from_utf8_lossy(&self.buffer[..len]).into_owned();
        self.buffer = [0; 0x100];
        match level {
            0 | 1 => log::error!(target: "gba_debug", "{text}"),
            2 => log::warn!(target: "gba_debug", "{text}"),
            3 => log::info!(target: "gba_debug", "{text}"),
            _ => log::debug!(target: "gba_debug", "{text}"),
        }
        if self.messages.len() == MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back((level, text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_and_print() {
        let mut d = DebugPort::new();
        assert_eq!(d.read8(ENABLE), 0);
        d.write16(ENABLE, 0xC0DE);
        assert_eq!(d.read8(ENABLE) as u16 | (d.read8(ENABLE + 1) as u16) << 8, 0x1DEA);
        for (i, b) in b"END: 5/7".iter().enumerate() {
            d.write8(BASE + i as u32, *b);
        }
        d.write16(FLAGS, 0x104);
        assert_eq!(d.messages.back().unwrap(), &(4, "END: 5/7".to_string()));
    }

    #[test]
    fn closed_port_is_silent() {
        let mut d = DebugPort::new();
        d.write8(BASE, b'x');
        d.write16(FLAGS, 0x100);
        assert!(d.messages.is_empty());
    }
}
