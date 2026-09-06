//! GBA Memory Management Unit (MMU) & System Bus Dispatcher

pub mod bios;
pub mod cartridge;
pub mod flash;
pub mod rtc;
pub mod sensors;
pub mod sio;

use super::apu::Apu;
use super::cpu::Arm7Tdmi;
use super::dma::DmaController;
use super::keypad::Keypad;
use super::ppu::Ppu;
use super::timer::TimerController;
use cartridge::Cartridge;
use sio::Sio;

pub struct Mmu {
    pub bios: Box<[u8; 16 * 1024]>,
    pub ewram: Box<[u8; 256 * 1024]>,
    pub iwram: Box<[u8; 32 * 1024]>,
    pub io_regs: Box<[u8; 1024]>,

    pub cartridge: Option<Cartridge>,
    pub ppu: Ppu,
    pub dma: DmaController,
    pub timers: TimerController,
    pub apu: Apu,
    pub keypad: Keypad,
    pub sio: Sio,

    pub ime: bool,
    pub ie: u16,
    pub if_reg: u16,
    pub waitcnt: u16,
    pub post_flg: u8,
    pub haltcnt: u8,

    pub last_read: u32,
}

impl Default for Mmu {
    fn default() -> Self {
        Self::new()
    }
}

impl Mmu {
    pub fn new() -> Self {
        let mut bios = Box::new([0u8; 16 * 1024]);
        // GBA BIOS IRQ Vector & Dispatcher at 0x0000_0018
        // When an interrupt fires, CPU vectors to 0x18 in IRQ mode.
        // The BIOS saves registers, jumps to [0x03007FFC] (user handler),
        // and on return restores registers and returns via subs pc, lr, #4.
        let irq_code: [u32; 6] = [
            0xE92D_500F, // 0x18: stmfd sp!, {r0-r3, r12, lr}
            0xE3A0_0301, // 0x1C: mov r0, #0x04000000
            0xE28F_E000, // 0x20: add lr, pc, #0
            0xE510_F004, // 0x24: ldr pc, [r0, #-4] (reads [0x03007FFC])
            0xE8BD_500F, // 0x28: ldmfd sp!, {r0-r3, r12, lr}
            0xE25E_F004, // 0x2C: subs pc, lr, #4
        ];
        for (i, &instr) in irq_code.iter().enumerate() {
            let offset = 0x18 + i * 4;
            bios[offset..offset + 4].copy_from_slice(&instr.to_le_bytes());
        }

        Self {
            bios,
            ewram: vec![0u8; 256 * 1024].into_boxed_slice().try_into().unwrap(),
            iwram: vec![0u8; 32 * 1024].into_boxed_slice().try_into().unwrap(),
            io_regs: Box::new([0; 1024]),
            cartridge: None,
            ppu: Ppu::new(),
            dma: DmaController::new(),
            timers: TimerController::new(),
            apu: Apu::new(),
            keypad: Keypad::new(),
            sio: Sio::new(),
            ime: false,
            ie: 0,
            if_reg: 0,
            waitcnt: 0,
            post_flg: 0,
            haltcnt: 0,
            last_read: 0,
        }
    }

    pub fn load_cartridge(&mut self, cart: Cartridge) {
        self.cartridge = Some(cart);
    }

    #[inline(always)]
    pub fn read8(&self, addr: u32) -> u8 {
        match (addr >> 24) & 0xFF {
            0x00 => {
                let off = (addr & 0x3FFF) as usize;
                self.bios[off]
            }
            0x02 => {
                let off = (addr & 0x3_FFFF) as usize;
                self.ewram[off]
            }
            0x03 => {
                let off = (addr & 0x7FFF) as usize;
                self.iwram[off]
            }
            0x04 => self.read_io8(addr),
            0x05 => {
                let off = (addr & 0x3FF) as usize;
                self.ppu.palette_ram[off]
            }
            0x06 => {
                let mut off = (addr & 0x1_FFFF) as usize;
                if off >= 0x18000 {
                    off -= 0x8000;
                }
                self.ppu.vram[off]
            }
            0x07 => {
                let off = (addr & 0x3FF) as usize;
                self.ppu.oam[off]
            }
            0x08..=0x0F => {
                if let Some(ref cart) = self.cartridge {
                    cart.read8(addr)
                } else {
                    0
                }
            }
            _ => (self.last_read & 0xFF) as u8,
        }
    }

