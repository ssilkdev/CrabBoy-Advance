//! GBA Cartridge EEPROM Backup Memory (4Kbit/512B or 64Kbit/8KB)
//!
//! Real GBA EEPROM is a bit-serial device driven exclusively via DMA: each
//! transferred 16-bit word carries exactly one protocol bit in bit 0. Games
//! always DMA the *exact* number of halfwords their chip's protocol needs
//! (73 or 81 for a write request, 9 or 17 for a read-address request, 68 for
//! a read reply), so rather than emulate the bit-serial shift register one
//! DMA halfword at a time, this module is driven directly by the DMA engine
//! (`Mmu::execute_dma_channel`), which already knows the whole transfer's
//! word count up front. That lets us decode/encode a whole logical
//! request/reply in one shot instead of maintaining per-bit state across many
//! calls -- functionally equivalent to the real protocol (which is itself
//! defined in terms of these exact fixed-length DMA transfers), and far
//! simpler to get right.
//!
//! Address width (and thus chip size) isn't distinguishable from the
//! "EEPROM_V" ROM marker alone, so it's inferred from the very first
//! request's bit count, matching how real hardware -- and every other GBA
//! emulator -- determines it in practice.

use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EepromSize {
    Unknown,
    Bit4K,  // 512 bytes, 64 x 8-byte blocks, 6-bit address
    Bit64K, // 8192 bytes, 1024 x 8-byte blocks, 14-bit address (only low 10 bits decoded)
}

impl EepromSize {
    fn data_len(self) -> usize {
        match self {
            EepromSize::Unknown => 8192, // default to the larger chip until observed
            EepromSize::Bit4K => 512,
            EepromSize::Bit64K => 8192,
        }
    }

    fn addr_bits(self) -> u32 {
        match self {
            EepromSize::Unknown => 14,
            EepromSize::Bit4K => 6,
            EepromSize::Bit64K => 14,
        }
    }

    fn from_request_bit_count(bits: usize) -> Option<(EepromSize, bool)> {
        // (size, is_write)
        match bits {
            9 => Some((EepromSize::Bit4K, false)),
            73 => Some((EepromSize::Bit4K, true)),
            17 => Some((EepromSize::Bit64K, false)),
            81 => Some((EepromSize::Bit64K, true)),
            _ => None,
        }
    }
}

pub struct Eeprom {
    pub size: EepromSize,
    pub data: Vec<u8>,
    pub dirty: bool,
    pending_read_block: Option<usize>,
    save_path: Option<PathBuf>,
}

impl Eeprom {
    pub fn new(save_path: Option<PathBuf>) -> Self {
        let size = EepromSize::Unknown;
        let mut data = vec![0xFFu8; size.data_len()];

        if let Some(ref path) = save_path {
            if path.exists() {
                if let Ok(file_data) = fs::read(path) {
                    let len = file_data.len().min(data.len());
                    data[..len].copy_from_slice(&file_data[..len]);
                    log::info!("Loaded {} bytes EEPROM save file from {:?}", len, path);
                }
            }
        }

        Self {
            size,
            data,
            dirty: false,
            pending_read_block: None,
            save_path,
        }
    }

    fn ensure_size(&mut self, size: EepromSize) {
        if self.size == size {
            return;
        }
        // First real access reveals the true chip size; resize (preserving
        // any already-loaded save bytes) rather than truncating data.
        let new_len = size.data_len();
        if self.data.len() != new_len {
            self.data.resize(new_len, 0xFF);
        }
        self.size = size;
    }

    /// Handles a whole write-direction transfer into the EEPROM (a DMA whose
    /// destination lands in the EEPROM window): either a write request
    /// (address + 64 data bits) or a read-address request (address only).
    /// `bits` is the sequence of protocol bits in transmission order (MSB
    /// first), one per transferred halfword's bit 0.
    pub fn handle_write_request(&mut self, bits: &[bool]) {
        let Some((size, is_write)) = EepromSize::from_request_bit_count(bits.len()) else {
            log::warn!("EEPROM: unrecognized request length {} bits", bits.len());
            return;
        };
        self.ensure_size(size);

        let mut pos = 0usize;
        let mut take = |n: u32| -> u32 {
            let mut v = 0u32;
            for _ in 0..n {
                v = (v << 1) | (bits.get(pos).copied().unwrap_or(false) as u32);
                pos += 1;
            }
            v
        };

        let command = take(2); // 11 = read, 10 = write
        let addr_bits = size.addr_bits();
        let address = take(addr_bits) as usize;
        let block_count = size.data_len() / 8;
        let block = address % block_count.max(1);

        if is_write && command == 0b10 {
            let mut value = [0u8; 8];
            for byte in value.iter_mut() {
                *byte = take(8) as u8;
            }
            // Data is stored MSB-first (byte 0 = most significant byte).
            let base = block * 8;
            if base + 8 <= self.data.len() {
                if self.data[base..base + 8] != value {
                    self.data[base..base + 8].copy_from_slice(&value);
                    self.dirty = true;
                }
            }
        } else if !is_write && command == 0b11 {
            self.pending_read_block = Some(block);
        } else {
            log::warn!("EEPROM: malformed request (command={:02b}, is_write={})", command, is_write);
        }
    }

