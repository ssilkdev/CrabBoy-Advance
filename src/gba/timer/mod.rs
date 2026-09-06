//! GBA Hardware Timers (TM0 to TM3)

pub struct Timer {
    pub reload: u16,
    pub counter: u16,
    pub cnt_h: u16,
    pub enabled: bool,
    pub prescaler_cycles: u32,
    pub cycles_accum: u32,
    pub cascade: bool,
    pub irq_enable: bool,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    pub fn new() -> Self {
        Self {
            reload: 0,
            counter: 0,
            cnt_h: 0,
            enabled: false,
            prescaler_cycles: 1,
            cycles_accum: 0,
            cascade: false,
            irq_enable: false,
        }
    }

    pub fn write_cnt_h(&mut self, val: u16) {
        let was_enabled = self.enabled;
        self.cnt_h = val;
        self.enabled = (val & (1 << 7)) != 0;
        self.cascade = (val & (1 << 2)) != 0;
        self.irq_enable = (val & (1 << 6)) != 0;

        self.prescaler_cycles = match val & 3 {
            0 => 1,
            1 => 64,
            2 => 256,
            3 => 1024,
            _ => 1,
        };

        if !was_enabled && self.enabled {
            self.counter = self.reload;
            self.cycles_accum = 0;
        }
    }

    pub fn step(&mut self, cycles: u32) -> bool {
        if !self.enabled || self.cascade {
            return false;
        }

        self.cycles_accum += cycles;
        let mut overflowed = false;

        while self.cycles_accum >= self.prescaler_cycles {
            self.cycles_accum -= self.prescaler_cycles;
            if self.counter == 0xFFFF {
                self.counter = self.reload;
                overflowed = true;
            } else {
                self.counter += 1;
            }
        }

        overflowed
    }

    pub fn increment_cascade(&mut self) -> bool {
        if !self.enabled || !self.cascade {
            return false;
        }

        if self.counter == 0xFFFF {
            self.counter = self.reload;
            true
        } else {
            self.counter += 1;
            false
        }
    }
}

pub struct TimerController {
    pub timers: [Timer; 4],
}

impl Default for TimerController {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerController {
    pub fn new() -> Self {
        Self {
            timers: [
                Timer::new(),
                Timer::new(),
                Timer::new(),
                Timer::new(),
            ],
        }
    }

    /// Step timers by CPU cycles. Returns bitmask of timers that overflowed with IRQ enabled
    pub fn step(&mut self, cycles: u32) -> (u8, [bool; 4]) {
        let mut irq_mask = 0u8;
        let mut overflows = [false; 4];

        for i in 0..4 {
            let overflow = if i == 0 || !self.timers[i].cascade {
                self.timers[i].step(cycles)
            } else {
                // Cascaded timer
                if overflows[i - 1] {
                    self.timers[i].increment_cascade()
                } else {
                    false
                }
            };

            overflows[i] = overflow;
            if overflow && self.timers[i].irq_enable {
                irq_mask |= 1 << i;
            }
        }

        (irq_mask, overflows)
    }
}
