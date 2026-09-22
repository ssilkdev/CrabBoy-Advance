//! Game Boy / Game Boy Color memory map, timer, joypad and DMA.
//!
//! Address space:
//! ```text
//! 0000-3FFF  ROM bank 0          4000-7FFF  ROM bank N (MBC)
//! 8000-9FFF  VRAM (2 banks CGB)  A000-BFFF  cartridge RAM
//! C000-CFFF  WRAM bank 0         D000-DFFF  WRAM bank 1-7 (CGB, SVBK)
//! E000-FDFF  echo of C000-DDFF   FE00-FE9F  OAM
//! FEA0-FEFF  unusable            FF00-FF7F  I/O
//! FF80-FFFE  HRAM                FFFF       IE
//! ```

use super::apu::GbApu;
use super::cartridge::GbCartridge;
use super::ppu::GbPpu;

/// Joypad buttons. Bit order matches the hardware nibbles: the low nibble of
/// P1 reports either directions or actions depending on the select bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GbKey {
    Right = 0,
    Left = 1,
    Up = 2,
    Down = 3,
    A = 4,
    B = 5,
    Select = 6,
    Start = 7,
}

pub struct GbMmu {
    pub cart: GbCartridge,
    pub ppu: GbPpu,
    pub apu: GbApu,

    pub cgb: bool,
    pub wram: Vec<u8>,
    pub wram_bank: usize,
    pub hram: [u8; 0x7F],

    pub ie_reg: u8,
    pub if_reg: u8,

    // --- timer ---
    /// The DIV register is the upper 8 bits of a free-running 16-bit counter;
    /// writing to DIV resets the whole counter, which is observable because
    /// TIMA is clocked off a falling edge of one of its bits.
    pub div_counter: u16,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
    tima_overflow_pending: bool,

    // --- joypad ---
    /// 1 = released, matching the hardware's active-low reporting.
    pub buttons: u8,
    pub p1_select: u8,

    // --- serial ---
    pub sb: u8,
    pub sc: u8,
    serial_counter: u32,
    /// Bytes shifted out of the link port. Blargg's test ROMs print here, so
    /// this doubles as the automated pass/fail channel.
    pub serial_out: Vec<u8>,

    // --- CGB ---
    pub key1: u8,
    pub double_speed: bool,
    /// HDMA/GDMA source, destination and length (CGB VRAM DMA).
    pub hdma_src: u16,
    pub hdma_dst: u16,
    pub hdma_len: u8,
    pub hdma_active: bool,
    /// Object priority mode (OPRI, FF6C): CGB boot ROM sets 0 = OAM order.
    pub opri: u8,

    pub boot_rom_mapped: bool,
}

impl GbMmu {
    pub fn new(rom: Vec<u8>, cgb: bool) -> Self {
        let cart = GbCartridge::from_bytes(rom);
        Self::with_cartridge(cart, cgb)
    }

    pub fn with_cartridge(cart: GbCartridge, cgb: bool) -> Self {
        Self {
            cart,
            ppu: GbPpu::new(cgb),
            apu: GbApu::new(),
            cgb,
            wram: vec![0; if cgb { 0x8000 } else { 0x2000 }],
            wram_bank: 1,
            hram: [0; 0x7F],
            ie_reg: 0,
            if_reg: 0xE1,
            div_counter: 0xABCC,
            tima: 0,
            tma: 0,
            tac: 0xF8,
            tima_overflow_pending: false,
            buttons: 0xFF,
            p1_select: 0x30,
            sb: 0,
            sc: 0x7E,
            serial_counter: 0,
            serial_out: Vec::new(),
            key1: 0,
            double_speed: false,
            hdma_src: 0,
            hdma_dst: 0,
            hdma_len: 0xFF,
            hdma_active: false,
            opri: 0,
            boot_rom_mapped: false,
        }
    }

    pub fn set_key(&mut self, key: GbKey, pressed: bool) {
        let bit = 1u8 << (key as u8);
        let was_pressed = (self.buttons & bit) == 0;
        if pressed {
            self.buttons &= !bit;
            if !was_pressed {
                // Joypad interrupt fires on any high-to-low transition of a
                // selected line. Games use it to wake from STOP.
                self.if_reg |= 0x10;
            }
        } else {
            self.buttons |= bit;
        }
    }

