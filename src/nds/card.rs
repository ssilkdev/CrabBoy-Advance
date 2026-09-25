//! Nintendo DS Game Card (Slot-1) & Cartridge Header

use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct NdsHeader {
    pub title: String,
    pub game_code: String,
    pub maker_code: String,
    pub unit_code: u8,
    pub device_capacity: u8,
    pub arm9_rom_offset: u32,
    pub arm9_entry_addr: u32,
    pub arm9_ram_addr: u32,
    pub arm9_size: u32,
    pub arm7_rom_offset: u32,
    pub arm7_entry_addr: u32,
    pub arm7_ram_addr: u32,
    pub arm7_size: u32,
    pub fnt_offset: u32,
    pub fnt_size: u32,
    pub fat_offset: u32,
    pub fat_size: u32,
    pub banner_offset: u32,
    pub rom_size: u32,
}

impl NdsHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 512 {
            return Err("NDS file too small for 512-byte header".to_string());
        }

        // Homebrew often leaves code bytes here (e.g. a branch at 0x00);
        // keep printable ASCII only so window titles stay clean.
        let title: String = bytes[0x00..0x0C]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        let title = if title.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            title.trim().to_string()
        } else {
            String::new()
        };
        let game_code = String::from_utf8_lossy(&bytes[0x0C..0x10]).to_string();
        let maker_code = String::from_utf8_lossy(&bytes[0x10..0x12]).to_string();
        let unit_code = bytes[0x12];
        let device_capacity = bytes[0x14];

        let read_u32 = |offset: usize| -> u32 {
            u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
        };

        let arm9_rom_offset = read_u32(0x020);
        let arm9_entry_addr = read_u32(0x024);
        let arm9_ram_addr = read_u32(0x028);
        let arm9_size = read_u32(0x02C);

        let arm7_rom_offset = read_u32(0x030);
        let arm7_entry_addr = read_u32(0x034);
        let arm7_ram_addr = read_u32(0x038);
        let arm7_size = read_u32(0x03C);

        let fnt_offset = read_u32(0x040);
        let fnt_size = read_u32(0x044);
        let fat_offset = read_u32(0x048);
        let fat_size = read_u32(0x04C);

        let banner_offset = read_u32(0x068);
        let rom_size = read_u32(0x080);

        Ok(Self {
            title,
            game_code,
            maker_code,
            unit_code,
            device_capacity,
            arm9_rom_offset,
            arm9_entry_addr,
            arm9_ram_addr,
            arm9_size,
            arm7_rom_offset,
            arm7_entry_addr,
            arm7_ram_addr,
            arm7_size,
            fnt_offset,
            fnt_size,
            fat_offset,
            fat_size,
            banner_offset,
            rom_size,
        })
    }
}

/// Slot-1 game card: ROM command protocol (GBATEK "DS Cartridge Protocol")
/// plus the SPI backup chip on AUXSPI. The card is used as if the BIOS had
/// already done the KEY1 handshake (direct boot), so only the unencrypted
/// KEY2-mode commands are served.
pub struct NdsCard {
    pub rom: Vec<u8>,
    pub header: NdsHeader,
    pub romctrl: u32,
    pub auxspicnt: u16,
    pub auxspidata: u8,
    /// Command bytes written to 0x040001A8..0x040001AF
    pub cmd_buffer: [u8; 8],
    pub cmd_len: usize,
    /// Words still to be read from 0x04100010 in the current transfer
    pub transfer_count: u32,
    transfer_addr: u32,
    transfer_kind: Transfer,
    pub backup: Backup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Rom,
    ChipId,
    Header,
    Dummy,
}

impl NdsCard {
    pub fn new() -> Self {
        Self {
            rom: Vec::new(),
            header: NdsHeader::default(),
            romctrl: 0,
            auxspicnt: 0,
            auxspidata: 0,
            cmd_buffer: [0; 8],
            cmd_len: 0,
            transfer_count: 0,
            transfer_addr: 0,
            transfer_kind: Transfer::Dummy,
            backup: Backup::new(),
        }
    }

    pub fn load_from_file(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Failed to read NDS ROM: {e}"))?;
        self.load_from_bytes(bytes)?;
        self.backup.attach_file(path.with_extension("sav"));
        Ok(())
    }