    /// Handles a whole read-direction transfer out of the EEPROM (a DMA whose
    /// *source* lands in the EEPROM window): the 68-bit reply to a prior
    /// read-address request (4 dummy bits + 64 data bits, MSB first).
    pub fn handle_read_reply(&self, out_bits: &mut [bool]) {
        if out_bits.len() != 68 {
            log::warn!("EEPROM: unexpected read-reply length {} bits", out_bits.len());
            for b in out_bits.iter_mut() {
                *b = false;
            }
            return;
        }
        for b in out_bits.iter_mut().take(4) {
            *b = false; // dummy bits
        }
        let block = self.pending_read_block.unwrap_or(0);
        let base = block * 8;
        let bytes: [u8; 8] = if base + 8 <= self.data.len() {
            self.data[base..base + 8].try_into().unwrap()
        } else {
            [0xFF; 8]
        };
        for (i, bit) in out_bits[4..].iter_mut().enumerate() {
            let byte = bytes[i / 8];
            let bit_idx = 7 - (i % 8);
            *bit = ((byte >> bit_idx) & 1) != 0;
        }
    }

    pub fn sync_to_disk(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(ref path) = self.save_path {
            if let Ok(()) = fs::write(path, &self.data[..]) {
                self.dirty = false;
                log::info!("Flushed EEPROM save data to {:?}", path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_to_bits(val: u32, n: u32) -> Vec<bool> {
        (0..n).rev().map(|i| ((val >> i) & 1) != 0).collect()
    }

    #[test]
    fn write_then_read_roundtrip_4kbit() {
        let mut ee = Eeprom::new(None);

        // Write request: 10 (write) + 6-bit address(=5) + 64-bit data, total 2+6+64=72... plus stop bit = 73
        let mut bits = Vec::new();
        bits.extend(u32_to_bits(0b10, 2));
        bits.extend(u32_to_bits(5, 6));
        let value: [u8; 8] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        for b in value {
            bits.extend(u32_to_bits(b as u32, 8));
        }
        bits.push(false); // stop bit (ignored by handle_write_request's bit budget)
        ee.handle_write_request(&bits);
        assert_eq!(ee.size, EepromSize::Bit4K);
        assert_eq!(&ee.data[5 * 8..5 * 8 + 8], &value);

        // Read-address request: 11 (read) + 6-bit address(=5) + stop = 9 bits
        let mut req = Vec::new();
        req.extend(u32_to_bits(0b11, 2));
        req.extend(u32_to_bits(5, 6));
        req.push(false);
        ee.handle_write_request(&req);

        // Read reply: 4 dummy + 64 data bits
        let mut reply = vec![false; 68];
        ee.handle_read_reply(&mut reply);
        assert!(reply[..4].iter().all(|b| !b));
        for (i, &expected_byte) in value.iter().enumerate() {
            for bit in 0..8 {
                let idx = 4 + i * 8 + bit;
                let expected_bit = ((expected_byte >> (7 - bit)) & 1) != 0;
                assert_eq!(reply[idx], expected_bit, "byte {} bit {}", i, bit);
            }
        }
    }

    #[test]
    fn write_then_read_roundtrip_64kbit() {
        let mut ee = Eeprom::new(None);
        let mut bits = Vec::new();
        bits.extend(u32_to_bits(0b10, 2));
        bits.extend(u32_to_bits(100, 14));
        let value: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        for b in value {
            bits.extend(u32_to_bits(b as u32, 8));
        }
        bits.push(false);
        assert_eq!(bits.len(), 81);
        ee.handle_write_request(&bits);
        assert_eq!(ee.size, EepromSize::Bit64K);
        assert_eq!(&ee.data[100 * 8..100 * 8 + 8], &value);
    }
}
