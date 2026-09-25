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

        let title = String::from_utf8_lossy(&bytes[0x00..0x0C])
            .trim_matches('\0')
            .trim()
            .to_string();
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

pub struct NdsCard {
    pub rom: Vec<u8>,
    pub header: NdsHeader,
    pub romctrl: u32,
    pub auxspicnt: u16,
    pub auxspidata: u8,
    pub cmd_buffer: [u8; 8],
    pub cmd_len: usize,
    pub transfer_count: u32,
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
        }
    }

    pub fn load_from_file(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Failed to read NDS ROM: {e}"))?;
        self.load_from_bytes(bytes)
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

    pub fn read_card_data(&self, offset: u32) -> u32 {
        let o = offset as usize;
        if o + 4 <= self.rom.len() {
            u32::from_le_bytes([self.rom[o], self.rom[o + 1], self.rom[o + 2], self.rom[o + 3]])
        } else {
            0
        }
    }
}

impl Default for NdsCard {
    fn default() -> Self {
        Self::new()
    }
}