    pub fn load_from_bytes(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let header = NdsHeader::parse(&bytes)?;
        log::info!(
            "Loaded NDS ROM: \"{}\" [{}] ARM9: 0x{:08X} ({}B) ARM7: 0x{:08X} ({}B)",
            header.title,
            header.game_code,
            header.arm9_entry_addr,
            header.arm9_size,
            header.arm7_entry_addr,
            header.arm7_size
        );
        self.header = header;
        self.rom = bytes;
        Ok(())
    }

    /// Card chip ID as returned by commands 90h/B8h: maker C2h (Macronix),
    /// then the size code (size in MB - 1, or 100h - size/256MB above 128MB).
    pub fn chip_id(&self) -> u32 {
        let mb = ((self.rom.len() + 0xF_FFFF) >> 20).max(1) as u32;
        let size = if mb <= 128 { mb - 1 } else { 0x100 - (mb >> 8) };
        0xC2 | (size & 0xFF) << 8
    }

    /// Word from the ROM image at `offset` (reads past the end give FFh).
    pub fn read_card_data(&self, offset: u32) -> u32 {
        let o = offset as usize;
        let mut b = [0xFFu8; 4];
        for (i, v) in b.iter_mut().enumerate() {
            if let Some(&x) = self.rom.get(o + i) {
                *v = x;
            }
        }
        u32::from_le_bytes(b)
    }

    /// ROMCTRL write. Returns true if a transfer with data just started.
    pub fn write_romctrl(&mut self, val: u32) -> bool {
        // Bit 23 (data ready) is read-only; bit 31 starts a transfer.
        let start = val & (1 << 31) != 0 && self.romctrl & (1 << 31) == 0;
        self.romctrl = (self.romctrl & (1 << 23)) | (val & !(1 << 23));
        if !start {
            return false;
        }
        let blocks = (self.romctrl >> 24) & 7;
        let bytes = match blocks {
            0 => 0,
            7 => 4,
            n => 0x100 << n,
        };
        self.transfer_count = bytes / 4;
        let c = &self.cmd_buffer;
        match c[0] {
            0xB7 => {
                let mut addr = u32::from_be_bytes([c[1], c[2], c[3], c[4]]);
                let len = self.rom.len().next_power_of_two().max(1) as u32;
                addr &= len - 1;
                // The secure area can't be read in KEY2 mode.
                if addr < 0x8000 {
                    addr = 0x8000 + (addr & 0x1FF);
                }
                self.transfer_addr = addr;
                self.transfer_kind = Transfer::Rom;
            }
            0x90 | 0xB8 => self.transfer_kind = Transfer::ChipId,
            0x00 => {
                self.transfer_addr = 0;
                self.transfer_kind = Transfer::Header;
            }
            _ => self.transfer_kind = Transfer::Dummy,
        }
        if self.transfer_count == 0 {
            self.romctrl &= !((1 << 31) | (1 << 23));
            return false;
        }
        self.romctrl |= 1 << 23;
        true
    }

    /// Whether a word is waiting at 0x04100010.
    pub fn data_ready(&self) -> bool {
        self.romctrl & (1 << 23) != 0
    }

    /// Read the next data word. The second value is true when this word
    /// ended the transfer (card IRQ time).
    pub fn read_data(&mut self) -> (u32, bool) {
        if self.transfer_count == 0 {
            return (0xFFFF_FFFF, false);
        }
        let word = match self.transfer_kind {
            Transfer::Rom => {
                // Reads wrap inside 4 KiB pages.
                let page = self.transfer_addr & !0xFFF;
                let w = self.read_card_data(self.transfer_addr);
                self.transfer_addr = page | (self.transfer_addr.wrapping_add(4) & 0xFFF);
                w
            }
            Transfer::ChipId => self.chip_id(),
            Transfer::Header => {
                let w = self.read_card_data(self.transfer_addr & 0xFFF);
                self.transfer_addr = self.transfer_addr.wrapping_add(4);
                w
            }
            Transfer::Dummy => 0xFFFF_FFFF,
        };
        self.transfer_count -= 1;
        let done = self.transfer_count == 0;
        if done {
            self.romctrl &= !((1 << 31) | (1 << 23));
        }
        (word, done)
    }

    /// AUXSPIDATA write: one byte to the backup chip, returns its reply.
    pub fn write_auxspidata(&mut self, val: u8) {
        if self.auxspicnt & (1 << 15) == 0 || self.auxspicnt & (1 << 13) == 0 {
            return;
        }
        self.auxspidata = self.backup.transfer(val);
        // Chip-select hold (bit 6) clear: this was the last byte.
        if self.auxspicnt & (1 << 6) == 0 {
            self.backup.deselect();
        }
    }

