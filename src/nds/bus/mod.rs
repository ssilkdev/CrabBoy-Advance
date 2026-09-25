//! Nintendo DS Memory Bus Controller & Memory Mapping
//!
//! Routes memory transactions for ARM9 and ARM7 across Main RAM (4MB),
//! Shared WRAM (32KB), ARM7 WRAM (64KB), the 9 VRAM banks (A-I),
//! PPU (dual 2D engines), SPI, and IPC.

use crate::nds::card::NdsCard;
use crate::nds::ipc::Ipc;
use crate::nds::ppu::NdsPpu;
use crate::nds::spi::SpiBus;

pub struct NdsBus {
    /// 4 MB Main RAM (mirrored across 0x0200_0000..0x0300_0000)
    pub main_ram: Box<[u8; 0x400000]>,
    /// 32 KB Shared WRAM (two 16 KB blocks)
    pub shared_wram: [u8; 0x8000],
    /// 64 KB dedicated ARM7 WRAM (0x0380_0000..0x0381_0000)
    pub arm7_wram: Box<[u8; 0x10000]>,

    // VRAM Banks (656 KB total)
    pub vram_a: Box<[u8; 0x20000]>, // 128 KB
    pub vram_b: Box<[u8; 0x20000]>, // 128 KB
    pub vram_c: Box<[u8; 0x20000]>, // 128 KB
    pub vram_d: Box<[u8; 0x20000]>, // 128 KB
    pub vram_e: Box<[u8; 0x10000]>, // 64 KB
    pub vram_f: Box<[u8; 0x4000]>,  // 16 KB
    pub vram_g: Box<[u8; 0x4000]>,  // 16 KB
    pub vram_h: Box<[u8; 0x8000]>,  // 32 KB
    pub vram_i: Box<[u8; 0x4000]>,  // 16 KB

    pub vramcnt: [u8; 9],
    pub wramcnt: u8,
    pub postflg_arm9: u8,
    pub postflg_arm7: u8,

    // Input registers
    pub keyinput: u16,
    pub extkeyin: u16,

    // Interrupts (ARM9 & ARM7)
    pub ime_arm9: bool,
    pub ie_arm9: u32,
    pub if_arm9: u32,

    pub ime_arm7: bool,
    pub ie_arm7: u32,
    pub if_arm7: u32,

    // Peripherals
    pub ppu: NdsPpu,
    pub spi: SpiBus,
}

impl Default for NdsBus {
    fn default() -> Self {
        Self::new()
    }
}

