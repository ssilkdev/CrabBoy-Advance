//! Nintendo DS Memory Bus Controller & Memory Mapping
//!
//! Routes memory transactions for ARM9 and ARM7 across Main RAM (4MB),
//! Shared WRAM (32KB), ARM7 WRAM (64KB), the 9 VRAM banks (A-I),
//! ITCM/DTCM, PPU (dual 2D engines), SPI, Timers, DMA, and IPC.

use crate::nds::card::NdsCard;
use crate::nds::ipc::Ipc;
use crate::nds::ppu::NdsPpu;
use crate::nds::spi::SpiBus;

#[derive(Debug, Clone, Copy, Default)]
pub struct NdsTimer {
    pub counter: u16,
    pub reload: u16,
    pub control: u16,
    pub prescaler_shift: u32,
    pub count_up: bool,
    pub irq_enable: bool,
    pub enabled: bool,
    pub sub_ticks: u32,
}

impl NdsTimer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_control(&mut self, val: u16) {
        let was_enabled = self.enabled;
        self.control = val;
        let prescaler = val & 3;
        self.prescaler_shift = match prescaler {
            0 => 0,   // /1 (33.51 MHz)
            1 => 6,   // /64
            2 => 8,   // /256
            3 => 10,  // /1024
            _ => 0,
        };
        self.count_up = (val & (1 << 2)) != 0;
        self.irq_enable = (val & (1 << 6)) != 0;
        self.enabled = (val & (1 << 7)) != 0;

