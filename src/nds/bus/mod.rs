//! Nintendo DS Memory Bus Controller & Memory Mapping
//!
//! Routes memory transactions for ARM9 and ARM7 across Main RAM (4MB),
//! Shared WRAM (32KB), ARM7 WRAM (64KB), the 9 VRAM banks (A-I),
//! ITCM/DTCM, PPU (dual 2D engines), SPI, Timers, DMA, and IPC.

use crate::nds::card::NdsCard;
use crate::nds::ipc::Ipc;
use crate::nds::ppu::NdsPpu;
use crate::nds::spi::SpiBus;
use crate::nds::spu::NdsSpu;

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
    /// Internal address/count registers, latched when the channel is enabled.
    pub cur_sad: u32,
    pub cur_dad: u32,
    pub cur_count: u32,
    /// ARM9 DMAxFILL (0x040000E0 + 4x): fill value registers.
    pub fill: u32,
}

/// DMA start timings (GBATEK "DS DMA Transfers").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaEvent {
    VBlank,
    HBlank,
    /// Start of a display frame (ARM9 mode 3).
    DisplayStart,
    /// Slot-1 card data ready (ARM9 mode 5, ARM7 mode 2).
    Card,
}

/// VRAMCNT byte index per bank A..I (WRAMCNT sits at 0x247 between G and H).
const VRAMCNT_INDEX: [usize; 9] = [0, 1, 2, 3, 4, 5, 6, 8, 9];

/// Address spaces VRAM banks can be mapped into (besides LCDC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VramRegion {
    ABg,
    AObj,
    BBg,
    BObj,
    Arm7,
}

impl VramRegion {
    pub fn size(self) -> usize {
        match self {
            VramRegion::ABg => 0x8_0000,
            VramRegion::AObj => 0x4_0000,
            VramRegion::BBg | VramRegion::BObj => 0x2_0000,
            VramRegion::Arm7 => 0x4_0000,
        }
    }
}

/// Per-engine flattened VRAM, rebuilt by [`NdsBus::build_vram_views`].
#[derive(Debug, Clone, Default)]
pub struct VramViews {
    pub a_bg: Vec<u8>,
    pub a_obj: Vec<u8>,
    pub b_bg: Vec<u8>,
    pub b_obj: Vec<u8>,
}

/// Minimal HLE BIOS images. Common SWIs are emulated in the executor; these
/// stubs only supply what real code jumps into: the exception vectors and the
/// BIOS IRQ dispatcher, which calls the user handler whose address sits at
/// 0x03FF_FFFC (ARM7) or DTCM+0x3FFC (ARM9), exactly like the real BIOS.
const BIOS7_STUB: [u32; 14] = [
    0xEAFF_FFFE, // 00 reset: b .
    0xEAFF_FFFE, // 04 undefined: b .
    0xE1B0_F00E, // 08 swi: movs pc, lr (unhandled SWIs are no-ops)
    0xEAFF_FFFE, // 0C prefetch abort
    0xEAFF_FFFE, // 10 data abort
    0xEAFF_FFFE, // 14 reserved
    0xEA00_0000, // 18 irq: b 0x20
    0xEAFF_FFFE, // 1C fiq
    0xE92D_500F, // 20 stmfd sp!, {r0-r3, r12, lr}
    0xE3A0_0301, // 24 mov r0, #0x04000000
    0xE28F_E000, // 28 add lr, pc, #0
    0xE510_F004, // 2C ldr pc, [r0, #-4]
    0xE8BD_500F, // 30 ldmfd sp!, {r0-r3, r12, lr}
    0xE25E_F004, // 34 subs pc, lr, #4
];

const BIOS9_STUB: [u32; 17] = [
    0xEAFF_FFFE, // 00 reset
    0xEAFF_FFFE, // 04 undefined
    0xE1B0_F00E, // 08 swi: movs pc, lr
    0xEAFF_FFFE, // 0C prefetch abort
    0xEAFF_FFFE, // 10 data abort
    0xEAFF_FFFE, // 14 reserved
    0xEA00_0000, // 18 irq: b 0x20
    0xEAFF_FFFE, // 1C fiq
    0xE92D_500F, // 20 stmfd sp!, {r0-r3, r12, lr}
    0xEE19_0F11, // 24 mrc p15, 0, r0, c9, c1, 0  (DTCM region)
    0xE1A0_0620, // 28 mov r0, r0, lsr #12
    0xE1A0_0600, // 2C mov r0, r0, lsl #12
    0xE280_0901, // 30 add r0, r0, #0x4000
    0xE28F_E000, // 34 add lr, pc, #0
    0xE510_F004, // 38 ldr pc, [r0, #-4]
    0xE8BD_500F, // 3C ldmfd sp!, {r0-r3, r12, lr}
    0xE25E_F004, // 40 subs pc, lr, #4
];