impl NdsBus {
    pub fn new() -> Self {
        Self {
            main_ram: vec![0u8; 0x400000].into_boxed_slice().try_into().unwrap(),
            shared_wram: [0u8; 0x8000],
            arm7_wram: vec![0u8; 0x10000].into_boxed_slice().try_into().unwrap(),

            vram_a: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_b: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_c: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_d: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_e: vec![0u8; 0x10000].into_boxed_slice().try_into().unwrap(),
            vram_f: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),
            vram_g: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),
            vram_h: vec![0u8; 0x8000].into_boxed_slice().try_into().unwrap(),
            vram_i: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),

            vramcnt: [0; 9],
            wramcnt: 0,
            postflg_arm9: 0,
            postflg_arm7: 0,

            // Active low: all keys released (0x03FF for 10 keys)
            keyinput: 0x03FF,
            // EXTKEYIN: X, Y, pen down bits (default 0x007F)
            extkeyin: 0x007F,

            ime_arm9: false,
            ie_arm9: 0,
            if_arm9: 0,

            ime_arm7: false,
            ie_arm7: 0,
            if_arm7: 0,

            ppu: NdsPpu::new(),
            spi: SpiBus::new(),
        }
    }

    /// Read 32-bit word from ARM9 address space
    pub fn read_arm9_u32(&self, addr: u32, ipc: &Ipc, card: &NdsCard) -> u32 {
        match addr {
            // Main RAM (4MB mirrored)
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFF) as usize;
                u32::from_le_bytes([
                    self.main_ram[offset],
                    self.main_ram[offset + 1],
                    self.main_ram[offset + 2],
                    self.main_ram[offset + 3],
                ])
            }
            // Shared WRAM (if mapped to ARM9)
            0x0300_0000..=0x03FF_FFFF => {
                let offset = (addr & 0x7FFF) as usize;
                u32::from_le_bytes([
                    self.shared_wram[offset],
                    self.shared_wram[offset + 1],
                    self.shared_wram[offset + 2],
                    self.shared_wram[offset + 3],
                ])
            }
            // Engine A I/O registers
            0x0400_0000..=0x0400_006C => self.ppu.read_io_a(addr),
            // Keypad
            0x0400_0130 => self.keyinput as u32,
            // IPC
            0x0400_0180 => ipc.sync.read_arm9() as u32,
            0x0400_0184 => ipc.fifo.read_cnt_arm9() as u32,
            0x0400_0188 => 0, // Reading ARM9 data handled by FIFO
            // Interrupt Controller (ARM9)
            0x0400_0208 => self.ime_arm9 as u32,
            0x0400_0210 => self.ie_arm9,
            0x0400_0214 => self.if_arm9,
            // VRAM Bank Control
            0x0400_0240 => {
                u32::from_le_bytes([self.vramcnt[0], self.vramcnt[1], self.vramcnt[2], self.vramcnt[3]])
            }
            0x0400_0244 => {
                u32::from_le_bytes([self.vramcnt[4], self.vramcnt[5], self.vramcnt[6], self.vramcnt[7]])
            }
            0x0400_0248 => self.vramcnt[8] as u32,
            0x0400_0300 => self.postflg_arm9 as u32,
            0x0400_0304 => self.ppu.powcnt1,
            // Engine B I/O registers
            0x0400_1000..=0x0400_106C => self.ppu.read_io_b(addr),
            // Palette RAM
            0x0500_0000..=0x0500_03FF => {
                let offset = (addr & 0x3FC) as usize;
                u32::from_le_bytes([
                    self.ppu.engine_a.palette[offset],
                    self.ppu.engine_a.palette[offset + 1],
                    self.ppu.engine_a.palette[offset + 2],
                    self.ppu.engine_a.palette[offset + 3],
                ])
            }
            0x0500_0400..=0x0500_07FF => {
                let offset = (addr & 0x3FC) as usize;
                u32::from_le_bytes([
                    self.ppu.engine_b.palette[offset],
                    self.ppu.engine_b.palette[offset + 1],
                    self.ppu.engine_b.palette[offset + 2],
                    self.ppu.engine_b.palette[offset + 3],
                ])
            }
            // VRAM Read (VRAM Bank A default)
            0x0600_0000..=0x061F_FFFF => {
                let offset = (addr & 0x1_FFFC) as usize;
                u32::from_le_bytes([
                    self.vram_a[offset],
                    self.vram_a[offset + 1],
                    self.vram_a[offset + 2],
                    self.vram_a[offset + 3],
                ])
            }
            // OAM
            0x0700_0000..=0x0700_03FF => {
                let offset = (addr & 0x3FC) as usize;
                u32::from_le_bytes([
                    self.ppu.engine_a.oam[offset],
                    self.ppu.engine_a.oam[offset + 1],
                    self.ppu.engine_a.oam[offset + 2],
                    self.ppu.engine_a.oam[offset + 3],
                ])
            }
            0x0700_0400..=0x0700_07FF => {
                let offset = (addr & 0x3FC) as usize;
                u32::from_le_bytes([
                    self.ppu.engine_b.oam[offset],
                    self.ppu.engine_b.oam[offset + 1],
                    self.ppu.engine_b.oam[offset + 2],
                    self.ppu.engine_b.oam[offset + 3],
                ])
            }
            // Game Card
            0x0410_0000 => card.read_card_data(0),
            0x0410_0010 => card.romctrl,
            _ => 0,
        }
    }

    /// Write 32-bit word from ARM9 address space
    pub fn write_arm9_u32(&mut self, addr: u32, val: u32, ipc: &mut Ipc, card: &mut NdsCard) {
        match addr {
            // Main RAM
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFF) as usize;
                let bytes = val.to_le_bytes();
                self.main_ram[offset..offset + 4].copy_from_slice(&bytes);
            }
            // Shared WRAM
            0x0300_0000..=0x03FF_FFFF => {
                let offset = (addr & 0x7FFF) as usize;
                let bytes = val.to_le_bytes();
                self.shared_wram[offset..offset + 4].copy_from_slice(&bytes);
            }
            // Engine A I/O
            0x0400_0000..=0x0400_006C => self.ppu.write_io_a(addr, val),
            // IPC
            0x0400_0180 => ipc.sync.write_arm9(val as u16),
            0x0400_0184 => ipc.fifo.write_cnt_arm9(val as u16),
            0x0400_0188 => ipc.fifo.write_data_arm9(val),
            // Interrupt Controller (ARM9)
            0x0400_0208 => self.ime_arm9 = (val & 1) != 0,
            0x0400_0210 => self.ie_arm9 = val,
            0x0400_0214 => self.if_arm9 &= !val, // Write 1 to clear
            // VRAM Bank Control
            0x0400_0240 => {
                let bytes = val.to_le_bytes();
                self.vramcnt[0..4].copy_from_slice(&bytes);
            }
            0x0400_0244 => {
                let bytes = val.to_le_bytes();
                self.vramcnt[4..8].copy_from_slice(&bytes);
            }
            0x0400_0248 => self.vramcnt[8] = val as u8,
            0x0400_0300 => self.postflg_arm9 = val as u8,
            0x0400_0304 => self.ppu.powcnt1 = val,
            // Engine B I/O
            0x0400_1000..=0x0400_106C => self.ppu.write_io_b(addr, val),
            // Palette RAM
            0x0500_0000..=0x0500_03FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_a.palette[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0500_0400..=0x0500_07FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_b.palette[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            // VRAM Write
            0x0600_0000..=0x061F_FFFF => {
                let offset = (addr & 0x1_FFFC) as usize;
                self.vram_a[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            // OAM
            0x0700_0000..=0x0700_03FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_a.oam[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0700_0400..=0x0700_07FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_b.oam[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            // Game Card
            0x0410_0010 => card.romctrl = val,
            _ => {}
        }
    }

    /// Read 32-bit word from ARM7 address space
    pub fn read_arm7_u32(&self, addr: u32, ipc: &Ipc) -> u32 {
        match addr {
            // Main RAM
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFF) as usize;
                u32::from_le_bytes([
                    self.main_ram[offset],
                    self.main_ram[offset + 1],
                    self.main_ram[offset + 2],
                    self.main_ram[offset + 3],
                ])
            }
            // ARM7 dedicated WRAM (64KB)
            0x0380_0000..=0x0380_FFFF => {
                let offset = (addr & 0xFFFF) as usize;
                u32::from_le_bytes([
                    self.arm7_wram[offset],
                    self.arm7_wram[offset + 1],
                    self.arm7_wram[offset + 2],
                    self.arm7_wram[offset + 3],
                ])
            }
            // Keypad & ExtKey (X, Y, Pen Down)
            0x0400_0130 => (self.keyinput as u32) | ((self.extkeyin as u32) << 16),
            0x0400_0136 => self.extkeyin as u32,
            // IPC
            0x0400_0180 => ipc.sync.read_arm7() as u32,
            0x0400_0184 => ipc.fifo.read_cnt_arm7() as u32,
            0x0400_0188 => 0,
            // SPI Bus
            0x0400_01C0 => (self.spi.spicnt as u32) | ((self.spi.spidata as u32) << 16),
            0x0400_01C2 => self.spi.spidata as u32,
            // Interrupt Controller (ARM7)
            0x0400_0208 => self.ime_arm7 as u32,
            0x0400_0210 => self.ie_arm7,
            0x0400_0214 => self.if_arm7,
            0x0400_0300 => self.postflg_arm7 as u32,
            _ => 0,
        }
    }

    /// Write 32-bit word from ARM7 address space
    pub fn write_arm7_u32(&mut self, addr: u32, val: u32, ipc: &mut Ipc) {
        match addr {
            // Main RAM
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFF) as usize;
                let bytes = val.to_le_bytes();
                self.main_ram[offset..offset + 4].copy_from_slice(&bytes);
            }
            // ARM7 dedicated WRAM
            0x0380_0000..=0x0380_FFFF => {
                let offset = (addr & 0xFFFF) as usize;
                let bytes = val.to_le_bytes();
                self.arm7_wram[offset..offset + 4].copy_from_slice(&bytes);
            }
            // IPC
            0x0400_0180 => ipc.sync.write_arm7(val as u16),
            0x0400_0184 => ipc.fifo.write_cnt_arm7(val as u16),
            0x0400_0188 => ipc.fifo.write_data_arm7(val),
            // SPI Bus
            0x0400_01C0 => {
                self.spi.write_cnt(val as u16);
                let data = (val >> 16) as u8;
                let resp = self.spi.transfer_byte(data);
                self.spi.spidata = resp as u16;
            }
            0x0400_01C2 => {
                let resp = self.spi.transfer_byte(val as u8);
                self.spi.spidata = resp as u16;
            }
            // Interrupt Controller (ARM7)
            0x0400_0208 => self.ime_arm7 = (val & 1) != 0,
            0x0400_0210 => self.ie_arm7 = val,
            0x0400_0214 => self.if_arm7 &= !val,
            0x0400_0300 => self.postflg_arm7 = val as u8,
            _ => {}
        }
    }
}
