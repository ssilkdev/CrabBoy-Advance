//! IO Bus Flight Data Recorder
//!
//! Maintains a zero-allocation circular trace buffer of recent hardware I/O accesses
//! (APU, PPU, DMA, Timers, Interrupts) with exact cycle timestamps and source PC addresses.
//! Enables AI agents and developers to pinpoint anomalies, re-trigger bugs, or register conflicts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum IoAccessSize {
    Byte = 8,
    Halfword = 16,
    Word = 32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IoEvent {
    pub cycle: u64,
    pub pc: u32,
    pub addr: u32,
    pub val: u32,
    pub size: u8,
    pub is_write: bool,
    pub reg_name: String,
}

pub const FLIGHT_RECORDER_CAPACITY: usize = 512;

#[derive(Clone, Debug)]
pub struct FlightRecorder {
    buffer: Vec<Option<IoEvent>>,
    head: usize,
    pub total_events: u64,
    pub enabled: bool,
}

impl Default for FlightRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl FlightRecorder {
    pub fn new() -> Self {
        Self {
            buffer: vec![None; FLIGHT_RECORDER_CAPACITY],
            head: 0,
            total_events: 0,
            enabled: true,
        }
    }

    pub fn record(&mut self, cycle: u64, pc: u32, addr: u32, val: u32, size: u8, is_write: bool) {
        if !self.enabled {
            return;
        }

        let reg_name = get_io_register_name(addr & 0x3FF).to_string();
        let event = IoEvent {
            cycle,
            pc,
            addr: addr & 0x3FF,
            val,
            size,
            is_write,
            reg_name,
        };

        self.buffer[self.head] = Some(event);
        self.head = (self.head + 1) % FLIGHT_RECORDER_CAPACITY;
        self.total_events += 1;
    }

    /// Returns recent recorded events in chronological order (oldest to newest)
    pub fn recent_events(&self, limit: usize) -> Vec<IoEvent> {
        let mut result = Vec::with_capacity(limit.min(FLIGHT_RECORDER_CAPACITY));
        let count = self.total_events.min(FLIGHT_RECORDER_CAPACITY as u64) as usize;
        let fetch = limit.min(count);

        let start_pos = if count < FLIGHT_RECORDER_CAPACITY {
            0
        } else {
            self.head
        };

        for i in 0..count {
            let idx = (start_pos + i) % FLIGHT_RECORDER_CAPACITY;
            if let Some(ref ev) = self.buffer[idx] {
                result.push(ev.clone());
            }
        }

        if result.len() > fetch {
            result.split_off(result.len() - fetch)
        } else {
            result
        }
    }

    pub fn clear(&mut self) {
        self.buffer.fill(None);
        self.head = 0;
        self.total_events = 0;
    }
}

/// Maps GBA IO register offsets (0x000..0x3FE) to official GBA hardware names
pub fn get_io_register_name(offset: u32) -> &'static str {
    match offset & 0x3FE {
        // PPU
        0x000 => "DISPCNT",
        0x004 => "DISPSTAT",
        0x006 => "VCOUNT",
        0x008 => "BG0CNT",
        0x00A => "BG1CNT",
        0x00C => "BG2CNT",
        0x00E => "BG3CNT",
        0x010 => "BG0HOFS",
        0x012 => "BG0VOFS",
        0x014 => "BG1HOFS",
        0x016 => "BG1VOFS",
        0x018 => "BG2HOFS",
        0x01A => "BG2VOFS",
        0x01C => "BG3HOFS",
        0x01E => "BG3VOFS",
        0x020 => "BG2PA",
        0x022 => "BG2PB",
        0x024 => "BG2PC",
        0x026 => "BG2PD",
        0x028 => "BG2X_L",
        0x02A => "BG2X_H",
        0x02C => "BG2Y_L",
        0x02E => "BG2Y_H",
        0x030 => "BG3PA",
        0x032 => "BG3PB",
        0x034 => "BG3PC",
        0x036 => "BG3PD",
        0x038 => "BG3X_L",
        0x03A => "BG3X_H",
        0x03C => "BG3Y_L",
        0x03E => "BG3Y_H",
        0x040 => "WIN0H",
        0x042 => "WIN1H",
        0x044 => "WIN0V",
        0x046 => "WIN1V",
        0x048 => "WININ",
        0x04A => "WINOUT",
        0x04C => "MOSAIC",
        0x050 => "BLDCNT",
        0x052 => "BLDALPHA",
        0x054 => "BLDY",

        // Sound / APU
        0x060 => "SOUND1CNT_L",
        0x062 => "SOUND1CNT_H",
        0x064 => "SOUND1CNT_X",
        0x068 => "SOUND2CNT_L",
        0x06C => "SOUND2CNT_H",
        0x070 => "SOUND3CNT_L",
        0x072 => "SOUND3CNT_H",
        0x074 => "SOUND3CNT_X",
        0x078 => "SOUND4CNT_L",
        0x07C => "SOUND4CNT_H",
        0x080 => "SOUNDCNT_L",
        0x082 => "SOUNDCNT_H",
        0x084 => "SOUNDCNT_X",
        0x088 => "SOUNDBIAS",
        0x090..=0x09E => "WAVE_RAM",
        0x0A0 | 0x0A2 => "FIFO_A",
        0x0A4 | 0x0A6 => "FIFO_B",

        // DMA
        0x0B0 => "DMA0SAD_L",
        0x0B2 => "DMA0SAD_H",
        0x0B4 => "DMA0DAD_L",
        0x0B6 => "DMA0DAD_H",
        0x0B8 => "DMA0CNT_L",
        0x0BA => "DMA0CNT_H",
        0x0BC => "DMA1SAD_L",
        0x0BE => "DMA1SAD_H",
        0x0C0 => "DMA1DAD_L",
        0x0C2 => "DMA1DAD_H",
        0x0C4 => "DMA1CNT_L",
        0x0C6 => "DMA1CNT_H",
        0x0C8 => "DMA2SAD_L",
        0x0CA => "DMA2SAD_H",
        0x0CC => "DMA2DAD_L",
        0x0CE => "DMA2DAD_H",
        0x0D0 => "DMA2CNT_L",
        0x0D2 => "DMA2CNT_H",
        0x0D4 => "DMA3SAD_L",
        0x0D6 => "DMA3SAD_H",
        0x0D8 => "DMA3DAD_L",
        0x0DA => "DMA3DAD_H",
        0x0DC => "DMA3CNT_L",
        0x0DE => "DMA3CNT_H",

        // Timers
        0x100 => "TM0CNT_L",
        0x102 => "TM0CNT_H",
        0x104 => "TM1CNT_L",
        0x106 => "TM1CNT_H",
        0x108 => "TM2CNT_L",
        0x10A => "TM2CNT_H",
        0x10C => "TM3CNT_L",
        0x10E => "TM3CNT_H",

        // Serial / Keypad / System
        0x120 => "SIODATA32_L",
        0x122 => "SIODATA32_H",
        0x128 => "SIOCNT",
        0x130 => "KEYINPUT",
        0x132 => "KEYCNT",
        0x134 => "RCNT",
        0x200 => "IE",
        0x202 => "IF",
        0x204 => "WAITCNT",
        0x208 => "IME",
        0x300 => "POSTFLG/HALTCNT",
        _ => "UNKNOWN_IO",
    }
}
