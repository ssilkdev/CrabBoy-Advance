//! GBA Direct Memory Access (DMA) Controller

pub struct DmaChannel {
    pub sad: u32,
    pub dad: u32,
    pub count: u16,
    pub cnt_h: u16,

    // Internal working copies
    pub internal_sad: u32,
    pub internal_dad: u32,
    pub internal_count: u32,
    pub enabled: bool,
}

impl Default for DmaChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaChannel {
    pub fn new() -> Self {
        Self {
            sad: 0,
            dad: 0,
            count: 0,
            cnt_h: 0,
            internal_sad: 0,
            internal_dad: 0,
            internal_count: 0,
            enabled: false,
        }
    }

    pub fn write_cnt_h(&mut self, val: u16, is_dma3: bool) -> bool {
        self.cnt_h = val;
        self.enabled = (val & (1 << 15)) != 0;

        if self.enabled {
            self.internal_sad = self.sad;
            self.internal_dad = self.dad;
            let max_cnt = if is_dma3 { 0x10000 } else { 0x4000 };
            let cnt = (self.count as u32) & (max_cnt - 1);
            self.internal_count = if cnt == 0 { max_cnt } else { cnt };
        }

        let start_immediate = self.enabled && ((val >> 12) & 3) == 0;
        start_immediate
    }
}

pub struct DmaController {
    pub channels: [DmaChannel; 4],
}

impl Default for DmaController {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaController {
    pub fn new() -> Self {
        Self {
            channels: [
                DmaChannel::new(),
                DmaChannel::new(),
                DmaChannel::new(),
                DmaChannel::new(),
            ],
        }
    }

    /// Triggers pending DMAs matching start_timing (1=VBlank, 2=HBlank, 3=Special)
    pub fn trigger(&mut self, timing: u16) -> Vec<usize> {
        let mut triggered = Vec::new();
        for i in 0..4 {
            let ch = &self.channels[i];
            if ch.enabled && ((ch.cnt_h >> 12) & 3) == timing {
                triggered.push(i);
            }
        }
        triggered
    }

    /// Executes the transfer for channel `idx` using supplied bus read/write callbacks
    pub fn execute_channel(
        &mut self,
        idx: usize,
        read_mem16: &impl Fn(u32) -> u16,
        read_mem32: &impl Fn(u32) -> u32,
        write_mem16: &mut impl FnMut(u32, u16),
        write_mem32: &mut impl FnMut(u32, u32),
    ) -> bool {
        let ch = &mut self.channels[idx];
        if !ch.enabled {
            return false;
        }

        let is_32bit = (ch.cnt_h & (1 << 10)) != 0;
        let dad_ctrl = (ch.cnt_h >> 5) & 3;
        let sad_ctrl = (ch.cnt_h >> 7) & 3;
        let repeat = (ch.cnt_h & (1 << 9)) != 0;
        let irq_on_finish = (ch.cnt_h & (1 << 14)) != 0;

        let step_size = if is_32bit { 4 } else { 2 };
        let count = ch.internal_count;

        for _ in 0..count {
            if is_32bit {
                let val = read_mem32(ch.internal_sad);
                write_mem32(ch.internal_dad, val);
            } else {
                let val = read_mem16(ch.internal_sad);
                write_mem16(ch.internal_dad, val);
            }

            match sad_ctrl {
                0 => ch.internal_sad = ch.internal_sad.wrapping_add(step_size),
                1 => ch.internal_sad = ch.internal_sad.wrapping_sub(step_size),
                _ => {} // Fixed
            }

            match dad_ctrl {
                0 | 3 => ch.internal_dad = ch.internal_dad.wrapping_add(step_size),
                1 => ch.internal_dad = ch.internal_dad.wrapping_sub(step_size),
                _ => {} // Fixed
            }
        }

        if repeat {
            let max_cnt = if idx == 3 { 0x10000 } else { 0x4000 };
            let cnt = (ch.count as u32) & (max_cnt - 1);
            ch.internal_count = if cnt == 0 { max_cnt } else { cnt };
            if dad_ctrl == 3 {
                ch.internal_dad = ch.dad; // Reload dest
            }
        } else {
            ch.enabled = false;
            ch.cnt_h &= !(1 << 15);
        }

        irq_on_finish
    }
}