#[inline]
fn bios_word(stub: &[u32], offset: u32) -> u32 {
    stub.get((offset >> 2) as usize).copied().unwrap_or(0)
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

    /// Raw bytes of 0x0400_0240..=0x0400_0249: VRAMCNT_A-G, WRAMCNT (idx 7), VRAMCNT_H, VRAMCNT_I.
    pub vramcnt: [u8; 10],
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
    /// EXMEMCNT (ARM9 0x04000204): bit 11 gives Slot-1 to the ARM7.
    pub exmemcnt: u16,
    pub dma_arm9: [NdsDma; 4],
    pub dma_arm7: [NdsDma; 4],

    // Peripherals
    pub ppu: NdsPpu,
    pub spi: SpiBus,
    pub ipc: Ipc,
    /// Engine-side VRAM views for the PPU
    pub vram_views: VramViews,
    /// ARM9 DIV/SQRT unit
    pub math: crate::nds::math::MathUnit,
    pub card: NdsCard,
    pub spu: NdsSpu,
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
            cp15_control: 0x00052078, // DTCM/ITCM on, high vectors (post-BIOS state)
            itcm_control: 0x0000_0020, // base 0, 32 MiB virtual (32 KiB mirrored)
            dtcm_control: 0x027C_000A, // 16 KiB at 0x027C_0000

            vram_a: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_b: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_c: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_d: vec![0u8; 0x20000].into_boxed_slice().try_into().unwrap(),
            vram_e: vec![0u8; 0x10000].into_boxed_slice().try_into().unwrap(),
            vram_f: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),
            vram_g: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),
            vram_h: vec![0u8; 0x8000].into_boxed_slice().try_into().unwrap(),
            vram_i: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),

            vramcnt: [0, 0, 0, 0, 0, 0, 0, 3, 0, 0],
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
            exmemcnt: 0x6000,
            dma_arm9: [NdsDma::default(); 4],
            dma_arm7: [NdsDma::default(); 4],

            ppu: NdsPpu::new(),
            spi: SpiBus::new(),
            ipc: Ipc::default(),
            vram_views: VramViews::default(),
            math: Default::default(),
            card: NdsCard::new(),
            spu: NdsSpu::new(),
        }
    }

    pub fn step_spu(&mut self, cycles: u32) {
        self.spu.step(cycles, &self.main_ram[..], &self.shared_wram[..], &self.arm7_wram[..]);
    }

    /// Shared-WRAM offset for an ARM9 access at 0x03xx_xxxx (WRAMCNT, GBATEK).
    #[inline]
    fn arm9_wram(&self, addr: u32) -> Option<usize> {
        let a = (addr & 0x7FFF) as usize;
        match self.vramcnt[7] & 3 {
            0 => Some(a),                   // 32K to ARM9
            1 => Some(0x4000 | (a & 0x3FFF)), // 2nd half to ARM9
            2 => Some(a & 0x3FFF),          // 1st half to ARM9
            _ => None,                      // all to ARM7: unmapped
        }
    }

    /// ARM7 view of 0x03xx_xxxx: `Some((true, off))` = shared WRAM,
    /// `Some((false, off))` = ARM7 WRAM.
    #[inline]
    fn arm7_wram(&self, addr: u32) -> (bool, usize) {
        if addr >= 0x0380_0000 {
            return (false, (addr & 0xFFFF) as usize);
        }
        let a = (addr & 0x7FFF) as usize;
        match self.vramcnt[7] & 3 {
            0 => (false, (addr & 0xFFFF) as usize), // no shared WRAM: mirrors ARM7 WRAM
            1 => (true, a & 0x3FFF),
            2 => (true, 0x4000 | (a & 0x3FFF)),
            _ => (true, a),
        }
    }

    /// Where bank `idx` (0 = A .. 8 = I) is mapped by its VRAMCNT, as
    /// (region, byte offset inside the region). GBATEK "DS Memory Control - VRAM".
    fn bank_mapping(&self, idx: usize) -> Option<(VramRegion, usize)> {
        let cnt = self.vramcnt[VRAMCNT_INDEX[idx]];
        if cnt & 0x80 == 0 {
            return None;
        }
        let mst = cnt & if idx < 2 { 3 } else { 7 };
        let ofs = ((cnt >> 3) & 3) as usize;
        use VramRegion::*;
        Some(match (idx, mst) {
            (0..=3, 1) => (ABg, 0x20000 * ofs),
            (0..=1, 2) => (AObj, 0x20000 * (ofs & 1)),
            (2..=3, 2) => (Arm7, 0x20000 * (ofs & 1)),
            (2, 4) => (BBg, 0),
            (3, 4) => (BObj, 0),
            (4, 1) => (ABg, 0),
            (4, 2) => (AObj, 0),
            (5..=6, 1) => (ABg, 0x4000 * (ofs & 1) + 0x10000 * (ofs >> 1)),
            (5..=6, 2) => (AObj, 0x4000 * (ofs & 1) + 0x10000 * (ofs >> 1)),
            (7, 1) => (BBg, 0),
            (8, 1) => (BBg, 0x8000),
            (8, 2) => (BObj, 0),
            _ => return None, // LCDC, textures, extended palettes
        })
    }

    fn bank(&self, idx: usize) -> &[u8] {
        match idx {
            0 => &self.vram_a[..], 1 => &self.vram_b[..], 2 => &self.vram_c[..],
            3 => &self.vram_d[..], 4 => &self.vram_e[..], 5 => &self.vram_f[..],
            6 => &self.vram_g[..], 7 => &self.vram_h[..], _ => &self.vram_i[..],
        }
    }

    fn bank_mut(&mut self, idx: usize) -> &mut [u8] {
        match idx {
            0 => &mut self.vram_a[..], 1 => &mut self.vram_b[..], 2 => &mut self.vram_c[..],
            3 => &mut self.vram_d[..], 4 => &mut self.vram_e[..], 5 => &mut self.vram_f[..],
            6 => &mut self.vram_g[..], 7 => &mut self.vram_h[..], _ => &mut self.vram_i[..],
        }
    }

    /// Resolve a region offset to (bank, offset in bank).
    fn vram_lookup(&self, region: VramRegion, off: usize) -> Option<(usize, usize)> {
        let off = off % region.size();
        (0..9).find_map(|idx| {
            let (r, start) = self.bank_mapping(idx)?;
            let len = self.bank(idx).len();
            (r == region && off >= start && off < start + len).then(|| (idx, off - start))
        })
    }

    /// ARM9 engine VRAM at 0x0600_0000..0x067F_FFFF.
    fn arm9_vram(&mut self, addr: u32) -> Option<(&mut [u8], usize)> {
        let region = match (addr >> 21) & 3 {
            0 => VramRegion::ABg,
            1 => VramRegion::BBg,
            2 => VramRegion::AObj,
            _ => VramRegion::BObj,
        };
        let (idx, o) = self.vram_lookup(region, (addr & 0x1F_FFFF) as usize)?;
        Some((self.bank_mut(idx), o))
    }

    /// ARM7 VRAM (banks C/D with MST=2) at 0x0600_0000 + 0x20000*OFS.
    fn arm7_vram(&mut self, addr: u32) -> Option<(&mut [u8], usize)> {
        let (idx, o) = self.vram_lookup(VramRegion::Arm7, (addr & 0x3_FFFF) as usize)?;
        Some((self.bank_mut(idx), o))
    }

    /// Flatten the banks mapped to each engine into contiguous views for the
    /// PPU (A BG 512K, A OBJ 256K, B BG 128K, B OBJ 128K).
    pub fn build_vram_views(&mut self) {
        let mut views = std::mem::take(&mut self.vram_views);
        for (region, buf) in [
            (VramRegion::ABg, &mut views.a_bg),
            (VramRegion::AObj, &mut views.a_obj),
            (VramRegion::BBg, &mut views.b_bg),
            (VramRegion::BObj, &mut views.b_obj),
        ] {
            buf.resize(region.size(), 0);
            buf.fill(0);
            for idx in 0..9 {
                if let Some((r, start)) = self.bank_mapping(idx) {
                    if r == region {
                        let src = self.bank(idx);
                        let end = (start + src.len()).min(buf.len());
                        if start < end {
                            buf[start..end].copy_from_slice(&src[..end - start]);
                        }
                    }
                }
            }
        }
        self.vram_views = views;
    }

    /// VRAMSTAT (0x0400_0240 on ARM7): bit0 = C, bit1 = D mapped to ARM7.
    fn vramstat(&self) -> u8 {
        let c = matches!(self.bank_mapping(2), Some((VramRegion::Arm7, _))) as u8;
        let d = matches!(self.bank_mapping(3), Some((VramRegion::Arm7, _))) as u8;
        c | (d << 1)
    }

    /// Move IPC FIFO interrupt requests into the CPUs' IF registers.
    #[inline]
    fn drain_fifo_irqs(&mut self) {
        self.if_arm9 |= std::mem::take(&mut self.ipc.fifo.pending_arm9);
        self.if_arm7 |= std::mem::take(&mut self.ipc.fifo.pending_arm7);
    }

    /// ITCM is always based at 0; its region register only sets the
    /// virtual size (512 << N bytes), and the 32 KiB mirrors inside it.
    #[inline]
    pub fn itcm_base(&self) -> u32 {
        0
    }

    #[inline]
    pub fn dtcm_base(&self) -> u32 {
        self.dtcm_control & 0xFFFF_F000
    }

    #[inline]
    fn tcm_size(region: u32) -> u64 {
        // Bits 1-5: virtual size = 512 << N (GBATEK CP15 C9,C1).
        512u64 << ((region >> 1) & 0x1F)
    }

    #[inline]
    fn itcm_read_hit(&self, addr: u32) -> Option<usize> {
        if self.cp15_control & (1 << 19) != 0 { None } else { self.itcm_hit(addr) }
    }

    #[inline]
    fn dtcm_read_hit(&self, addr: u32) -> Option<usize> {
        if self.cp15_control & (1 << 17) != 0 { None } else { self.dtcm_hit(addr) }
    }

    /// Offset into ITCM (32 KiB, mirrored) if `addr` hits it.
    #[inline]
    fn itcm_hit(&self, addr: u32) -> Option<usize> {
        if self.is_itcm_enabled() && (addr as u64) < Self::tcm_size(self.itcm_control) {
            Some((addr & 0x7FFF) as usize)
        } else {
            None
        }
    }

    /// Offset into DTCM (16 KiB, mirrored) if `addr` hits it.
    #[inline]
    fn dtcm_hit(&self, addr: u32) -> Option<usize> {
        let base = self.dtcm_base();
        if self.is_dtcm_enabled() && addr >= base && ((addr - base) as u64) < Self::tcm_size(self.dtcm_control) {
            Some(((addr - base) & 0x3FFF) as usize)
        } else {
            None
        }
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

    fn dma(&mut self, arm9: bool, ch: usize) -> &mut NdsDma {
        if arm9 { &mut self.dma_arm9[ch] } else { &mut self.dma_arm7[ch] }
    }

    /// DMA start mode of a channel, normalised to ARM9 numbering
    /// (0 immediate, 1 VBlank, 2 HBlank, 3 display start, 5 card, ...).
    fn dma_mode(&self, arm9: bool, ch: usize) -> u32 {
        let cnt = if arm9 { self.dma_arm9[ch].cnt } else { self.dma_arm7[ch].cnt };
        if arm9 {
            (cnt >> 27) & 7
        } else {
            // ARM7: 0 imm, 1 VBlank, 2 card, 3 wifi/GBA slot
            match (cnt >> 28) & 3 {
                0 => 0,
                1 => 1,
                2 => 5,
                _ => 7,
            }
        }
    }

    /// DMAxCNT write: latch addresses on enable, run immediate transfers.
    fn write_dma_cnt(&mut self, arm9: bool, ch: usize, val: u32) {
        let was_on = if arm9 { self.dma_arm9[ch].cnt } else { self.dma_arm7[ch].cnt } & (1 << 31) != 0;
        let d = self.dma(arm9, ch);
        d.cnt = val;
        if val & (1 << 31) == 0 || was_on {
            return;
        }
        d.cur_sad = d.sad;
        d.cur_dad = d.dad;
        d.cur_count = Self::dma_count(arm9, ch, val);
        match self.dma_mode(arm9, ch) {
            0 => self.run_dma(arm9, ch),
            5 if self.card.data_ready() => self.run_dma(arm9, ch),
            _ => {}
        }
    }

    fn dma_count(arm9: bool, ch: usize, cnt: u32) -> u32 {
        let mask = if arm9 { 0x1F_FFFF } else if ch == 3 { 0xFFFF } else { 0x3FFF };
        let c = cnt & mask;
        if c == 0 { mask + 1 } else { c }
    }

    /// Fire every enabled channel waiting on `event`.
    pub fn trigger_dma(&mut self, event: DmaEvent) {
        for arm9 in [true, false] {
            for ch in 0..4 {
                let cnt = if arm9 { self.dma_arm9[ch].cnt } else { self.dma_arm7[ch].cnt };
                if cnt & (1 << 31) == 0 {
                    continue;
                }
                let mode = self.dma_mode(arm9, ch);
                let hit = match event {
                    DmaEvent::VBlank => mode == 1,
                    DmaEvent::HBlank => arm9 && mode == 2,
                    DmaEvent::DisplayStart => arm9 && mode == 3,
                    DmaEvent::Card => mode == 5,
                };
                if hit {
                    self.run_dma(arm9, ch);
                }
            }
        }
    }

    fn run_dma(&mut self, arm9: bool, ch: usize) {
        let d = *self.dma(arm9, ch);
        let is_32 = d.cnt & (1 << 26) != 0;
        let step = if is_32 { 4 } else { 2 };
        let dad_ctrl = (d.cnt >> 21) & 3;
        let sad_ctrl = (d.cnt >> 23) & 3;
        let card = self.dma_mode(arm9, ch) == 5;
        // Card DMA moves one 512-byte block (or what's left) per request.
        let count = if card { d.cur_count.min(self.card.transfer_count.max(1)) } else { d.cur_count };
        let (mut sad, mut dad) = (d.cur_sad, d.cur_dad);
        for _ in 0..count {
            if is_32 {
                let v = if arm9 { self.read_arm9_u32(sad & !3) } else { self.read_arm7_u32(sad & !3) };
                if arm9 { self.write_arm9_u32(dad & !3, v) } else { self.write_arm7_u32(dad & !3, v) }
            } else {
                let v = if arm9 { self.read_arm9_u16(sad & !1) } else { self.read_arm7_u16(sad & !1) };
                if arm9 { self.write_arm9_u16(dad & !1, v) } else { self.write_arm7_u16(dad & !1, v) }
            }
            match sad_ctrl {
                0 => sad = sad.wrapping_add(step),
                1 => sad = sad.wrapping_sub(step),
                _ => {}
            }
            match dad_ctrl {
                0 | 3 => dad = dad.wrapping_add(step),
                1 => dad = dad.wrapping_sub(step),
                _ => {}
            }
        }
        let remaining = d.cur_count - count;
        let repeat = d.cnt & (1 << 25) != 0;
        let dd = self.dma(arm9, ch);
        dd.cur_sad = sad;
        dd.cur_dad = dad;
        if card && remaining > 0 {
            dd.cur_count = remaining;
            return; // wait for the next card block
        }
        let mode_immediate = (if arm9 { (d.cnt >> 27) & 7 } else { (d.cnt >> 28) & 3 }) == 0;
        if repeat && !mode_immediate {
            dd.cur_count = Self::dma_count(arm9, ch, d.cnt);
            if dad_ctrl == 3 {
                dd.cur_dad = d.dad;
            }
        } else {
            dd.cnt &= !(1 << 31);
        }
        if d.cnt & (1 << 30) != 0 {
            if arm9 { self.if_arm9 |= 1 << (8 + ch) } else { self.if_arm7 |= 1 << (8 + ch) }
        }
    }

    fn read_dma_reg(&self, arm9: bool, addr: u32) -> Option<u32> {
        if !(0x0400_00B0..0x0400_00F0).contains(&addr) {
            return None;
        }
        let a = addr & !3;
        let dmas = if arm9 { &self.dma_arm9 } else { &self.dma_arm7 };
        if a >= 0x0400_00E0 {
            return Some(if arm9 { dmas[((a - 0x0400_00E0) / 4) as usize].fill } else { 0 });
        }
        let ch = ((a - 0x0400_00B0) / 12) as usize;
        Some(match (a - 0x0400_00B0) % 12 {
            0 => dmas[ch].sad,
            4 => dmas[ch].dad,
            _ => dmas[ch].cnt,
        })
    }

    fn write_dma_reg(&mut self, arm9: bool, addr: u32, val: u32) -> bool {
        if !(0x0400_00B0..0x0400_00F0).contains(&addr) {
            return false;
        }
        let a = addr & !3;
        if a >= 0x0400_00E0 {
            if arm9 {
                self.dma_arm9[((a - 0x0400_00E0) / 4) as usize].fill = val;
            }
            return true;
        }
        let ch = ((a - 0x0400_00B0) / 12) as usize;
        match (a - 0x0400_00B0) % 12 {
            0 => self.dma(arm9, ch).sad = val,
            4 => self.dma(arm9, ch).dad = val,
            _ => self.write_dma_cnt(arm9, ch, val),
        }
        true
    }

    /// Card register writes shared by both CPUs (GBATEK 0x040001A0..0x040001AF).
    fn write_card_u8(&mut self, addr: u32, val: u8) {
        match addr {
            0x0400_01A0 => self.card.write_auxspicnt((self.card.auxspicnt & 0xFF00) | val as u16),
            0x0400_01A1 => self.card.write_auxspicnt((self.card.auxspicnt & 0x00FF) | (val as u16) << 8),
            0x0400_01A2 => self.card.write_auxspidata(val),
            0x0400_01A4..=0x0400_01A7 => {
                let sh = (addr & 3) * 8;
                let v = (self.card.romctrl & !(0xFF << sh)) | ((val as u32) << sh);
                self.write_romctrl(v);
            }
            0x0400_01A8..=0x0400_01AF => self.card.cmd_buffer[(addr - 0x0400_01A8) as usize] = val,
            _ => {}
        }
    }

    fn write_romctrl(&mut self, val: u32) {
        if self.card.write_romctrl(val) {
            self.trigger_dma(DmaEvent::Card);
        } else if self.card.romctrl & (1 << 31) == 0 && val & (1 << 31) != 0 {
            // Zero-length command finished immediately.
            self.card_irq();
        }
    }

    fn card_irq(&mut self) {
        if self.card.auxspicnt & (1 << 14) != 0 {
            // Delivered to whichever CPU owns Slot-1 (EXMEMCNT bit 11).
            if self.exmemcnt & (1 << 11) != 0 {
                self.if_arm7 |= 1 << 19;
            } else {
                self.if_arm9 |= 1 << 19;
            }
        }
    }

    /// Read the card data port (0x04100010).
    fn read_card_port(&mut self) -> u32 {
        let (w, done) = self.card.read_data();
        if done {
            self.card_irq();
        } else if self.card.data_ready() && self.card.transfer_count % 128 == 0 {
            // Next 512-byte block ready for card DMA.
            self.trigger_dma(DmaEvent::Card);
        }
        w
    }

    /// Card registers read by either CPU.
    fn read_card_u32(&mut self, addr: u32) -> Option<u32> {
        Some(match addr & !3 {
            0x0400_01A0 => self.card.auxspicnt as u32 | (self.card.auxspidata as u32) << 16,
            0x0400_01A4 => self.card.romctrl,
            0x0400_01A8 => u32::from_le_bytes(self.card.cmd_buffer[0..4].try_into().unwrap()),
            0x0400_01AC => u32::from_le_bytes(self.card.cmd_buffer[4..8].try_into().unwrap()),
            0x0410_0010 => self.read_card_port(),
            _ => return None,
        })
    }

    fn is_card_reg(addr: u32) -> bool {
        (0x0400_01A0..0x0400_01B0).contains(&addr) || (0x0410_0010..0x0410_0014).contains(&addr)
    }

    // ==========================================
    // ARM9 Memory Access
    // ==========================================

    /// LCDC (CPU-direct) VRAM window, 0x0680_0000..0x068A_3FFF: banks A-I
    /// laid out back to back. Returns the bank and the offset inside it.
    #[inline]
    fn lcdc_bank(&mut self, addr: u32) -> Option<(&mut [u8], usize)> {
        let a = addr & 0x00FF_FFFF;
        let (bank, base): (&mut [u8], u32) = match a {
            0x80_0000..=0x81_FFFF => (&mut self.vram_a[..], 0x80_0000),
            0x82_0000..=0x83_FFFF => (&mut self.vram_b[..], 0x82_0000),
            0x84_0000..=0x85_FFFF => (&mut self.vram_c[..], 0x84_0000),
            0x86_0000..=0x87_FFFF => (&mut self.vram_d[..], 0x86_0000),
            0x88_0000..=0x88_FFFF => (&mut self.vram_e[..], 0x88_0000),
            0x89_0000..=0x89_3FFF => (&mut self.vram_f[..], 0x89_0000),
            0x89_4000..=0x89_7FFF => (&mut self.vram_g[..], 0x89_4000),
            0x89_8000..=0x89_FFFF => (&mut self.vram_h[..], 0x89_8000),
            0x8A_0000..=0x8A_3FFF => (&mut self.vram_i[..], 0x8A_0000),
            _ => return None,
        };
        Some((bank, (a - base) as usize))
    }

    #[inline]
    fn is_lcdc(addr: u32) -> bool {
        (0x0680_0000..0x068A_4000).contains(&addr)
    }

    pub fn read_arm9_u8(&mut self, addr: u32) -> u8 {
        if Self::is_card_reg(addr) || (0x0400_0004..0x0400_0008).contains(&addr) || (0x0400_00B0..0x0400_00F0).contains(&addr) {
            if addr == 0x0400_01A2 {
                return self.card.auxspidata;
            }
            return (self.read_arm9_u32(addr & !3) >> ((addr & 3) * 8)) as u8;
        }
        if let Some(off) = self.itcm_read_hit(addr) {
            return self.itcm[off];
        }
        if let Some(off) = self.dtcm_read_hit(addr) {
            return self.dtcm[off];
        }

        if Self::is_lcdc(addr) {
            return self.lcdc_bank(addr).map(|(b, o)| b[o]).unwrap_or(0);
        }

        if (0x0300_0000..0x0380_0000).contains(&addr) || (0x0380_0000..0x0400_0000).contains(&addr) {
            return match self.arm9_wram(addr) {
                Some(o) => { let b = &self.shared_wram[..]; b[o] }
                None => 0,
            };
        }

        if (0x0600_0000..0x0680_0000).contains(&addr) {
            return match self.arm9_vram(addr) {
                Some((b, o)) => b[o],
                None => 0,
            };
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize],
            0x0300_0000..=0x03FF_FFFF => self.shared_wram[(addr & 0x7FFF) as usize],
            0x0400_0130..=0x0400_0131 => ((self.keyinput >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0180..=0x0400_0181 => ((self.ipc.sync.read_arm9() >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0208 => self.ime_arm9 as u8,
            0x0400_0240..=0x0400_0249 => self.vramcnt[(addr - 0x0400_0240) as usize],
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
        if Self::is_card_reg(addr) || (0x0400_00B0..0x0400_00F0).contains(&addr) {
            return (self.read_arm9_u32(addr & !3) >> ((addr & 2) * 8)) as u16;
        }
        if addr == 0x0400_0204 {
            return self.exmemcnt;
        }
        if crate::nds::math::MathUnit::owns(addr) {
            return self.math.read16(addr);
        }
        if addr >= 0xFFFF_0000 {
            return (bios_word(&BIOS9_STUB, (addr - 0xFFFF_0000) & !3) >> ((addr & 2) * 8)) as u16;
        }
        if let Some(off) = self.itcm_read_hit(addr) {
            return u16::from_le_bytes([self.itcm[off], self.itcm[off + 1]]);
        }
        if let Some(off) = self.dtcm_read_hit(addr) {
            return u16::from_le_bytes([self.dtcm[off], self.dtcm[off + 1]]);
        }

        if Self::is_lcdc(addr) {
            return self.lcdc_bank(addr & !1).map(|(b, o)| u16::from_le_bytes([b[o], b[o + 1]])).unwrap_or(0);
        }

        if (0x0300_0000..0x0380_0000).contains(&addr) || (0x0380_0000..0x0400_0000).contains(&addr) {
            return match self.arm9_wram(addr) {
                Some(o) => { let b = &self.shared_wram[..]; u16::from_le_bytes([b[o & !1], b[(o & !1) + 1]]) }
                None => 0,
            };
        }

        if (0x0600_0000..0x0680_0000).contains(&addr) {
            return match self.arm9_vram(addr) {
                Some((b, o)) => u16::from_le_bytes([b[o & !1], b[(o & !1) + 1]]),
                None => 0,
            };
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
        if Self::is_card_reg(addr) {
            if let Some(v) = self.read_card_u32(addr) {
                return v;
            }
        }
        if let Some(v) = self.read_dma_reg(true, addr) {
            return v;
        }
        if crate::nds::math::MathUnit::owns(addr) {
            return self.math.read32(addr);
        }
        if addr >= 0xFFFF_0000 {
            return bios_word(&BIOS9_STUB, (addr - 0xFFFF_0000) & !3);
        }
        if let Some(off) = self.itcm_read_hit(addr) {
            return u32::from_le_bytes([
                self.itcm[off],
                self.itcm[off + 1],
                self.itcm[off + 2],
                self.itcm[off + 3],
            ]);
        }
        if let Some(off) = self.dtcm_read_hit(addr) {
            return u32::from_le_bytes([
                self.dtcm[off],
                self.dtcm[off + 1],
                self.dtcm[off + 2],
                self.dtcm[off + 3],
            ]);
        }

        if Self::is_lcdc(addr) {
            return self
                .lcdc_bank(addr & !3)
                .map(|(b, o)| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]))
                .unwrap_or(0);
        }

        if (0x0300_0000..0x0380_0000).contains(&addr) || (0x0380_0000..0x0400_0000).contains(&addr) {
            return match self.arm9_wram(addr) {
                Some(o) => { let b = &self.shared_wram[..]; u32::from_le_bytes([b[o & !3], b[(o & !3) + 1], b[(o & !3) + 2], b[(o & !3) + 3]]) }
                None => 0,
            };
        }

        if (0x0600_0000..0x0680_0000).contains(&addr) {
            return match self.arm9_vram(addr) {
                Some((b, o)) => u32::from_le_bytes([b[o & !3], b[(o & !3) + 1], b[(o & !3) + 2], b[(o & !3) + 3]]),
                None => 0,
            };
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
            0x0410_0000 => {
                let val = self.ipc.fifo.read_data_arm9();
                self.drain_fifo_irqs();
                val
            }
            0x0400_0208 => self.ime_arm9 as u32,
            0x0400_0210 => self.ie_arm9,
            0x0400_0214 => self.if_arm9,
            0x0400_0240 => u32::from_le_bytes([self.vramcnt[0], self.vramcnt[1], self.vramcnt[2], self.vramcnt[3]]),
            0x0400_0244 => u32::from_le_bytes([self.vramcnt[4], self.vramcnt[5], self.vramcnt[6], self.vramcnt[7]]),
            0x0400_0248 => u16::from_le_bytes([self.vramcnt[8], self.vramcnt[9]]) as u32,
            0x0400_0300 => self.postflg_arm9 as u32,
            0x0400_0304 => self.ppu.powcnt1,
            0x0400_1000..=0x0400_106C => self.ppu.read_io_b(addr),
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
        if Self::is_card_reg(addr) {
            self.write_card_u8(addr, val);
            return;
        }
        if (0x0400_00B0..0x0400_00F0).contains(&addr) {
            let old = self.read_dma_reg(true, addr & !3).unwrap_or(0);
            let sh = (addr & 3) * 8;
            self.write_dma_reg(true, addr & !3, (old & !(0xFF << sh)) | ((val as u32) << sh));
            return;
        }
        if let Some(off) = self.itcm_hit(addr) {
            self.itcm[off] = val;
            return;
        }
        if let Some(off) = self.dtcm_hit(addr) {
            self.dtcm[off] = val;
            return;
        }

        if Self::is_lcdc(addr) {
            // Byte writes to VRAM are ignored on hardware.
            return;
        }

        if (0x0300_0000..0x0400_0000).contains(&addr) {
            if let Some(o) = self.arm9_wram(addr) {
                let b = &mut self.shared_wram[..];
                b[o] = val;
            }
            return;
        }

        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize] = val,
            0x0300_0000..=0x03FF_FFFF => self.shared_wram[(addr & 0x7FFF) as usize] = val,
            0x0400_0208 => self.ime_arm9 = (val & 1) != 0,
            0x0400_0240..=0x0400_0249 => self.vramcnt[(addr - 0x0400_0240) as usize] = val,
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
        if Self::is_card_reg(addr) {
            if addr == 0x0400_01A0 {
                self.card.write_auxspicnt(val);
            } else {
                self.write_card_u8(addr, val as u8);
                self.write_card_u8(addr + 1, (val >> 8) as u8);
            }
            return;
        }
        if (0x0400_00B0..0x0400_00F0).contains(&addr) {
            let old = self.read_dma_reg(true, addr & !3).unwrap_or(0);
            let sh = (addr & 2) * 8;
            self.write_dma_reg(true, addr & !3, (old & !(0xFFFF << sh)) | ((val as u32) << sh));
            return;
        }
        if addr == 0x0400_0204 {
            self.exmemcnt = val;
            return;
        }
        if crate::nds::math::MathUnit::owns(addr) {
            self.math.write16(addr, val);
            return;
        }
        if let Some(off) = self.itcm_hit(addr) {
            self.itcm[off..off + 2].copy_from_slice(&val.to_le_bytes());
            return;
        }
        if let Some(off) = self.dtcm_hit(addr) {
            self.dtcm[off..off + 2].copy_from_slice(&val.to_le_bytes());
            return;
        }

        if Self::is_lcdc(addr) {
            if let Some((b, o)) = self.lcdc_bank(addr & !1) {
                b[o..o + 2].copy_from_slice(&val.to_le_bytes());
            }
            return;
        }

        if (0x0300_0000..0x0400_0000).contains(&addr) {
            if let Some(o) = self.arm9_wram(addr) {
                let b = &mut self.shared_wram[..];
                b[o & !1..(o & !1) + 2].copy_from_slice(&val.to_le_bytes());
            }
            return;
        }

        if (0x0600_0000..0x0680_0000).contains(&addr) {
            if let Some((b, o)) = self.arm9_vram(addr) {
                b[o & !1..(o & !1) + 2].copy_from_slice(&val.to_le_bytes());
            }
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
            0x0400_0184 => { self.ipc.fifo.write_cnt_arm9(val); self.drain_fifo_irqs(); }
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
        if Self::is_card_reg(addr) {
            if addr == 0x0400_01A4 {
                self.write_romctrl(val);
            } else {
                for (i, b) in val.to_le_bytes().iter().enumerate() {
                    self.write_card_u8(addr + i as u32, *b);
                }
            }
            return;
        }
        if self.write_dma_reg(true, addr, val) {
            return;
        }
        if crate::nds::math::MathUnit::owns(addr) {
            self.math.write32(addr, val);
            return;
        }
        if let Some(off) = self.itcm_hit(addr) {
            self.itcm[off..off + 4].copy_from_slice(&val.to_le_bytes());
            return;
        }
        if let Some(off) = self.dtcm_hit(addr) {
            self.dtcm[off..off + 4].copy_from_slice(&val.to_le_bytes());
            return;
        }

        if Self::is_lcdc(addr) {
            if let Some((b, o)) = self.lcdc_bank(addr & !3) {
                b[o..o + 4].copy_from_slice(&val.to_le_bytes());
            }
            return;
        }

        if (0x0300_0000..0x0400_0000).contains(&addr) {
            if let Some(o) = self.arm9_wram(addr) {
                let b = &mut self.shared_wram[..];
                b[o & !3..(o & !3) + 4].copy_from_slice(&val.to_le_bytes());
            }
            return;
        }

        if (0x0600_0000..0x0680_0000).contains(&addr) {
            if let Some((b, o)) = self.arm9_vram(addr) {
                b[o & !3..(o & !3) + 4].copy_from_slice(&val.to_le_bytes());
            }
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
            0x0400_00B8 => self.write_dma_cnt(true, 0, val),
            0x0400_00BC => self.dma_arm9[1].sad = val,
            0x0400_00C0 => self.dma_arm9[1].dad = val,
            0x0400_00C4 => self.write_dma_cnt(true, 1, val),
            0x0400_00C8 => self.dma_arm9[2].sad = val,
            0x0400_00CC => self.dma_arm9[2].dad = val,
            0x0400_00D0 => self.write_dma_cnt(true, 2, val),
            0x0400_00D4 => self.dma_arm9[3].sad = val,
            0x0400_00D8 => self.dma_arm9[3].dad = val,
            0x0400_00DC => self.write_dma_cnt(true, 3, val),
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
            0x0400_0184 => { self.ipc.fifo.write_cnt_arm9(val as u16); self.drain_fifo_irqs(); }
            0x0400_0188 => {
                self.ipc.fifo.write_data_arm9(val);
                self.drain_fifo_irqs();
            }
            0x0400_0208 => self.ime_arm9 = (val & 1) != 0,
            0x0400_0210 => self.ie_arm9 = val,
            0x0400_0214 => self.if_arm9 &= !val,
            0x0400_0240 => self.vramcnt[0..4].copy_from_slice(&val.to_le_bytes()),
            0x0400_0244 => self.vramcnt[4..8].copy_from_slice(&val.to_le_bytes()),
            0x0400_0248 => self.vramcnt[8..10].copy_from_slice(&(val as u16).to_le_bytes()),
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
        if Self::is_card_reg(addr) || (0x0400_0004..0x0400_0008).contains(&addr) || (0x0400_00B0..0x0400_00E0).contains(&addr) {
            if addr == 0x0400_01A2 {
                return self.card.auxspidata;
            }
            return (self.read_arm7_u32(addr & !3) >> ((addr & 3) * 8)) as u8;
        }
        if (0x0300_0000..0x0400_0000).contains(&addr) {
            let (shared, o) = self.arm7_wram(addr);
            let b = if shared { &self.shared_wram[..] } else { &self.arm7_wram[..] };
            return b[o];
        }
        if (0x0600_0000..0x0700_0000).contains(&addr) {
            return match self.arm7_vram(addr) {
                Some((b, o)) => b[o],
                None => 0,
            };
        }
        if addr == 0x0400_0240 {
            return self.vramstat();
        }
        if addr == 0x0400_0241 {
            return self.vramcnt[7] & 3;
        }
        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize],
            0x0380_0000..=0x03FF_FFFF => self.arm7_wram[(addr & 0xFFFF) as usize],
            0x0400_0130..=0x0400_0131 => ((self.keyinput >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0136..=0x0400_0137 => ((self.extkeyin >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_0180..=0x0400_0181 => ((self.ipc.sync.read_arm7() >> ((addr & 1) * 8)) & 0xFF) as u8,
            0x0400_01C2 => self.spi.spidata as u8,
            0x0400_0208 => self.ime_arm7 as u8,
            0x0400_0300 => self.postflg_arm7,
            0x0400_0400..=0x0400_051F => self.spu.read_u8(addr),
            _ => (self.read_arm7_u32(addr & !3) >> ((addr & 3) * 8)) as u8,
        }
    }

    pub fn read_arm7_u16(&mut self, addr: u32) -> u16 {
        if Self::is_card_reg(addr) || (0x0400_00B0..0x0400_00E0).contains(&addr) {
            return (self.read_arm7_u32(addr & !3) >> ((addr & 2) * 8)) as u16;
        }
        match addr {
            0x0400_0004 => return self.ppu.dispstat_b,
            0x0400_0006 => return self.ppu.vcount,
            0x0400_0204 => return self.exmemcnt,
            _ => {}
        }
        if (0x0300_0000..0x0400_0000).contains(&addr) {
            let (shared, o) = self.arm7_wram(addr);
            let b = if shared { &self.shared_wram[..] } else { &self.arm7_wram[..] };
            return u16::from_le_bytes([b[o & !1], b[(o & !1) + 1]]);
        }
        if (0x0600_0000..0x0700_0000).contains(&addr) {
            return match self.arm7_vram(addr) {
                Some((b, o)) => u16::from_le_bytes([b[o & !1], b[(o & !1) + 1]]),
                None => 0,
            };
        }
        if addr < 0x4000 {
            return (bios_word(&BIOS7_STUB, addr & !3) >> ((addr & 2) * 8)) as u16;
        }
        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let off = (addr & 0x3F_FFFE) as usize;
                u16::from_le_bytes([self.main_ram[off], self.main_ram[off + 1]])
            }
            0x0380_0000..=0x03FF_FFFF => {
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
            0x0400_0400..=0x0400_051F => self.spu.read_u16(addr),
            _ => (self.read_arm7_u32(addr & !3) >> ((addr & 2) * 8)) as u16,
        }
    }

    pub fn read_arm7_u32(&mut self, addr: u32) -> u32 {
        if Self::is_card_reg(addr) {
            if let Some(v) = self.read_card_u32(addr) {
                return v;
            }
        }
        if let Some(v) = self.read_dma_reg(false, addr) {
            return v;
        }
        if addr == 0x0400_0004 {
            return self.ppu.dispstat_b as u32 | (self.ppu.vcount as u32) << 16;
        }
        if (0x0300_0000..0x0400_0000).contains(&addr) {
            let (shared, o) = self.arm7_wram(addr);
            let b = if shared { &self.shared_wram[..] } else { &self.arm7_wram[..] };
            return u32::from_le_bytes([b[o & !3], b[(o & !3) + 1], b[(o & !3) + 2], b[(o & !3) + 3]]);
        }
        if (0x0600_0000..0x0700_0000).contains(&addr) {
            return match self.arm7_vram(addr) {
                Some((b, o)) => u32::from_le_bytes([b[o & !3], b[(o & !3) + 1], b[(o & !3) + 2], b[(o & !3) + 3]]),
                None => 0,
            };
        }
        if addr < 0x4000 {
            return bios_word(&BIOS7_STUB, addr & !3);
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
            0x0380_0000..=0x03FF_FFFF => {
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
            0x0410_0000 => {
                let val = self.ipc.fifo.read_data_arm7();
                self.drain_fifo_irqs();
                val
            }
            0x0400_01C0 => (self.spi.spicnt as u32) | ((self.spi.spidata as u32) << 16),
            0x0400_01C2 => self.spi.spidata as u32,
            0x0400_0208 => self.ime_arm7 as u32,
            0x0400_0210 => self.ie_arm7,
            0x0400_0214 => self.if_arm7,
            0x0400_0300 => self.postflg_arm7 as u32,
            0x0400_0400..=0x0400_051F => self.spu.read_u32(addr),
            _ => 0,
        }
    }

    pub fn write_arm7_u8(&mut self, addr: u32, val: u8) {
        if Self::is_card_reg(addr) {
            self.write_card_u8(addr, val);
            return;
        }
        if (0x0400_00B0..0x0400_00F0).contains(&addr) {
            let old = self.read_dma_reg(false, addr & !3).unwrap_or(0);
            let sh = (addr & 3) * 8;
            self.write_dma_reg(false, addr & !3, (old & !(0xFF << sh)) | ((val as u32) << sh));
            return;
        }
        if (0x0300_0000..0x0400_0000).contains(&addr) {
            let (shared, o) = self.arm7_wram(addr);
            let b = if shared { &mut self.shared_wram[..] } else { &mut self.arm7_wram[..] };
            b[o] = val;
            return;
        }
        if (0x0600_0000..0x0700_0000).contains(&addr) {
            if let Some((b, o)) = self.arm7_vram(addr) {
                b[o] = val;
            }
            return;
        }
        match addr {
            0x0200_0000..=0x02FF_FFFF => self.main_ram[(addr & 0x3F_FFFF) as usize] = val,
            0x0380_0000..=0x03FF_FFFF => self.arm7_wram[(addr & 0xFFFF) as usize] = val,
            0x0400_01C2 => {
                let resp = self.spi.transfer_byte(val);
                self.spi.spidata = resp as u16;
            }
            0x0400_0208 => self.ime_arm7 = (val & 1) != 0,
            0x0400_0300 => self.postflg_arm7 = val,
            0x0400_0400..=0x0400_051F => self.spu.write_u8(addr, val, &self.main_ram[..], &self.shared_wram[..], &self.arm7_wram[..]),
            _ => {}
        }
    }

    pub fn write_arm7_u16(&mut self, addr: u32, val: u16) {
        if Self::is_card_reg(addr) {
            if addr == 0x0400_01A0 {
                self.card.write_auxspicnt(val);
            } else {
                self.write_card_u8(addr, val as u8);
                self.write_card_u8(addr + 1, (val >> 8) as u8);
            }
            return;
        }
        if (0x0400_00B0..0x0400_00F0).contains(&addr) {
            let old = self.read_dma_reg(false, addr & !3).unwrap_or(0);
            let sh = (addr & 2) * 8;
            self.write_dma_reg(false, addr & !3, (old & !(0xFFFF << sh)) | ((val as u32) << sh));
            return;
        }
        if addr == 0x0400_0004 {
            self.ppu.dispstat_b = (self.ppu.dispstat_b & 0x07) | (val & !0x07);
            return;
        }
        if (0x0300_0000..0x0400_0000).contains(&addr) {
            let (shared, o) = self.arm7_wram(addr);
            let b = if shared { &mut self.shared_wram[..] } else { &mut self.arm7_wram[..] };
            b[o & !1..(o & !1) + 2].copy_from_slice(&val.to_le_bytes());
            return;
        }
        if (0x0600_0000..0x0700_0000).contains(&addr) {
            if let Some((b, o)) = self.arm7_vram(addr) {
                b[o & !1..(o & !1) + 2].copy_from_slice(&val.to_le_bytes());
            }
            return;
        }
        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let off = (addr & 0x3F_FFFE) as usize;
                self.main_ram[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            0x0380_0000..=0x03FF_FFFF => {
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
            0x0400_0184 => { self.ipc.fifo.write_cnt_arm7(val); self.drain_fifo_irqs(); }
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
            0x0400_0400..=0x0400_051F => self.spu.write_u16(addr, val, &self.main_ram[..], &self.shared_wram[..], &self.arm7_wram[..]),
            _ => {}
        }
    }

    pub fn write_arm7_u32(&mut self, addr: u32, val: u32) {
        if Self::is_card_reg(addr) {
            if addr == 0x0400_01A4 {
                self.write_romctrl(val);
            } else {
                for (i, b) in val.to_le_bytes().iter().enumerate() {
                    self.write_card_u8(addr + i as u32, *b);
                }
            }
            return;
        }
        if self.write_dma_reg(false, addr, val) {
            return;
        }
        if addr == 0x0400_0004 {
            self.ppu.dispstat_b = (self.ppu.dispstat_b & 0x07) | (val as u16 & !0x07);
            return;
        }
        if (0x0300_0000..0x0400_0000).contains(&addr) {
            let (shared, o) = self.arm7_wram(addr);
            let b = if shared { &mut self.shared_wram[..] } else { &mut self.arm7_wram[..] };
            b[o & !3..(o & !3) + 4].copy_from_slice(&val.to_le_bytes());
            return;
        }
        if (0x0600_0000..0x0700_0000).contains(&addr) {
            if let Some((b, o)) = self.arm7_vram(addr) {
                b[o & !3..(o & !3) + 4].copy_from_slice(&val.to_le_bytes());
            }
            return;
        }
        match addr {
            0x0200_0000..=0x02FF_FFFF => {
                let offset = (addr & 0x3F_FFFC) as usize;
                self.main_ram[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0380_0000..=0x03FF_FFFF => {
                let offset = (addr & 0xFFFC) as usize;
                self.arm7_wram[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
            }
            0x0400_00B0 => self.dma_arm7[0].sad = val,
            0x0400_00B4 => self.dma_arm7[0].dad = val,
            0x0400_00B8 => self.write_dma_cnt(false, 0, val),
            0x0400_00BC => self.dma_arm7[1].sad = val,
            0x0400_00C0 => self.dma_arm7[1].dad = val,
            0x0400_00C4 => self.write_dma_cnt(false, 1, val),
            0x0400_00C8 => self.dma_arm7[2].sad = val,
            0x0400_00CC => self.dma_arm7[2].dad = val,
            0x0400_00D0 => self.write_dma_cnt(false, 2, val),
            0x0400_00D4 => self.dma_arm7[3].sad = val,
            0x0400_00D8 => self.dma_arm7[3].dad = val,
            0x0400_00DC => self.write_dma_cnt(false, 3, val),
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
            0x0400_0184 => { self.ipc.fifo.write_cnt_arm7(val as u16); self.drain_fifo_irqs(); }
            0x0400_0188 => {
                self.ipc.fifo.write_data_arm7(val);
                self.drain_fifo_irqs();
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
            0x0400_0400..=0x0400_051F => self.spu.write_u32(addr, val, &self.main_ram[..], &self.shared_wram[..], &self.arm7_wram[..]),
            _ => {}
        }
    }
}
