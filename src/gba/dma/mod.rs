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

        
        self.enabled && ((val >> 12) & 3) == 0
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

}