    #[inline(always)]
    pub fn read16(&self, addr: u32) -> u16 {
        let aligned = addr & !1;
        let b0 = self.read8(aligned) as u16;
        let b1 = self.read8(aligned + 1) as u16;
        let val = b0 | (b1 << 8);

        if (addr & 1) != 0 {
            // Unaligned 16-bit read rotates right by 8
            val.rotate_right(8)
        } else {
            val
        }
    }

    #[inline(always)]
    pub fn read32(&self, addr: u32) -> u32 {
        let aligned = addr & !3;
        let b0 = self.read8(aligned) as u32;
        let b1 = self.read8(aligned + 1) as u32;
        let b2 = self.read8(aligned + 2) as u32;
        let b3 = self.read8(aligned + 3) as u32;
        let val = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);

        let unaligned_offset = (addr & 3) * 8;
        if unaligned_offset != 0 {
            val.rotate_right(unaligned_offset)
        } else {
            val
        }
    }

    #[inline(always)]
    pub fn write8(&mut self, addr: u32, val: u8) {
        match (addr >> 24) & 0xFF {
            0x02 => {
                let off = (addr & 0x3_FFFF) as usize;
                self.ewram[off] = val;
            }
            0x03 => {
                let off = (addr & 0x7FFF) as usize;
                self.iwram[off] = val;
            }
            0x04 => self.write_io8(addr, val),
            0x05 => {
                // Byte writes to palette RAM write to both bytes of the halfword
                let off = (addr & 0x3FE) as usize;
                self.ppu.palette_ram[off] = val;
                self.ppu.palette_ram[off + 1] = val;
            }
            0x06 => {
                let off_raw = addr & 0x1_FFFF;
                // On GBA, 8-bit writes to OBJ VRAM (>= 0x10000) are ignored
                if off_raw >= 0x10000 {
                    return;
                }
                // Byte writes to BG VRAM write to both bytes of the halfword
                let mut off = (addr & 0x1_FFFE) as usize;
                if off >= 0x18000 {
                    off -= 0x8000;
                }
                self.ppu.vram[off] = val;
                self.ppu.vram[off + 1] = val;
            }
            0x07 => {
                // OAM byte writes are ignored on real GBA hardware
            }
            0x08..=0x0F => {
                if let Some(ref mut cart) = self.cartridge {
                    cart.write8(addr, val);
                }
            }
            _ => {}
        }
    }

    #[inline(always)]
    pub fn write16(&mut self, addr: u32, val: u16) {
        let aligned = addr & !1;
        match (aligned >> 24) & 0xFF {
            0x04 => self.write_io16(aligned, val),
            0x05 => {
                let off = (aligned & 0x3FE) as usize;
                self.ppu.palette_ram[off] = (val & 0xFF) as u8;
                self.ppu.palette_ram[off + 1] = (val >> 8) as u8;
            }
            0x06 => {
                let mut off = (aligned & 0x1_FFFE) as usize;
                if off >= 0x18000 {
                    off -= 0x8000;
                }
                self.ppu.vram[off] = (val & 0xFF) as u8;
                self.ppu.vram[off + 1] = (val >> 8) as u8;
            }
            0x07 => {
                let off = (aligned & 0x3FE) as usize;
                self.ppu.oam[off] = (val & 0xFF) as u8;
                self.ppu.oam[off + 1] = (val >> 8) as u8;
            }
            _ => {
                self.write8(aligned, (val & 0xFF) as u8);
                self.write8(aligned + 1, (val >> 8) as u8);
            }
        }
    }

    #[inline(always)]
    pub fn write32(&mut self, addr: u32, val: u32) {
        let aligned = addr & !3;
        self.write16(aligned, (val & 0xFFFF) as u16);
        self.write16(aligned + 2, (val >> 16) as u16);
    }

    fn read_io8(&self, addr: u32) -> u8 {
        let off = addr & 0x3FF;
        let is_high = (off & 1) != 0;
        let val16 = self.read_io16(off & !1);
        if is_high {
            (val16 >> 8) as u8
        } else {
            (val16 & 0xFF) as u8
        }
    }

    fn read_io16(&self, addr: u32) -> u16 {
        match addr & 0x3FE {
            0x000 => self.ppu.dispcnt,
            0x004 => self.ppu.dispstat,
            0x006 => self.ppu.vcount,
            0x008 => self.ppu.bgcnt[0],
            0x00A => self.ppu.bgcnt[1],
            0x00C => self.ppu.bgcnt[2],
            0x00E => self.ppu.bgcnt[3],
            0x080 => self.apu.soundcnt_l,
            0x082 => self.apu.soundcnt_h,
            0x084 => self.apu.soundcnt_x,
            0x088 => self.apu.soundbias,
            0x0B0 => self.dma.channels[0].sad as u16,
            0x0B2 => (self.dma.channels[0].sad >> 16) as u16,
            0x0B4 => self.dma.channels[0].dad as u16,
            0x0B6 => (self.dma.channels[0].dad >> 16) as u16,
            0x0B8 => self.dma.channels[0].count,
            0x0BA => self.dma.channels[0].cnt_h,
            0x0BC => self.dma.channels[1].sad as u16,
            0x0BE => (self.dma.channels[1].sad >> 16) as u16,
            0x0C0 => self.dma.channels[1].dad as u16,
            0x0C2 => (self.dma.channels[1].dad >> 16) as u16,
            0x0C4 => self.dma.channels[1].count,
            0x0C6 => self.dma.channels[1].cnt_h,
            0x0C8 => self.dma.channels[2].sad as u16,
            0x0CA => (self.dma.channels[2].sad >> 16) as u16,
            0x0CC => self.dma.channels[2].dad as u16,
            0x0CE => (self.dma.channels[2].dad >> 16) as u16,
            0x0D0 => self.dma.channels[2].count,
            0x0D2 => self.dma.channels[2].cnt_h,
            0x0D4 => self.dma.channels[3].sad as u16,
            0x0D6 => (self.dma.channels[3].sad >> 16) as u16,
            0x0D8 => self.dma.channels[3].dad as u16,
            0x0DA => (self.dma.channels[3].dad >> 16) as u16,
            0x0DC => self.dma.channels[3].count,
            0x0DE => self.dma.channels[3].cnt_h,
            0x100 => self.timers.timers[0].counter,
            0x102 => self.timers.timers[0].cnt_h,
            0x104 => self.timers.timers[1].counter,
            0x106 => self.timers.timers[1].cnt_h,
            0x108 => self.timers.timers[2].counter,
            0x10A => self.timers.timers[2].cnt_h,
            0x10C => self.timers.timers[3].counter,
            0x10E => self.timers.timers[3].cnt_h,
            0x120..=0x12E | 0x134 => self.sio.read_io16(addr & 0x3FE),
            0x130 => self.keypad.read_keyinput(),
            0x132 => self.keypad.keycnt,
            0x200 => self.ie,
            0x202 => self.if_reg,
            0x204 => self.waitcnt,
            0x208 => self.ime as u16,
            0x300 => (self.post_flg as u16) | ((self.haltcnt as u16) << 8),
            _ => {
                let off = (addr & 0x3FE) as usize;
                (self.io_regs[off] as u16) | ((self.io_regs[off + 1] as u16) << 8)
            }
        }
    }

    fn write_io8(&mut self, addr: u32, val: u8) {
        let off = addr & 0x3FF;
        match off {
            0x0A0..=0x0A3 => {
                // DirectSound FIFO A
                self.apu.sound_a.push_byte(val);
            }
            0x0A4..=0x0A7 => {
                // DirectSound FIFO B
                self.apu.sound_b.push_byte(val);
            }
            0x208 => self.ime = (val & 1) != 0,
            0x300 => self.post_flg = val,
            0x301 => self.haltcnt = val,
            _ => {
                let aligned = off & !1;
                let cur = self.read_io16(aligned);
                let new_val = if (off & 1) != 0 {
                    (cur & 0x00FF) | ((val as u16) << 8)
                } else {
                    (cur & 0xFF00) | (val as u16)
                };
                self.write_io16(aligned, new_val);
            }
        }
    }

    fn write_io16(&mut self, addr: u32, val: u16) {
        match addr & 0x3FE {
            0x000 => self.ppu.dispcnt = val,
            0x004 => {
                // Bits 0..2 are read only in DISPSTAT
                self.ppu.dispstat = (self.ppu.dispstat & 7) | (val & !7);
            }
            0x008 => self.ppu.bgcnt[0] = val,
            0x00A => self.ppu.bgcnt[1] = val,
            0x00C => self.ppu.bgcnt[2] = val,
            0x00E => self.ppu.bgcnt[3] = val,
            0x010 => self.ppu.bghofs[0] = val & 0x1FF,
            0x012 => self.ppu.bgvofs[0] = val & 0x1FF,
            0x014 => self.ppu.bghofs[1] = val & 0x1FF,
            0x016 => self.ppu.bgvofs[1] = val & 0x1FF,
            0x018 => self.ppu.bghofs[2] = val & 0x1FF,
            0x01A => self.ppu.bgvofs[2] = val & 0x1FF,
            0x01C => self.ppu.bghofs[3] = val & 0x1FF,
            0x01E => self.ppu.bgvofs[3] = val & 0x1FF,
            0x020 => self.ppu.bg_pa[0] = val as i16,
            0x022 => self.ppu.bg_pb[0] = val as i16,
            0x024 => self.ppu.bg_pc[0] = val as i16,
            0x026 => self.ppu.bg_pd[0] = val as i16,
            0x028 => self.ppu.bg_x[0] = (self.ppu.bg_x[0] & !0xFFFF) | (val as i32),
            0x02A => {
                let sign_ext = ((val as i16) as i32) << 16;
                self.ppu.bg_x[0] = (self.ppu.bg_x[0] & 0xFFFF) | sign_ext;
                self.ppu.bg_x_internal[0] = self.ppu.bg_x[0];
            }
            0x02C => self.ppu.bg_y[0] = (self.ppu.bg_y[0] & !0xFFFF) | (val as i32),
            0x02E => {
                let sign_ext = ((val as i16) as i32) << 16;
                self.ppu.bg_y[0] = (self.ppu.bg_y[0] & 0xFFFF) | sign_ext;
                self.ppu.bg_y_internal[0] = self.ppu.bg_y[0];
            }
            0x030 => self.ppu.bg_pa[1] = val as i16,
            0x032 => self.ppu.bg_pb[1] = val as i16,
            0x034 => self.ppu.bg_pc[1] = val as i16,
            0x036 => self.ppu.bg_pd[1] = val as i16,
            0x038 => self.ppu.bg_x[1] = (self.ppu.bg_x[1] & !0xFFFF) | (val as i32),
            0x03A => {
                let sign_ext = ((val as i16) as i32) << 16;
                self.ppu.bg_x[1] = (self.ppu.bg_x[1] & 0xFFFF) | sign_ext;
                self.ppu.bg_x_internal[1] = self.ppu.bg_x[1];
            }
            0x03C => self.ppu.bg_y[1] = (self.ppu.bg_y[1] & !0xFFFF) | (val as i32),
            0x03E => {
                let sign_ext = ((val as i16) as i32) << 16;
                self.ppu.bg_y[1] = (self.ppu.bg_y[1] & 0xFFFF) | sign_ext;
                self.ppu.bg_y_internal[1] = self.ppu.bg_y[1];
            }
            0x040 => self.ppu.win0h = val,
            0x042 => self.ppu.win1h = val,
            0x044 => self.ppu.win0v = val,
            0x046 => self.ppu.win1v = val,
            0x048 => self.ppu.winin = val,
            0x04A => self.ppu.winout = val,
            0x050 => self.ppu.bldcnt = val,
            0x052 => self.ppu.bldalpha = val,
            0x054 => self.ppu.bldy = val,
            0x080 => self.apu.soundcnt_l = val,
            0x082 => self.apu.write_soundcnt_h(val),
            0x084 => self.apu.soundcnt_x = val,
            0x088 => self.apu.soundbias = val,
            0x0A0 | 0x0A2 => {
                self.apu.sound_a.push_byte((val & 0xFF) as u8);
                self.apu.sound_a.push_byte((val >> 8) as u8);
            }
            0x0A4 | 0x0A6 => {
                self.apu.sound_b.push_byte((val & 0xFF) as u8);
                self.apu.sound_b.push_byte((val >> 8) as u8);
            }
            0x0B0 => self.dma.channels[0].sad = (self.dma.channels[0].sad & !0xFFFF) | (val as u32),
            0x0B2 => self.dma.channels[0].sad = (self.dma.channels[0].sad & 0xFFFF) | ((val as u32) << 16),
            0x0B4 => self.dma.channels[0].dad = (self.dma.channels[0].dad & !0xFFFF) | (val as u32),
            0x0B6 => self.dma.channels[0].dad = (self.dma.channels[0].dad & 0xFFFF) | ((val as u32) << 16),
            0x0B8 => self.dma.channels[0].count = val,
            0x0BA => {
                if self.dma.channels[0].write_cnt_h(val, false) {
                    self.execute_dma_channel(0);
                }
            }
            0x0BC => self.dma.channels[1].sad = (self.dma.channels[1].sad & !0xFFFF) | (val as u32),
            0x0BE => self.dma.channels[1].sad = (self.dma.channels[1].sad & 0xFFFF) | ((val as u32) << 16),
            0x0C0 => self.dma.channels[1].dad = (self.dma.channels[1].dad & !0xFFFF) | (val as u32),
            0x0C2 => self.dma.channels[1].dad = (self.dma.channels[1].dad & 0xFFFF) | ((val as u32) << 16),
            0x0C4 => self.dma.channels[1].count = val,
            0x0C6 => {
                if self.dma.channels[1].write_cnt_h(val, false) {
                    self.execute_dma_channel(1);
                }
            }
            0x0C8 => self.dma.channels[2].sad = (self.dma.channels[2].sad & !0xFFFF) | (val as u32),
            0x0CA => self.dma.channels[2].sad = (self.dma.channels[2].sad & 0xFFFF) | ((val as u32) << 16),
            0x0CC => self.dma.channels[2].dad = (self.dma.channels[2].dad & !0xFFFF) | (val as u32),
            0x0CE => self.dma.channels[2].dad = (self.dma.channels[2].dad & 0xFFFF) | ((val as u32) << 16),
            0x0D0 => self.dma.channels[2].count = val,
            0x0D2 => {
                if self.dma.channels[2].write_cnt_h(val, false) {
                    self.execute_dma_channel(2);
                }
            }
            0x0D4 => self.dma.channels[3].sad = (self.dma.channels[3].sad & !0xFFFF) | (val as u32),
            0x0D6 => self.dma.channels[3].sad = (self.dma.channels[3].sad & 0xFFFF) | ((val as u32) << 16),
            0x0D8 => self.dma.channels[3].dad = (self.dma.channels[3].dad & !0xFFFF) | (val as u32),
            0x0DA => self.dma.channels[3].dad = (self.dma.channels[3].dad & 0xFFFF) | ((val as u32) << 16),
            0x0DC => self.dma.channels[3].count = val,
            0x0DE => {
                if self.dma.channels[3].write_cnt_h(val, true) {
                    self.execute_dma_channel(3);
                }
            }
            0x100 => self.timers.timers[0].reload = val,
            0x102 => self.timers.timers[0].write_cnt_h(val),
            0x104 => self.timers.timers[1].reload = val,
            0x106 => self.timers.timers[1].write_cnt_h(val),
            0x108 => self.timers.timers[2].reload = val,
            0x10A => self.timers.timers[2].write_cnt_h(val),
            0x10C => self.timers.timers[3].reload = val,
            0x10E => self.timers.timers[3].write_cnt_h(val),
            0x120..=0x12E | 0x134 => self.sio.write_io16(addr & 0x3FE, val),
            0x130 => {} // KEYINPUT is read only
            0x132 => self.keypad.keycnt = val,
            0x200 => self.ie = val,
            0x202 => {
                // Writing 1s to IF clears the corresponding interrupt flags
                self.if_reg &= !val;
            }
            0x204 => self.waitcnt = val,
            0x208 => self.ime = (val & 1) != 0,
            0x300 => {
                self.post_flg = (val & 0xFF) as u8;
                self.haltcnt = (val >> 8) as u8;
            }
            _ => {
                let off = (addr & 0x3FE) as usize;
                self.io_regs[off] = (val & 0xFF) as u8;
                self.io_regs[off + 1] = (val >> 8) as u8;
            }
        }
    }

    pub fn execute_dma_channel(&mut self, idx: usize) {
        let (is_32bit, dad_ctrl, sad_ctrl, repeat, irq_on_finish, count, current_dad, current_sad, _is_direct_sound) = {
            let ch = &self.dma.channels[idx];
            if !ch.enabled {
                return;
            }

            let timing = (ch.cnt_h >> 12) & 3;
            let is_direct_sound = (idx == 1 || idx == 2) && timing == 3;

            // DirectSound FIFO DMA always transfers 4 32-bit words (16 bytes)
            let is_32bit = if is_direct_sound {
                true
            } else {
                (ch.cnt_h & (1 << 10)) != 0
            };

            let dad_ctrl = if is_direct_sound {
                2 // Fixed destination address for DirectSound FIFO
            } else {
                (ch.cnt_h >> 5) & 3
            };

            let sad_ctrl = (ch.cnt_h >> 7) & 3;
            let repeat = (ch.cnt_h & (1 << 9)) != 0;
            let irq_on_finish = (ch.cnt_h & (1 << 14)) != 0;

            let count = if is_direct_sound {
                4
            } else {
                let max_cnt = if idx == 3 { 0x10000 } else { 0x4000 };
                let cnt = (ch.count as u32) & (max_cnt - 1);
                if cnt == 0 { max_cnt } else { cnt }
            };

            let current_dad = if is_direct_sound {
                if (ch.dad & !3) == 0x0400_00A4 {
                    0x0400_00A4
                } else {
                    0x0400_00A0
                }
            } else {
                ch.internal_dad
            };

            (is_32bit, dad_ctrl, sad_ctrl, repeat, irq_on_finish, count, current_dad, ch.internal_sad, is_direct_sound)
        };

        let step_size = if is_32bit { 4 } else { 2 };
        let mut sad = current_sad;
        let mut dad = current_dad;

        for _ in 0..count {
            if is_32bit {
                let val = self.read32(sad);
                self.write32(dad, val);
            } else {
                let val = self.read16(sad);
                self.write16(dad, val);
            }

            match sad_ctrl {
                0 => sad = sad.wrapping_add(step_size),
                1 => sad = sad.wrapping_sub(step_size),
                _ => {} // Fixed
            }

            match dad_ctrl {
                0 => dad = dad.wrapping_add(step_size),
                1 => dad = dad.wrapping_sub(step_size),
                2 => {} // Fixed
                3 => dad = dad.wrapping_add(step_size),
                _ => {}
            }
        }

        // Advance internal working registers!
        self.dma.channels[idx].internal_sad = sad;
        self.dma.channels[idx].internal_dad = dad;

        let timing = (self.dma.channels[idx].cnt_h >> 12) & 3;
        let is_direct_sound = (idx == 1 || idx == 2) && timing == 3;

        if is_direct_sound {
            // DirectSound repeats until explicitly disabled by software.
            if !repeat {
                self.dma.channels[idx].enabled = false;
                self.dma.channels[idx].cnt_h &= !(1 << 15);
            }
        } else if repeat {
            if dad_ctrl == 3 {
                self.dma.channels[idx].internal_dad = self.dma.channels[idx].dad;
            }
        } else {
            self.dma.channels[idx].enabled = false;
            self.dma.channels[idx].cnt_h &= !(1 << 15);
        }

        if irq_on_finish {
            self.request_interrupt(8 + idx as u16);
        }
    }

    pub fn request_interrupt(&mut self, irq_bit: u16) {
        self.if_reg |= 1 << irq_bit;
    }

    pub fn has_pending_irq(&self) -> bool {
        self.ime && ((self.ie & self.if_reg) != 0)
    }

    pub fn handle_swi(&mut self, cpu: &mut Arm7Tdmi, comment: u32) {
        let swi_num = if comment >= 0x10000 {
            ((comment >> 16) & 0xFF) as u8
        } else {
            (comment & 0xFF) as u8
        };
        // In GBA, BIOS functions 0x00..0x2A can be handled via HLE BIOS
        if swi_num <= 0x2A {
            let mut writes_8: Vec<(u32, u8)> = Vec::new();
            let mut writes_16: Vec<(u32, u16)> = Vec::new();
            let mut writes_32: Vec<(u32, u32)> = Vec::new();

            {
                let read_8 = |addr: u32| -> u8 { self.read8(addr) };
                let mut write_8 = |addr: u32, val: u8| { writes_8.push((addr, val)); };
                let mut write_16 = |addr: u32, val: u16| { writes_16.push((addr, val)); };
                let mut write_32 = |addr: u32, val: u32| { writes_32.push((addr, val)); };

                bios::execute_hle_swi(swi_num, cpu, &read_8, &mut write_8, &mut write_16, &mut write_32);
            }

            for (addr, val) in writes_8 {
                self.write8(addr, val);
            }
            for (addr, val) in writes_16 {
                self.write16(addr, val);
            }
            for (addr, val) in writes_32 {
                self.write32(addr, val);
            }
        } else {
            cpu.trigger_swi(comment);
        }
    }
}