    pub fn write_auxspicnt(&mut self, val: u16) {
        let was_hold = self.auxspicnt & (1 << 6) != 0;
        self.auxspicnt = val & !(1 << 7); // busy never set
        if was_hold && val & (1 << 6) == 0 && val & (1 << 13) == 0 {
            self.backup.deselect();
        }
    }
}

/// SPI FLASH backup chip (512 KiB, ST M25PE40-compatible; the type used
/// by Pokemon Diamond/Pearl/Platinum/HeartGold/SoulSilver and many others).
/// Saved to `<rom>.sav` by [`Backup::flush`].
pub struct Backup {
    pub data: Vec<u8>,
    path: Option<std::path::PathBuf>,
    dirty: bool,
    cmd: u8,
    addr: u32,
    addr_bytes: u8,
    write_enable: bool,
    active: bool,
}

impl Backup {
    pub const SIZE: usize = 512 * 1024;

    pub fn new() -> Self {
        Self {
            data: vec![0xFF; Self::SIZE],
            path: None,
            dirty: false,
            cmd: 0,
            addr: 0,
            addr_bytes: 0,
            write_enable: false,
            active: false,
        }
    }

    fn attach_file(&mut self, path: std::path::PathBuf) {
        if let Ok(bytes) = std::fs::read(&path) {
            let n = bytes.len().min(self.data.len());
            self.data[..n].copy_from_slice(&bytes[..n]);
            log::info!("Loaded NDS save {} ({} bytes)", path.display(), bytes.len());
        }
        self.path = Some(path);
    }

    /// Write the save to disk if it changed.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(path) = &self.path {
            match std::fs::write(path, &self.data) {
                Ok(()) => self.dirty = false,
                Err(e) => log::warn!("NDS save write failed ({}): {e}", path.display()),
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn deselect(&mut self) {
        if self.active && matches!(self.cmd, 0x02 | 0x0A | 0xDB | 0xD8) {
            self.write_enable = false; // write/erase ends by deselect
        }
        self.active = false;
    }

    fn transfer(&mut self, val: u8) -> u8 {
        if !self.active {
            self.active = true;
            self.cmd = val;
            self.addr = 0;
            self.addr_bytes = 0;
            match val {
                0x06 => self.write_enable = true,
                0x04 => self.write_enable = false,
                _ => {}
            }
            return 0xFF;
        }
        let needs_addr = matches!(self.cmd, 0x03 | 0x0B | 0x02 | 0x0A | 0xDB | 0xD8);
        if needs_addr && self.addr_bytes < 3 {
            self.addr = (self.addr << 8) | val as u32;
            self.addr_bytes += 1;
            if self.addr_bytes == 3 {
                let a = self.addr as usize % Self::SIZE;
                match self.cmd {
                    0xDB if self.write_enable => {
                        let p = a & !0xFF;
                        self.data[p..p + 0x100].fill(0xFF);
                        self.dirty = true;
                    }
                    0xD8 if self.write_enable => {
                        let p = a & !0xFFFF;
                        self.data[p..p + 0x1_0000].fill(0xFF);
                        self.dirty = true;
                    }
                    _ => {}
                }
            }
            return 0xFF;
        }
        let a = self.addr as usize % Self::SIZE;
        match self.cmd {
            0x05 => (self.write_enable as u8) << 1, // RDSR: WEL
            0x9F => {
                // RDID: 20h 40h 13h (M25PE40)
                let id = [0x20, 0x40, 0x13];
                let v = *id.get(self.addr as usize).unwrap_or(&0xFF);
                self.addr += 1;
                v
            }
            0x03 => {
                self.addr += 1;
                self.data[a]
            }
            0x0B => {
                // Fast read: one dummy byte first.
                if self.addr_bytes == 3 {
                    self.addr_bytes = 4;
                    return 0xFF;
                }
                self.addr += 1;
                self.data[a]
            }
            0x0A | 0x02 if self.write_enable => {
                // Page write/program, wrapping inside the 256-byte page.
                self.data[a] = if self.cmd == 0x02 { self.data[a] & val } else { val };
                self.addr = (self.addr & !0xFF) | ((self.addr + 1) & 0xFF);
                self.dirty = true;
                0xFF
            }
            _ => 0xFF,
        }
    }
}

impl Default for Backup {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for NdsCard {
    fn default() -> Self {
        Self::new()
    }
}