        if !was_enabled && self.enabled {
            self.counter = self.reload;
            self.sub_ticks = 0;
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NdsDma {
    pub sad: u32,
    pub dad: u32,
    pub cnt: u32,
}

pub struct NdsBus {
    /// 4 MB Main RAM (mirrored across 0x0200_0000..0x0300_0000)
    pub main_ram: Box<[u8; 0x400000]>,
    /// 32 KB Shared WRAM (two 16 KB blocks)
    pub shared_wram: [u8; 0x8000],
    /// 64 KB dedicated ARM7 WRAM (0x0380_0000..0x0381_0000)
    pub arm7_wram: Box<[u8; 0x10000]>,

    // ARM9 Tightly-Coupled Memories
    pub itcm: Box<[u8; 0x8000]>, // 32 KB
    pub dtcm: Box<[u8; 0x4000]>, // 16 KB
    pub cp15_control: u32,
    pub itcm_control: u32,
    pub dtcm_control: u32,

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

    // Timers & DMA
    pub timers_arm9: [NdsTimer; 4],
    pub timers_arm7: [NdsTimer; 4],
    pub dma_arm9: [NdsDma; 4],
    pub dma_arm7: [NdsDma; 4],

    // Peripherals
    pub ppu: NdsPpu,
    pub spi: SpiBus,
    pub ipc: Ipc,
    pub card: NdsCard,
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

            itcm: vec![0u8; 0x8000].into_boxed_slice().try_into().unwrap(),
            dtcm: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),
            cp15_control: 0x00050078, // Default DTCM/ITCM enabled
            itcm_control: 0x00000038, // 32KB at 0x00000000
            dtcm_control: 0x027C0030, // 16KB at 0x027C0000

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

            keyinput: 0x03FF,
            extkeyin: 0x007F,

            ime_arm9: false,
            ie_arm9: 0,
            if_arm9: 0,

            ime_arm7: false,
            ie_arm7: 0,
            if_arm7: 0,

            timers_arm9: [NdsTimer::new(); 4],
            timers_arm7: [NdsTimer::new(); 4],
            dma_arm9: [NdsDma::default(); 4],
            dma_arm7: [NdsDma::default(); 4],

            ppu: NdsPpu::new(),
            spi: SpiBus::new(),
            ipc: Ipc::default(),
            card: NdsCard::new(),
        }
    }

    #[inline]
    pub fn itcm_base(&self) -> u32 {
        self.itcm_control & 0xFFFFF000
    }

    #[inline]
    pub fn dtcm_base(&self) -> u32 {
        self.dtcm_control & 0xFFFFF000
    }

    #[inline]
    pub fn is_itcm_enabled(&self) -> bool {
        (self.cp15_control & (1 << 18)) != 0
    }

    #[inline]
    pub fn is_dtcm_enabled(&self) -> bool {
        (self.cp15_control & (1 << 16)) != 0
    }

    /// Advance timers by sys cycles (33.51 MHz clock ticks)
    pub fn step_timers(&mut self, cycles: u32, for_arm9: bool) {
        let (timers, if_reg) = if for_arm9 {
            (&mut self.timers_arm9, &mut self.if_arm9)
        } else {
            (&mut self.timers_arm7, &mut self.if_arm7)
        };

        let mut cascade = false;
        for i in 0..4 {
            let timer = &mut timers[i];
            if !timer.enabled {
                cascade = false;
                continue;
            }

            let mut overflow = false;
            if timer.count_up {
                if cascade {
                    let (next_cnt, ov) = timer.counter.overflowing_add(1);
                    if ov {
                        timer.counter = timer.reload;
                        overflow = true;
                    } else {
                        timer.counter = next_cnt;
                    }
                }
            } else {
                timer.sub_ticks += cycles;
                let step = 1 << timer.prescaler_shift;
                let ticks = timer.sub_ticks >> timer.prescaler_shift;
                timer.sub_ticks &= step - 1;

                if ticks > 0 {
                    let cur = timer.counter as u32;
                    let next = cur + ticks;
                    if next > 0xFFFF {
                        let span = 0x10000 - timer.reload as u32;
                        let remainder = if span > 0 { (next - 0x10000) % span } else { 0 };
                        timer.counter = (timer.reload as u32 + remainder) as u16;
                        overflow = true;
                    } else {
                        timer.counter = next as u16;
                    }
                }
            }

            if overflow {
                if timer.irq_enable {
                    *if_reg |= 1 << (3 + i);
                }
                cascade = true;
            } else {
                cascade = false;
            }
        }
    }

    fn check_trigger_dma(&mut self, channel: usize, for_arm9: bool) {
        let dma = if for_arm9 { self.dma_arm9[channel] } else { self.dma_arm7[channel] };
        if (dma.cnt & (1 << 31)) == 0 {
            return;
        }
        let start_mode = (dma.cnt >> 27) & 0x07;
        if start_mode != 0 {
            return; // Only immediate DMA runs synchronously
        }

        let is_32bit = (dma.cnt & (1 << 26)) != 0;
        let mut count = (dma.cnt & 0x1F_FFFF) as usize;
        if count == 0 {
            count = if is_32bit { 0x20_0000 } else { 0x10_0000 };
        }
        count = count.min(0x10_0000);

        let dad_ctrl = (dma.cnt >> 21) & 3;
        let sad_ctrl = (dma.cnt >> 23) & 3;
        let mut sad = dma.sad;
        let mut dad = dma.dad;
        let step = if is_32bit { 4 } else { 2 };

        for _ in 0..count {
            if is_32bit {
                let val = if for_arm9 { self.read_arm9_u32(sad) } else { self.read_arm7_u32(sad) };
                if for_arm9 { self.write_arm9_u32(dad, val); } else { self.write_arm7_u32(dad, val); }
            } else {
                let val = if for_arm9 { self.read_arm9_u16(sad) } else { self.read_arm7_u16(sad) };
                if for_arm9 { self.write_arm9_u16(dad, val); } else { self.write_arm7_u16(dad, val); }
            }
            match sad_ctrl {
                0 => sad = sad.wrapping_add(step),
                1 => sad = sad.wrapping_sub(step),
                _ => {}
            }
            match dad_ctrl {
                0 => dad = dad.wrapping_add(step),
                1 => dad = dad.wrapping_sub(step),
                _ => {}
            }
        }

        let repeat = (dma.cnt & (1 << 25)) != 0;
        if !repeat {
            if for_arm9 {
                self.dma_arm9[channel].cnt &= !(1 << 31);
            } else {
                self.dma_arm7[channel].cnt &= !(1 << 31);
            }
        }

        if (dma.cnt & (1 << 30)) != 0 {
            if for_arm9 {
                self.if_arm9 |= 1 << (8 + channel);
            } else {
                self.if_arm7 |= 1 << (8 + channel);
            }
        }
    }

    // ==========================================
    // ARM9 Memory Access
    // ==========================================

    pub fn read_arm9_u8(&mut self, addr: u32) -> u8 {
        if self.is_itcm_enabled() && addr >= self.itcm_base() && addr < self.itcm_base() + 0x8000 {
            return self.itcm[(addr - self.itcm_base()) as usize];
        }
        if self.is_dtcm_enabled() && addr >= self.dtcm_base() && addr < self.dtcm_base() + 0x4000 {
            return self.dtcm[(addr - self.dtcm_base()) as usize];
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize],
            0x0300_0000..=0x03FF_FFFF => self.shared_wram[(addr & 0x7FFF) as usize],
            0x0400_0130..=0x0400_0131 => ((self.keyinput >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0180..=0x0400_0181 => ((self.ipc.sync.read_arm9() >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0208 => self.ime_arm9 as u8,
            0x0400_0240..=0x0400_0248 => {
                let idx = (addr - 0x0400_0240) as usize;
                if idx < 9 { self.vramcnt[idx] } else { 0 }
            }
            0x0400_0300 => self.postflg_arm9,
            0x0500_0000..=0x0500_03FF => self.ppu.engine_a.palette[(addr & 0x3FF) as usize],
            0x0500_0400..=0x0500_07FF => self.ppu.engine_b.palette[(addr & 0x3FF) as usize],
            0x0600_0000..=0x061F_FFFF => self.vram_a[(addr & 0x1_FFFF) as usize],
            0x0680_0000..=0x0681_FFFF => self.vram_a[(addr & 0x1_FFFF) as usize],
            0x0682_0000..=0x0683_FFFF => self.vram_b[(addr & 0x1_FFFF) as usize],
            0x0684_0000..=0x0685_FFFF => self.vram_c[(addr & 0x1_FFFF) as usize],
            0x0686_0000..=0x0687_FFFF => self.vram_d[(addr & 0x1_FFFF) as usize],
            0x0688_0000..=0x0688_FFFF => self.vram_e[(addr & 0xFFFF) as usize],
            0x0689_0000..=0x0689_3FFF => self.vram_f[(addr & 0x3FFF) as usize],
            0x0689_4000..=0x0689_7FFF => self.vram_g[(addr & 0x3FFF) as usize],
            0x0689_8000..=0x0689_FFFF => self.vram_h[(addr & 0x7FFF) as usize],
            0x068A_0000..=0x068A_3FFF => self.vram_i[(addr & 0x3FFF) as usize],
            0x0700_0000..=0x0700_03FF => self.ppu.engine_a.oam[(addr & 0x3FF) as usize],
            0x0700_0400..=0x0700_07FF => self.ppu.engine_b.oam[(addr & 0x3FF) as usize],
            _ => (self.read_arm9_u32(addr & !3) >> ((addr & 3) * 8)) as u8,
        }
    }

    pub fn read_arm9_u16(&mut self, addr: u32) -> u16 {
        if self.is_itcm_enabled() && addr >= self.itcm_base() && addr + 1 < self.itcm_base() + 0x8000 {
            let off = (addr - self.itcm_base()) as usize;
            return u16::from_le_bytes([self.itcm[off], self.itcm[off + 1]]);
        }
        if self.is_dtcm_enabled() && addr >= self.dtcm_base() && addr + 1 < self.dtcm_base() + 0x4000 {
            let off = (addr - self.dtcm_base()) as usize;
            return u16::from_le_bytes([self.dtcm[off], self.dtcm[off + 1]]);
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let off = (addr & 0x3F_FFFE) as usize;
                u16::from_le_bytes([self.main_ram[off], self.main_ram[off + 1]])
            }
            0x0300_0000..=0x03FF_FFFF => {
                let off = (addr & 0x7FFE) as usize;
                u16::from_le_bytes([self.shared_wram[off], self.shared_wram[off + 1]])
            }
            0x0400_0004 => self.ppu.dispstat_a,
            0x0400_0006 => self.ppu.vcount,
            0x0400_0100 => self.timers_arm9[0].counter,
            0x0400_0102 => self.timers_arm9[0].control,
            0x0400_0104 => self.timers_arm9[1].counter,
            0x0400_0106 => self.timers_arm9[1].control,
            0x0400_0108 => self.timers_arm9[2].counter,
            0x0400_010A => self.timers_arm9[2].control,
            0x0400_010C => self.timers_arm9[3].counter,
            0x0400_010E => self.timers_arm9[3].control,
            0x0400_0130 => self.keyinput,
            0x0400_0180 => self.ipc.sync.read_arm9(),
            0x0400_0184 => self.ipc.fifo.read_cnt_arm9(),
            0x0400_0208 => self.ime_arm9 as u16,
            0x0400_0210 => self.ie_arm9 as u16,
            0x0400_0212 => (self.ie_arm9 >> 16) as u16,
            0x0400_0214 => self.if_arm9 as u16,
            0x0400_0216 => (self.if_arm9 >> 16) as u16,
            0x0400_1004 => self.ppu.dispstat_b,
            0x0400_1006 => self.ppu.vcount,
            0x0500_0000..=0x0500_03FF => {
                let off = (addr & 0x3FE) as usize;
                u16::from_le_bytes([self.ppu.engine_a.palette[off], self.ppu.engine_a.palette[off + 1]])
            }
            0x0500_0400..=0x0500_07FF => {
                let off = (addr & 0x3FE) as usize;
                u16::from_le_bytes([self.ppu.engine_b.palette[off], self.ppu.engine_b.palette[off + 1]])
            }
            0x0600_0000..=0x061F_FFFF => {
                let off = (addr & 0x1_FFFE) as usize;
                u16::from_le_bytes([self.vram_a[off], self.vram_a[off + 1]])
            }
            0x0700_0000..=0x0700_03FF => {
                let off = (addr & 0x3FE) as usize;
                u16::from_le_bytes([self.ppu.engine_a.oam[off], self.ppu.engine_a.oam[off + 1]])
            }
            0x0700_0400..=0x0700_07FF => {
                let off = (addr & 0x3FE) as usize;
                u16::from_le_bytes([self.ppu.engine_b.oam[off], self.ppu.engine_b.oam[off + 1]])
            }
            _ => (self.read_arm9_u32(addr & !3) >> ((addr & 2) * 8)) as u16,
        }
    }

    pub fn read_arm9_u32(&mut self, addr: u32) -> u32 {
        if self.is_itcm_enabled() && addr >= self.itcm_base() && addr + 3 < self.itcm_base() + 0x8000 {
            let off = (addr - self.itcm_base()) as usize;
            return u32::from_le_bytes([
                self.itcm[off],
                self.itcm[off + 1],
                self.itcm[off + 2],
                self.itcm[off + 3],
            ]);
        }
        if self.is_dtcm_enabled() && addr >= self.dtcm_base() && addr + 3 < self.dtcm_base() + 0x4000 {
            let off = (addr - self.dtcm_base()) as usize;
            return u32::from_le_bytes([
                self.dtcm[off],
                self.dtcm[off + 1],
                self.dtcm[off + 2],
                self.dtcm[off + 3],
            ]);
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFC) as usize;
                u32::from_le_bytes([
                    self.main_ram[offset],
                    self.main_ram[offset + 1],
                    self.main_ram[offset + 2],
                    self.main_ram[offset + 3],
                ])
            }
            0x0300_0000..=0x03FF_FFFF => {
                let offset = (addr & 0x7FFC) as usize;
                u32::from_le_bytes([
                    self.shared_wram[offset],
                    self.shared_wram[offset + 1],
                    self.shared_wram[offset + 2],
                    self.shared_wram[offset + 3],
                ])
            }
            0x0400_0000..=0x0400_006C | 0x0400_0320..=0x0400_06A0 => self.ppu.read_io_a(addr),
            0x0400_00B0 => self.dma_arm9[0].sad,
            0x0400_00B4 => self.dma_arm9[0].dad,
            0x0400_00B8 => self.dma_arm9[0].cnt,
            0x0400_00BC => self.dma_arm9[1].sad,
            0x0400_00C0 => self.dma_arm9[1].dad,
            0x0400_00C4 => self.dma_arm9[1].cnt,
            0x0400_00C8 => self.dma_arm9[2].sad,
            0x0400_00CC => self.dma_arm9[2].dad,
            0x0400_00D0 => self.dma_arm9[2].cnt,
            0x0400_00D4 => self.dma_arm9[3].sad,
            0x0400_00D8 => self.dma_arm9[3].dad,
            0x0400_00DC => self.dma_arm9[3].cnt,
            0x0400_0100 => (self.timers_arm9[0].counter as u32) | ((self.timers_arm9[0].control as u32) << 16),
            0x0400_0104 => (self.timers_arm9[1].counter as u32) | ((self.timers_arm9[1].control as u32) << 16),
            0x0400_0108 => (self.timers_arm9[2].counter as u32) | ((self.timers_arm9[2].control as u32) << 16),
            0x0400_010C => (self.timers_arm9[3].counter as u32) | ((self.timers_arm9[3].control as u32) << 16),
            0x0400_0130 => self.keyinput as u32,
            0x0400_0180 => self.ipc.sync.read_arm9() as u32,
            0x0400_0184 => self.ipc.fifo.read_cnt_arm9() as u32,
            0x0400_0188 => {
                let val = self.ipc.fifo.read_data_arm9();
                if self.ipc.fifo.irq_to_arm7 {
                    self.if_arm7 |= 1 << 17;
                    self.ipc.fifo.irq_to_arm7 = false;
                }
                val
            }
            0x0400_01A4 => self.card.romctrl,
            0x0400_0208 => self.ime_arm9 as u32,
            0x0400_0210 => self.ie_arm9,
            0x0400_0214 => self.if_arm9,
            0x0400_0240 => u32::from_le_bytes([self.vramcnt[0], self.vramcnt[1], self.vramcnt[2], self.vramcnt[3]]),
            0x0400_0244 => u32::from_le_bytes([self.vramcnt[4], self.vramcnt[5], self.vramcnt[6], self.vramcnt[7]]),
            0x0400_0248 => self.vramcnt[8] as u32,
            0x0400_0300 => self.postflg_arm9 as u32,
            0x0400_0304 => self.ppu.powcnt1,
            0x0400_1000..=0x0400_106C => self.ppu.read_io_b(addr),
            0x0410_0000 => self.card.read_card_data(0),
            0x0410_0010 => self.card.romctrl,
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
            0x0600_0000..=0x061F_FFFF => {
                let offset = (addr & 0x1_FFFC) as usize;
                u32::from_le_bytes([
                    self.vram_a[offset],
                    self.vram_a[offset + 1],
                    self.vram_a[offset + 2],
                    self.vram_a[offset + 3],
                ])
            }
            0x0680_0000..=0x0681_FFFF => {
                let offset = (addr & 0x1_FFFC) as usize;
                u32::from_le_bytes([
                    self.vram_a[offset],
                    self.vram_a[offset + 1],
                    self.vram_a[offset + 2],
                    self.vram_a[offset + 3],
                ])
            }
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
            _ => 0,
        }
    }

    pub fn write_arm9_u8(&mut self, addr: u32, val: u8) {
        if self.is_itcm_enabled() && addr >= self.itcm_base() && addr < self.itcm_base() + 0x8000 {
            self.itcm[(addr - self.itcm_base()) as usize] = val;
            return;
        }
        if self.is_dtcm_enabled() && addr >= self.dtcm_base() && addr < self.dtcm_base() + 0x4000 {
            self.dtcm[(addr - self.dtcm_base()) as usize] = val;
            return;
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize] = val,
            0x0300_0000..=0x03FF_FFFF => self.shared_wram[(addr & 0x7FFF) as usize] = val,
            0x0400_0208 => self.ime_arm9 = (val & 1) != 0,
            0x0400_0240..=0x0400_0248 => {
                let idx = (addr - 0x0400_0240) as usize;
                if idx < 9 { self.vramcnt[idx] = val; }
            }
            0x0400_0300 => self.postflg_arm9 = val,
            0x0500_0000..=0x0500_03FF => self.ppu.engine_a.palette[(addr & 0x3FF) as usize] = val,
            0x0500_0400..=0x0500_07FF => self.ppu.engine_b.palette[(addr & 0x3FF) as usize] = val,
            0x0600_0000..=0x061F_FFFF => self.vram_a[(addr & 0x1_FFFF) as usize] = val,
            0x0680_0000..=0x0681_FFFF => self.vram_a[(addr & 0x1_FFFF) as usize] = val,
            0x0682_0000..=0x0683_FFFF => self.vram_b[(addr & 0x1_FFFF) as usize] = val,
            0x0684_0000..=0x0685_FFFF => self.vram_c[(addr & 0x1_FFFF) as usize] = val,
            0x0686_0000..=0x0687_FFFF => self.vram_d[(addr & 0x1_FFFF) as usize] = val,
            0x0688_0000..=0x0688_FFFF => self.vram_e[(addr & 0xFFFF) as usize] = val,
            0x0689_0000..=0x0689_3FFF => self.vram_f[(addr & 0x3FFF) as usize] = val,
            0x0689_4000..=0x0689_7FFF => self.vram_g[(addr & 0x3FFF) as usize] = val,
            0x0689_8000..=0x0689_FFFF => self.vram_h[(addr & 0x7FFF) as usize] = val,
            0x068A_0000..=0x068A_3FFF => self.vram_i[(addr & 0x3FFF) as usize] = val,
            0x0700_0000..=0x0700_03FF => self.ppu.engine_a.oam[(addr & 0x3FF) as usize] = val,
            0x0700_0400..=0x0700_07FF => self.ppu.engine_b.oam[(addr & 0x3FF) as usize] = val,
            _ => {}
        }
    }

    pub fn write_arm9_u16(&mut self, addr: u32, val: u16) {
        if self.is_itcm_enabled() && addr >= self.itcm_base() && addr + 1 < self.itcm_base() + 0x8000 {
            let off = (addr - self.itcm_base()) as usize;
            self.itcm[off..off + 2].copy_from_slice(&val.to_le_bytes());
            return;
        }
        if self.is_dtcm_enabled() && addr >= self.dtcm_base() && addr + 1 < self.dtcm_base() + 0x4000 {
            let off = (addr - self.dtcm_base()) as usize;
            self.dtcm[off..off + 2].copy_from_slice(&val.to_le_bytes());
            return;
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let off = (addr & 0x3F_FFFE) as usize;
                self.main_ram[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0300_0000..=0x03FF_FFFF => {
                let off = (addr & 0x7FFE) as usize;
                self.shared_wram[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0400_0000..=0x0400_006C | 0x0400_0320..=0x0400_06A0 => self.ppu.write_io_a_u16(addr, val),
            0x0400_0100 => self.timers_arm9[0].reload = val,
            0x0400_0102 => self.timers_arm9[0].write_control(val),
            0x0400_0104 => self.timers_arm9[1].reload = val,
            0x0400_0106 => self.timers_arm9[1].write_control(val),
            0x0400_0108 => self.timers_arm9[2].reload = val,
            0x0400_010A => self.timers_arm9[2].write_control(val),
            0x0400_010C => self.timers_arm9[3].reload = val,
            0x0400_010E => self.timers_arm9[3].write_control(val),
            0x0400_0180 => {
                self.ipc.sync.write_arm9(val);
                if self.ipc.sync.irq_to_arm7 {
                    self.if_arm7 |= 1 << 16;
                    self.ipc.sync.irq_to_arm7 = false;
                }
            }
            0x0400_0184 => self.ipc.fifo.write_cnt_arm9(val),
            0x0400_0208 => self.ime_arm9 = (val & 1) != 0,
            0x0400_0210 => self.ie_arm9 = (self.ie_arm9 & 0xFFFF_0000) | (val as u32),
            0x0400_0212 => self.ie_arm9 = (self.ie_arm9 & 0x0000_FFFF) | ((val as u32) << 16),
            0x0400_0214 => self.if_arm9 &= !(val as u32),
            0x0400_0216 => self.if_arm9 &= !((val as u32) << 16),
            0x0400_1000..=0x0400_106C => self.ppu.write_io_b_u16(addr, val),
            0x0500_0000..=0x0500_03FF => {
                let off = (addr & 0x3FE) as usize;
                self.ppu.engine_a.palette[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0500_0400..=0x0500_07FF => {
                let off = (addr & 0x3FE) as usize;
                self.ppu.engine_b.palette[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0600_0000..=0x061F_FFFF => {
                let off = (addr & 0x1_FFFE) as usize;
                self.vram_a[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0700_0000..=0x0700_03FF => {
                let off = (addr & 0x3FE) as usize;
                self.ppu.engine_a.oam[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0700_0400..=0x0700_07FF => {
                let off = (addr & 0x3FE) as usize;
                self.ppu.engine_b.oam[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            _ => {}
        }
    }

    pub fn write_arm9_u32(&mut self, addr: u32, val: u32) {
        if self.is_itcm_enabled() && addr >= self.itcm_base() && addr + 3 < self.itcm_base() + 0x8000 {
            let off = (addr - self.itcm_base()) as usize;
            self.itcm[off..off + 4].copy_from_slice(&val.to_le_bytes());
            return;
        }
        if self.is_dtcm_enabled() && addr >= self.dtcm_base() && addr + 3 < self.dtcm_base() + 0x4000 {
            let off = (addr - self.dtcm_base()) as usize;
            self.dtcm[off..off + 4].copy_from_slice(&val.to_le_bytes());
            return;
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFC) as usize;
                self.main_ram[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0300_0000..=0x03FF_FFFF => {
                let offset = (addr & 0x7FFC) as usize;
                self.shared_wram[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0400_0000..=0x0400_006C | 0x0400_0320..=0x0400_06A0 => self.ppu.write_io_a(addr, val),
            0x0400_00B0 => self.dma_arm9[0].sad = val,
            0x0400_00B4 => self.dma_arm9[0].dad = val,
            0x0400_00B8 => {
                self.dma_arm9[0].cnt = val;
                self.check_trigger_dma(0, true);
            }
            0x0400_00BC => self.dma_arm9[1].sad = val,
            0x0400_00C0 => self.dma_arm9[1].dad = val,
            0x0400_00C4 => {
                self.dma_arm9[1].cnt = val;
                self.check_trigger_dma(1, true);
            }
            0x0400_00C8 => self.dma_arm9[2].sad = val,
            0x0400_00CC => self.dma_arm9[2].dad = val,
            0x0400_00D0 => {
                self.dma_arm9[2].cnt = val;
                self.check_trigger_dma(2, true);
            }
            0x0400_00D4 => self.dma_arm9[3].sad = val,
            0x0400_00D8 => self.dma_arm9[3].dad = val,
            0x0400_00DC => {
                self.dma_arm9[3].cnt = val;
                self.check_trigger_dma(3, true);
            }
            0x0400_0100 => {
                self.timers_arm9[0].reload = val as u16;
                self.timers_arm9[0].write_control((val >> 16) as u16);
            }
            0x0400_0104 => {
                self.timers_arm9[1].reload = val as u16;
                self.timers_arm9[1].write_control((val >> 16) as u16);
            }
            0x0400_0108 => {
                self.timers_arm9[2].reload = val as u16;
                self.timers_arm9[2].write_control((val >> 16) as u16);
            }
            0x0400_010C => {
                self.timers_arm9[3].reload = val as u16;
                self.timers_arm9[3].write_control((val >> 16) as u16);
            }
            0x0400_0180 => {
                self.ipc.sync.write_arm9(val as u16);
                if self.ipc.sync.irq_to_arm7 {
                    self.if_arm7 |= 1 << 16;
                    self.ipc.sync.irq_to_arm7 = false;
                }
            }
            0x0400_0184 => self.ipc.fifo.write_cnt_arm9(val as u16),
            0x0400_0188 => {
                self.ipc.fifo.write_data_arm9(val);
                if self.ipc.fifo.irq_to_arm7 {
                    self.if_arm7 |= 1 << 18;
                    self.ipc.fifo.irq_to_arm7 = false;
                }
            }
            0x0400_01A4 | 0x0410_0010 => self.card.romctrl = val,
            0x0400_0208 => self.ime_arm9 = (val & 1) != 0,
            0x0400_0210 => self.ie_arm9 = val,
            0x0400_0214 => self.if_arm9 &= !val,
            0x0400_0240 => self.vramcnt[0..4].copy_from_slice(&val.to_le_bytes()),
            0x0400_0244 => self.vramcnt[4..8].copy_from_slice(&val.to_le_bytes()),
            0x0400_0248 => self.vramcnt[8] = val as u8,
            0x0400_0300 => self.postflg_arm9 = val as u8,
            0x0400_0304 => self.ppu.powcnt1 = val,
            0x0400_1000..=0x0400_106C => self.ppu.write_io_b(addr, val),
            0x0500_0000..=0x0500_03FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_a.palette[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0500_0400..=0x0500_07FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_b.palette[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0600_0000..=0x061F_FFFF => {
                let offset = (addr & 0x1_FFFC) as usize;
                self.vram_a[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0680_0000..=0x0681_FFFF => {
                let offset = (addr & 0x1_FFFC) as usize;
                self.vram_a[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0700_0000..=0x0700_03FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_a.oam[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0700_0400..=0x0700_07FF => {
                let offset = (addr & 0x3FC) as usize;
                self.ppu.engine_b.oam[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            _ => {}
        }
    }

    // ==========================================
    // ARM7 Memory Access
    // ==========================================

    pub fn read_arm7_u8(&mut self, addr: u32) -> u8 {
        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize],
            0x0380_0000..=0x0380_FFFF => self.arm7_wram[(addr & 0xFFFF) as usize],
            0x0400_0130..=0x0400_0131 => ((self.keyinput >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0136..=0x0400_0137 => ((self.extkeyin >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0180..=0x0400_0181 => ((self.ipc.sync.read_arm7() >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_01C2 => self.spi.spidata as u8,
            0x0400_0208 => self.ime_arm7 as u8,
            0x0400_0300 => self.postflg_arm7,
            _ => (self.read_arm7_u32(addr & !3) >> ((addr & 3) * 8)) as u8,
        }
    }

    pub fn read_arm7_u16(&mut self, addr: u32) -> u16 {
        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let off = (addr & 0x3F_FFFE) as usize;
                u16::from_le_bytes([self.main_ram[off], self.main_ram[off + 1]])
            }
            0x0380_0000..=0x0380_FFFF => {
                let off = (addr & 0xFFFE) as usize;
                u16::from_le_bytes([self.arm7_wram[off], self.arm7_wram[off + 1]])
            }
            0x0400_0100 => self.timers_arm7[0].counter,
            0x0400_0102 => self.timers_arm7[0].control,
            0x0400_0104 => self.timers_arm7[1].counter,
            0x0400_0106 => self.timers_arm7[1].control,
            0x0400_0108 => self.timers_arm7[2].counter,
            0x0400_010A => self.timers_arm7[2].control,
            0x0400_010C => self.timers_arm7[3].counter,
            0x0400_010E => self.timers_arm7[3].control,
            0x0400_0130 => self.keyinput,
            0x0400_0136 => self.extkeyin,
            0x0400_0180 => self.ipc.sync.read_arm7(),
            0x0400_0184 => self.ipc.fifo.read_cnt_arm7(),
            0x0400_01C0 => self.spi.spicnt,
            0x0400_01C2 => self.spi.spidata,
            0x0400_0208 => self.ime_arm7 as u16,
            0x0400_0210 => self.ie_arm7 as u16,
            0x0400_0212 => (self.ie_arm7 >> 16) as u16,
            0x0400_0214 => self.if_arm7 as u16,
            0x0400_0216 => (self.if_arm7 >> 16) as u16,
            _ => (self.read_arm7_u32(addr & !3) >> ((addr & 2) * 8)) as u16,
        }
    }

    pub fn read_arm7_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFC) as usize;
                u32::from_le_bytes([
                    self.main_ram[offset],
                    self.main_ram[offset + 1],
                    self.main_ram[offset + 2],
                    self.main_ram[offset + 3],
                ])
            }
            0x0380_0000..=0x0380_FFFF => {
                let offset = (addr & 0xFFFC) as usize;
                u32::from_le_bytes([
                    self.arm7_wram[offset],
                    self.arm7_wram[offset + 1],
                    self.arm7_wram[offset + 2],
                    self.arm7_wram[offset + 3],
                ])
            }
            0x0400_00B0 => self.dma_arm7[0].sad,
            0x0400_00B4 => self.dma_arm7[0].dad,
            0x0400_00B8 => self.dma_arm7[0].cnt,
            0x0400_00BC => self.dma_arm7[1].sad,
            0x0400_00C0 => self.dma_arm7[1].dad,
            0x0400_00C4 => self.dma_arm7[1].cnt,
            0x0400_00C8 => self.dma_arm7[2].sad,
            0x0400_00CC => self.dma_arm7[2].dad,
            0x0400_00D0 => self.dma_arm7[2].cnt,
            0x0400_00D4 => self.dma_arm7[3].sad,
            0x0400_00D8 => self.dma_arm7[3].dad,
            0x0400_00DC => self.dma_arm7[3].cnt,
            0x0400_0100 => (self.timers_arm7[0].counter as u32) | ((self.timers_arm7[0].control as u32) << 16),
            0x0400_0104 => (self.timers_arm7[1].counter as u32) | ((self.timers_arm7[1].control as u32) << 16),
            0x0400_0108 => (self.timers_arm7[2].counter as u32) | ((self.timers_arm7[2].control as u32) << 16),
            0x0400_010C => (self.timers_arm7[3].counter as u32) | ((self.timers_arm7[3].control as u32) << 16),
            0x0400_0130 => (self.keyinput as u32) | ((self.extkeyin as u32) << 16),
            0x0400_0136 => self.extkeyin as u32,
            0x0400_0180 => self.ipc.sync.read_arm7() as u32,
            0x0400_0184 => self.ipc.fifo.read_cnt_arm7() as u32,
            0x0400_0188 => {
                let val = self.ipc.fifo.read_data_arm7();
                if self.ipc.fifo.irq_to_arm9 {
                    self.if_arm9 |= 1 << 17;
                    self.ipc.fifo.irq_to_arm9 = false;
                }
                val
            }
            0x0400_01C0 => (self.spi.spicnt as u32) | ((self.spi.spidata as u32) << 16),
            0x0400_01C2 => self.spi.spidata as u32,
            0x0400_0208 => self.ime_arm7 as u32,
            0x0400_0210 => self.ie_arm7,
            0x0400_0214 => self.if_arm7,
            0x0400_0300 => self.postflg_arm7 as u32,
            _ => 0,
        }
    }

    pub fn write_arm7_u8(&mut self, addr: u32, val: u8) {
        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize] = val,
            0x0380_0000..=0x0380_FFFF => self.arm7_wram[(addr & 0xFFFF) as usize] = val,
            0x0400_01C2 => {
                let resp = self.spi.transfer_byte(val);
                self.spi.spidata = resp as u16;
            }
            0x0400_0208 => self.ime_arm7 = (val & 1) != 0,
            0x0400_0300 => self.postflg_arm7 = val,
            _ => {}
        }
    }

    pub fn write_arm7_u16(&mut self, addr: u32, val: u16) {
        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let off = (addr & 0x3F_FFFE) as usize;
                self.main_ram[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0380_0000..=0x0380_FFFF => {
                let off = (addr & 0xFFFE) as usize;
                self.arm7_wram[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0400_0100 => self.timers_arm7[0].reload = val,
            0x0400_0102 => self.timers_arm7[0].write_control(val),
            0x0400_0104 => self.timers_arm7[1].reload = val,
            0x0400_0106 => self.timers_arm7[1].write_control(val),
            0x0400_0108 => self.timers_arm7[2].reload = val,
            0x0400_010A => self.timers_arm7[2].write_control(val),
            0x0400_010C => self.timers_arm7[3].reload = val,
            0x0400_010E => self.timers_arm7[3].write_control(val),
            0x0400_0180 => {
                self.ipc.sync.write_arm7(val);
                if self.ipc.sync.irq_to_arm9 {
                    self.if_arm9 |= 1 << 16;
                    self.ipc.sync.irq_to_arm9 = false;
                }
            }
            0x0400_0184 => self.ipc.fifo.write_cnt_arm7(val),
            0x0400_01C0 => self.spi.write_cnt(val),
            0x0400_01C2 => {
                let resp = self.spi.transfer_byte(val as u8);
                self.spi.spidata = resp as u16;
            }
            0x0400_0208 => self.ime_arm7 = (val & 1) != 0,
            0x0400_0210 => self.ie_arm7 = (self.ie_arm7 & 0xFFFF_0000) | (val as u32),
            0x0400_0212 => self.ie_arm7 = (self.ie_arm7 & 0x0000_FFFF) | ((val as u32) << 16),
            0x0400_0214 => self.if_arm7 &= !(val as u32),
            0x0400_0216 => self.if_arm7 &= !((val as u32) << 16),
            _ => {}
        }
    }

    pub fn write_arm7_u32(&mut self, addr: u32, val: u32) {
        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFC) as usize;
                self.main_ram[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0380_0000..=0x0380_FFFF => {
                let offset = (addr & 0xFFFC) as usize;
                self.arm7_wram[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0400_00B0 => self.dma_arm7[0].sad = val,
            0x0400_00B4 => self.dma_arm7[0].dad = val,
            0x0400_00B8 => {
                self.dma_arm7[0].cnt = val;
                self.check_trigger_dma(0, false);
            }
            0x0400_00BC => self.dma_arm7[1].sad = val,
            0x0400_00C0 => self.dma_arm7[1].dad = val,
            0x0400_00C4 => {
                self.dma_arm7[1].cnt = val;
                self.check_trigger_dma(1, false);
            }
            0x0400_00C8 => self.dma_arm7[2].sad = val,
            0x0400_00CC => self.dma_arm7[2].dad = val,
            0x0400_00D0 => {
                self.dma_arm7[2].cnt = val;
                self.check_trigger_dma(2, false);
            }
            0x0400_00D4 => self.dma_arm7[3].sad = val,
            0x0400_00D8 => self.dma_arm7[3].dad = val,
            0x0400_00DC => {
                self.dma_arm7[3].cnt = val;
                self.check_trigger_dma(3, false);
            }
            0x0400_0100 => {
                self.timers_arm7[0].reload = val as u16;
                self.timers_arm7[0].write_control((val >> 16) as u16);
            }
            0x0400_0104 => {
                self.timers_arm7[1].reload = val as u16;
                self.timers_arm7[1].write_control((val >> 16) as u16);
            }
            0x0400_0108 => {
                self.timers_arm7[2].reload = val as u16;
                self.timers_arm7[2].write_control((val >> 16) as u16);
            }
            0x0400_010C => {
                self.timers_arm7[3].reload = val as u16;
                self.timers_arm7[3].write_control((val >> 16) as u16);
            }
            0x0400_0180 => {
                self.ipc.sync.write_arm7(val as u16);
                if self.ipc.sync.irq_to_arm9 {
                    self.if_arm9 |= 1 << 16;
                    self.ipc.sync.irq_to_arm9 = false;
                }
            }
            0x0400_0184 => self.ipc.fifo.write_cnt_arm7(val as u16),
            0x0400_0188 => {
                self.ipc.fifo.write_data_arm7(val);
                if self.ipc.fifo.irq_to_arm9 {
                    self.if_arm9 |= 1 << 18;
                    self.ipc.fifo.irq_to_arm9 = false;
                }
            }
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
            0x0400_0208 => self.ime_arm7 = (val & 1) != 0,
            0x0400_0210 => self.ie_arm7 = val,
            0x0400_0214 => self.if_arm7 &= !val,
            0x0400_0300 => self.postflg_arm7 = val as u8,
            _ => {}
        }
    }
}