    fn read_p1(&self) -> u8 {
        let mut v = 0xC0 | (self.p1_select & 0x30) | 0x0F;
        if (self.p1_select & 0x10) == 0 {
            // directions selected
            v &= 0xF0 | (self.buttons & 0x0F);
        }
        if (self.p1_select & 0x20) == 0 {
            v &= 0xF0 | ((self.buttons >> 4) & 0x0F);
        }
        v
    }

    // --- bus ------------------------------------------------------------

    pub fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xCFFF => self.wram[(addr - 0xC000) as usize],
            0xD000..=0xDFFF => {
                let bank = if self.cgb { self.wram_bank.max(1) } else { 1 };
                self.wram[bank * 0x1000 + (addr - 0xD000) as usize]
            }
            // Echo RAM mirrors C000-DDFF. Real cartridges do hit it.
            0xE000..=0xFDFF => self.read(addr - 0x2000),
            0xFE00..=0xFE9F => self.ppu.oam[(addr - 0xFE00) as usize],
            0xFEA0..=0xFEFF => 0x00,
            0xFF00..=0xFF7F => self.read_io(addr),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.ie_reg,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x7FFF => self.cart.write_control(addr, val),
            0x8000..=0x9FFF => self.ppu.write_vram(addr, val),
            0xA000..=0xBFFF => self.cart.write_ram(addr, val),
            0xC000..=0xCFFF => self.wram[(addr - 0xC000) as usize] = val,
            0xD000..=0xDFFF => {
                let bank = if self.cgb { self.wram_bank.max(1) } else { 1 };
                self.wram[bank * 0x1000 + (addr - 0xD000) as usize] = val;
            }
            0xE000..=0xFDFF => self.write(addr - 0x2000, val),
            0xFE00..=0xFE9F => self.ppu.oam[(addr - 0xFE00) as usize] = val,
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => self.write_io(addr, val),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = val,
            0xFFFF => self.ie_reg = val,
        }
    }

    fn read_io(&self, addr: u16) -> u8 {
        match addr {
            0xFF00 => self.read_p1(),
            0xFF01 => self.sb,
            0xFF02 => self.sc | 0x7E,
            0xFF04 => (self.div_counter >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => self.tac | 0xF8,
            0xFF0F => self.if_reg | 0xE0,
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF40 => self.ppu.lcdc,
            0xFF41 => self.ppu.stat | 0x80,
            0xFF42 => self.ppu.scy,
            0xFF43 => self.ppu.scx,
            0xFF44 => self.ppu.ly,
            0xFF45 => self.ppu.lyc,
            0xFF47 => self.ppu.bgp,
            0xFF48 => self.ppu.obp0,
            0xFF49 => self.ppu.obp1,
            0xFF4A => self.ppu.wy,
            0xFF4B => self.ppu.wx,
            0xFF4D if self.cgb => {
                (if self.double_speed { 0x80 } else { 0 }) | (self.key1 & 0x01) | 0x7E
            }
            0xFF4F if self.cgb => (self.ppu.vram_bank as u8) | 0xFE,
            0xFF51 if self.cgb => (self.hdma_src >> 8) as u8,
            0xFF52 if self.cgb => self.hdma_src as u8,
            0xFF53 if self.cgb => (self.hdma_dst >> 8) as u8,
            0xFF54 if self.cgb => self.hdma_dst as u8,
            0xFF55 if self.cgb => {
                // Bit 7 clear means a transfer is in progress.
                if self.hdma_active {
                    self.hdma_len & 0x7F
                } else {
                    0xFF
                }
            }
            0xFF68 if self.cgb => self.ppu.bcps | 0x40,
            0xFF69 if self.cgb => self.ppu.read_bcpd(),
            0xFF6A if self.cgb => self.ppu.ocps | 0x40,
            0xFF6B if self.cgb => self.ppu.read_ocpd(),
            0xFF6C if self.cgb => self.opri | 0xFE,
            0xFF70 if self.cgb => (self.wram_bank as u8) | 0xF8,
            _ => 0xFF,
        }
    }

    fn write_io(&mut self, addr: u16, val: u8) {
        match addr {
            0xFF00 => self.p1_select = val & 0x30,
            0xFF01 => self.sb = val,
            0xFF02 => {
                self.sc = val;
                if (val & 0x81) == 0x81 {
                    // Internal clock: start shifting a byte out.
                    self.serial_counter = if self.cgb && (val & 0x02) != 0 {
                        // fast mode: 256 kHz
                        512
                    } else {
                        8192
                    };
                }
            }
            0xFF04 => {
                self.div_counter = 0;
            }
            0xFF05 => {
                self.tima = val;
                self.tima_overflow_pending = false;
            }
            0xFF06 => self.tma = val,
            0xFF07 => self.tac = val & 0x07,
            0xFF0F => self.if_reg = val & 0x1F,
            0xFF10..=0xFF3F => self.apu.write(addr, val),
            0xFF40 => {
                let was_on = self.ppu.lcd_enabled();
                self.ppu.lcdc = val;
                if was_on && (val & 0x80) == 0 {
                    self.ppu.ly = 0;
                    self.ppu.dot = 0;
                }
            }
            // Bits 0-2 of STAT are read-only mode/coincidence status.
            0xFF41 => self.ppu.stat = (val & 0x78) | (self.ppu.stat & 0x07),
            0xFF42 => self.ppu.scy = val,
            0xFF43 => self.ppu.scx = val,
            0xFF44 => {} // LY is read-only
            0xFF45 => self.ppu.lyc = val,
            0xFF46 => self.oam_dma(val),
            0xFF47 => self.ppu.bgp = val,
            0xFF48 => self.ppu.obp0 = val,
            0xFF49 => self.ppu.obp1 = val,
            0xFF4A => self.ppu.wy = val,
            0xFF4B => self.ppu.wx = val,
            0xFF4D if self.cgb => self.key1 = (self.key1 & 0x80) | (val & 0x01),
            0xFF4F if self.cgb => self.ppu.vram_bank = (val & 1) as usize,
            0xFF51 if self.cgb => self.hdma_src = (self.hdma_src & 0x00FF) | ((val as u16) << 8),
            0xFF52 if self.cgb => self.hdma_src = (self.hdma_src & 0xFF00) | (val as u16 & 0xF0),
            0xFF53 if self.cgb => {
                self.hdma_dst = (self.hdma_dst & 0x00FF) | (((val as u16) & 0x1F) << 8)
            }
            0xFF54 if self.cgb => self.hdma_dst = (self.hdma_dst & 0xFF00) | (val as u16 & 0xF0),
            0xFF55 if self.cgb => self.start_hdma(val),
            0xFF68 if self.cgb => self.ppu.bcps = val,
            0xFF69 if self.cgb => self.ppu.write_bcpd(val),
            0xFF6A if self.cgb => self.ppu.ocps = val,
            0xFF6B if self.cgb => self.ppu.write_ocpd(val),
            0xFF6C if self.cgb => self.opri = val & 1,
            0xFF70 if self.cgb => {
                let b = (val & 0x07) as usize;
                self.wram_bank = if b == 0 { 1 } else { b };
            }
            _ => {}
        }
    }

    /// OAM DMA (FF46). Performed instantly: the 160-cycle bus lockout it
    /// causes on hardware is not observable to any game that follows the
    /// documented "call a HRAM routine and wait" pattern.
    fn oam_dma(&mut self, high: u8) {
        let src = (high as u16) << 8;
        for i in 0..0xA0u16 {
            let b = self.read(src + i);
            self.ppu.oam[i as usize] = b;
        }
    }

    /// CGB VRAM DMA (FF55). Bit 7 selects HBlank mode (16 bytes per HBlank)
    /// vs general-purpose mode (whole block at once, CPU halted).
    fn start_hdma(&mut self, val: u8) {
        let length = ((val & 0x7F) as u16 + 1) * 16;
        if (val & 0x80) == 0 {
            if self.hdma_active {
                // Writing bit 7 = 0 during an active HBlank DMA cancels it.
                self.hdma_active = false;
                self.hdma_len |= 0x80;
                return;
            }
            // General purpose: copy immediately.
            for i in 0..length {
                let b = self.read(self.hdma_src.wrapping_add(i));
                let dst = 0x8000u16.wrapping_add(self.hdma_dst.wrapping_add(i) & 0x1FFF);
                self.ppu.write_vram(dst, b);
            }
            self.hdma_src = self.hdma_src.wrapping_add(length);
            self.hdma_dst = self.hdma_dst.wrapping_add(length);
            self.hdma_len = 0xFF;
            self.hdma_active = false;
        } else {
            self.hdma_len = val & 0x7F;
            self.hdma_active = true;
        }
    }

    /// Transfer one 16-byte block. Called by the system at each HBlank while
    /// an HBlank DMA is armed.
    pub fn hdma_step_hblank(&mut self) {
        if !self.hdma_active {
            return;
        }
        for i in 0..16u16 {
            let b = self.read(self.hdma_src.wrapping_add(i));
            let dst = 0x8000u16.wrapping_add(self.hdma_dst.wrapping_add(i) & 0x1FFF);
            self.ppu.write_vram(dst, b);
        }
        self.hdma_src = self.hdma_src.wrapping_add(16);
        self.hdma_dst = self.hdma_dst.wrapping_add(16);
        if self.hdma_len == 0 {
            self.hdma_active = false;
            self.hdma_len = 0xFF;
        } else {
            self.hdma_len -= 1;
        }
    }

    // --- timer ----------------------------------------------------------

    /// The TIMA increment is a *falling edge detector* on a selected bit of
    /// the 16-bit DIV counter -- not a simple divider. Emulating it as an
    /// edge detector is what makes a DIV write mid-period able to clock TIMA
    /// an extra time, which `instr_timing` and several games rely on.
    #[inline]
    fn tac_bit(&self) -> u16 {
        match self.tac & 0x03 {
            0 => 1 << 9,  // 4096 Hz
            1 => 1 << 3,  // 262144 Hz
            2 => 1 << 5,  // 65536 Hz
            _ => 1 << 7,  // 16384 Hz
        }
    }

    /// Step the timer by `cycles` T-cycles. Returns true when TIMA overflowed
    /// (the caller raises the timer interrupt).
    pub fn step_timer(&mut self, cycles: u32) -> bool {
        let mut irq = false;
        let bit = self.tac_bit();
        let enabled = (self.tac & 0x04) != 0;

        for _ in 0..cycles {
            let prev = self.div_counter;
            self.div_counter = self.div_counter.wrapping_add(1);

            if self.tima_overflow_pending {
                // TIMA reload is delayed one M-cycle after overflow.
                self.tima = self.tma;
                self.tima_overflow_pending = false;
                irq = true;
            }

            if enabled {
                let fell = (prev & bit) != 0 && (self.div_counter & bit) == 0;
                if fell {
                    let (v, of) = self.tima.overflowing_add(1);
                    self.tima = v;
                    if of {
                        self.tima = 0;
                        self.tima_overflow_pending = true;
                    }
                }
            }
        }
        irq
    }

    /// Step the serial link. With no cable attached the shifted-in bits are
    /// all 1s; the byte that was shifted out is captured in `serial_out`.
    pub fn step_serial(&mut self, cycles: u32) -> bool {
        if self.serial_counter == 0 {
            return false;
        }
        if self.serial_counter > cycles {
            self.serial_counter -= cycles;
            return false;
        }
        self.serial_counter = 0;
        self.serial_out.push(self.sb);
        self.sb = 0xFF;
        self.sc &= !0x80;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mmu() -> GbMmu {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x00;
        GbMmu::new(rom, false)
    }

    #[test]
    fn echo_ram_mirrors_work_ram() {
        let mut m = mmu();
        m.write(0xC000, 0x5A);
        assert_eq!(m.read(0xE000), 0x5A);
        m.write(0xE100, 0xA5);
        assert_eq!(m.read(0xC100), 0xA5);
    }

    #[test]
    fn unusable_region_reads_zero_and_ignores_writes() {
        let mut m = mmu();
        m.write(0xFEA0, 0xFF);
        assert_eq!(m.read(0xFEA0), 0x00);
    }

    #[test]
    fn div_write_resets_the_whole_counter() {
        let mut m = mmu();
        m.step_timer(600);
        assert_ne!(m.read(0xFF04), 0);
        m.write(0xFF04, 0x00);
        assert_eq!(m.read(0xFF04), 0);
    }

    #[test]
    fn tima_increments_at_the_tac_rate_and_reloads_from_tma() {
        let mut m = mmu();
        m.div_counter = 0;
        m.tma = 0xAB;
        m.write(0xFF07, 0x05); // enable, 262144 Hz (every 16 T-cycles)
        m.write(0xFF05, 0xFF);
        let mut irq = false;
        for _ in 0..20 {
            irq |= m.step_timer(1);
        }
        assert!(irq, "TIMA overflow raises the timer interrupt");
        assert_eq!(m.tima, 0xAB, "TIMA reloads from TMA");
    }

    #[test]
    fn tima_keeps_counting_after_the_tma_reload() {
        // The reload is not a stop: the next falling edge increments again.
        let mut m = mmu();
        m.div_counter = 0;
        m.tma = 0xAB;
        m.write(0xFF07, 0x05);
        m.write(0xFF05, 0xFF);
        m.step_timer(40);
        assert_eq!(m.tima, 0xAC);
    }

    #[test]
    fn timer_is_stopped_when_tac_enable_is_clear() {
        let mut m = mmu();
        m.div_counter = 0;
        m.write(0xFF07, 0x01); // rate set but enable bit clear
        m.write(0xFF05, 0x00);
        m.step_timer(1000);
        assert_eq!(m.tima, 0);
    }

    #[test]
    fn joypad_reports_active_low_per_selected_nibble() {
        let mut m = mmu();
        m.set_key(GbKey::A, true);
        m.write(0xFF00, 0x10); // select action buttons (P15 low? no: bit4=1 -> directions deselected)
        // bit5 = 0 selects actions
        m.write(0xFF00, 0x10);
        assert_eq!(m.read(0xFF00) & 0x01, 0, "A reads as 0 when pressed");
        m.write(0xFF00, 0x20); // select directions
        assert_eq!(m.read(0xFF00) & 0x0F, 0x0F, "no direction pressed");
    }

    #[test]
    fn pressing_a_button_raises_the_joypad_interrupt() {
        let mut m = mmu();
        m.if_reg = 0;
        m.set_key(GbKey::Start, true);
        assert_eq!(m.if_reg & 0x10, 0x10);
    }

    #[test]
    fn oam_dma_copies_160_bytes() {
        let mut m = mmu();
        for i in 0..0xA0u16 {
            m.write(0xC000 + i, i as u8);
        }
        m.write(0xFF46, 0xC0);
        assert_eq!(m.ppu.oam[0], 0);
        assert_eq!(m.ppu.oam[0x9F], 0x9F);
    }

    #[test]
    fn cgb_wram_banking_switches_the_d000_window() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = 0xC0;
        let mut m = GbMmu::new(rom, true);
        m.write(0xFF70, 0x01);
        m.write(0xD000, 0x11);
        m.write(0xFF70, 0x02);
        m.write(0xD000, 0x22);
        assert_eq!(m.read(0xD000), 0x22);
        m.write(0xFF70, 0x01);
        assert_eq!(m.read(0xD000), 0x11);
    }

    #[test]
    fn cgb_wram_bank_zero_selects_bank_one() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = 0xC0;
        let mut m = GbMmu::new(rom, true);
        m.write(0xFF70, 0x00);
        assert_eq!(m.wram_bank, 1);
    }

    #[test]
    fn cgb_general_purpose_hdma_copies_immediately() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = 0xC0;
        let mut m = GbMmu::new(rom, true);
        for i in 0..32u16 {
            m.write(0xC000 + i, (i + 1) as u8);
        }
        m.write(0xFF51, 0xC0);
        m.write(0xFF52, 0x00);
        m.write(0xFF53, 0x00);
        m.write(0xFF54, 0x00);
        m.write(0xFF55, 0x01); // 2 blocks = 32 bytes, general purpose
        assert_eq!(m.ppu.vram[0][0], 1);
        assert_eq!(m.ppu.vram[0][31], 32);
        assert!(!m.hdma_active);
    }

    #[test]
    fn cgb_hblank_hdma_transfers_one_block_per_hblank() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = 0xC0;
        let mut m = GbMmu::new(rom, true);
        for i in 0..32u16 {
            m.write(0xC000 + i, (i + 1) as u8);
        }
        m.write(0xFF51, 0xC0);
        m.write(0xFF52, 0x00);
        m.write(0xFF53, 0x00);
        m.write(0xFF54, 0x00);
        m.write(0xFF55, 0x81); // 2 blocks, HBlank mode
        assert!(m.hdma_active);
        assert_eq!(m.ppu.vram[0][0], 0, "nothing copied yet");
        m.hdma_step_hblank();
        assert_eq!(m.ppu.vram[0][15], 16);
        assert_eq!(m.ppu.vram[0][16], 0);
        m.hdma_step_hblank();
        assert_eq!(m.ppu.vram[0][31], 32);
        assert!(!m.hdma_active, "finished after 2 blocks");
    }

    #[test]
    fn serial_transfer_captures_the_outgoing_byte() {
        let mut m = mmu();
        m.write(0xFF01, 0x42);
        m.write(0xFF02, 0x81);
        let fired = m.step_serial(10_000);
        assert!(fired);
        assert_eq!(m.serial_out, vec![0x42]);
        assert_eq!(m.sc & 0x80, 0, "transfer-start bit cleared");
    }
}
