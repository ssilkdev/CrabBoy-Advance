//! GBA DMG / PSG Sound Channels Emulation
//!
//! Implements the 4 legacy Game Boy audio channels used by GBA games for sound effects and retro music:
//! - Channel 1: Square wave with frequency sweep & volume envelope (0x060..=0x064)
//! - Channel 2: Square wave with volume envelope (0x068..=0x06C)
//! - Channel 3: Programmable Wave RAM playback (0x070..=0x074, 0x090..=0x09F)
//! - Channel 4: White noise with 15-bit/7-bit LFSR & volume envelope (0x078..=0x07C)
//! - 512 Hz Frame Sequencer for Length, Sweep, and Volume Envelope timing.

pub const FRAME_SEQUENCER_CYCLES: u32 = 32768; // 16,777,216 Hz / 512 Hz

// Wave duty waveforms (8 steps): 12.5%, 25%, 50%, 75%
const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

#[derive(Clone, Debug, Default)]
pub struct Envelope {
    pub initial_volume: u8,
    pub direction_inc: bool,
    pub period: u8,
    pub volume: u8,
    pub timer: u8,
}

impl Envelope {
    pub fn trigger(&mut self) {
        self.volume = self.initial_volume;
        self.timer = self.period;
    }

    /// The DAC is powered off exactly when NRx2's upper 5 bits (initial
    /// volume + direction) are all zero -- i.e. initial volume 0 and a
    /// decreasing envelope. This must be re-checked on every NRx2 write,
    /// not only at trigger time: real hardware silences the channel the
    /// instant the DAC is turned off, even mid-note.
    pub fn dac_enabled(&self) -> bool {
        self.initial_volume != 0 || self.direction_inc
    }

    pub fn step(&mut self) {
        if self.period == 0 {
            return;
        }
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = self.period;
            if self.direction_inc && self.volume < 15 {
                self.volume += 1;
            } else if !self.direction_inc && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sweep {
    pub shift: u8,
    pub decrease: bool,
    pub period: u8,
    pub timer: u8,
    pub enabled: bool,
    pub shadow_freq: u16,
}

impl Sweep {
    pub fn trigger(&mut self, freq: u16, active: &mut bool) {
        self.shadow_freq = freq;
        self.timer = if self.period > 0 { self.period } else { 8 };
        self.enabled = self.period > 0 || self.shift > 0;
        if self.shift > 0 {
            // Check frequency overflow upon trigger
            let delta = self.shadow_freq >> self.shift;
            let target = if self.decrease {
                self.shadow_freq.saturating_sub(delta)
            } else {
                self.shadow_freq + delta
            };
            if target > 2047 && !self.decrease {
                *active = false;
                self.enabled = false;
            }
        }
    }

    pub fn step(&mut self, freq: &mut u16, active: &mut bool) {
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = if self.period > 0 { self.period } else { 8 };
            if self.enabled && self.period > 0 {
                let delta = self.shadow_freq >> self.shift;
                let target = if self.decrease {
                    self.shadow_freq.saturating_sub(delta)
                } else {
                    self.shadow_freq + delta
                };

                if target <= 2047 && self.shift > 0 {
                    *freq = target;
                    self.shadow_freq = target;

                    // Re-check target calculation for overflow disable
                    let next_delta = target >> self.shift;
                    let next_target = if self.decrease {
                        target.saturating_sub(next_delta)
                    } else {
                        target + next_delta
                    };
                    if next_target > 2047 && !self.decrease {
                        *active = false;
                        self.enabled = false;
                    }
                } else if target > 2047 && !self.decrease {
                    *active = false;
                    self.enabled = false;
                }
            }
        }
    }
}

/// Channel 1: Square Wave with Sweep and Envelope
#[derive(Clone, Debug)]
pub struct Channel1 {
    pub cnt_l: u16,
    pub cnt_h: u16,
    pub cnt_x: u16,
    pub duty: usize,
    pub duty_step: usize,
    pub frequency: u16,
    pub timer: i32,
    pub length_counter: u16,
    pub length_enabled: bool,
    pub active: bool,
    pub envelope: Envelope,
    pub sweep: Sweep,
    pub trigger_count: u64,
}

impl Default for Channel1 {
    fn default() -> Self {
        Self {
            cnt_l: 0,
            cnt_h: 0,
            cnt_x: 0,
            duty: 0,
            duty_step: 0,
            frequency: 0,
            timer: 0,
            length_counter: 0,
            length_enabled: false,
            active: false,
            envelope: Envelope::default(),
            sweep: Sweep::default(),
            trigger_count: 0,
        }
    }
}

impl Channel1 {
    pub fn trigger(&mut self) {
        self.trigger_count += 1;
        self.active = true;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.timer = (2048 - self.frequency as i32) * 16;
        self.envelope.trigger();
        self.sweep.trigger(self.frequency, &mut self.active);

        // Channel disabled if DAC is powered off (initial volume == 0 && direction is dec)
        if self.envelope.initial_volume == 0 && !self.envelope.direction_inc {
            self.active = false;
        }
    }

    /// Resets NR10-NR14 (duty/frequency/envelope/sweep) as real hardware
    /// does on NR52 power-off. `trigger_count` (diagnostics-only) and
    /// `length_counter` are intentionally left untouched.
    pub fn power_off_reset(&mut self) {
        self.duty = 0;
        self.duty_step = 0;
        self.frequency = 0;
        self.length_enabled = false;
        self.active = false;
        self.envelope = Envelope::default();
        self.sweep = Sweep::default();
    }

    pub fn write_cnt_l_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            self.sweep.shift = val & 0x07;
            self.sweep.decrease = (val & 0x08) != 0;
            self.sweep.period = (val >> 4) & 0x07;
            self.cnt_l = (self.cnt_l & 0xFF00) | (val as u16);
        }
    }

    pub fn write_cnt_h_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            let len = (val & 0x3F) as u16;
            self.length_counter = 64 - len;
            self.duty = ((val >> 6) & 0x03) as usize;
            self.cnt_h = (self.cnt_h & 0xFF00) | (val as u16);
        } else {
            self.envelope.period = val & 0x07;
            self.envelope.direction_inc = (val & 0x08) != 0;
            self.envelope.initial_volume = (val >> 4) & 0x0F;
            self.cnt_h = (self.cnt_h & 0x00FF) | ((val as u16) << 8);
            if !self.envelope.dac_enabled() {
                self.active = false;
            }
        }
    }

    pub fn write_cnt_x_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            // Low byte (NR13): lower 8 bits of frequency. DOES NOT TRIGGER!
            self.frequency = (self.frequency & 0x0700) | (val as u16);
            self.cnt_x = (self.cnt_x & 0xFF00) | (val as u16);
        } else {
            // High byte (NR14): upper 3 bits of frequency, length enable, trigger
            self.frequency = (self.frequency & 0x00FF) | (((val & 0x07) as u16) << 8);
            self.length_enabled = (val & 0x40) != 0;
            self.cnt_x = (self.cnt_x & 0x00FF) | (((val & 0x47) as u16) << 8);
            if (val & 0x80) != 0 {
                self.trigger();
            }
        }
    }

    pub fn write_cnt_l(&mut self, val: u16) {
        self.cnt_l = val;
        self.sweep.shift = (val & 0x07) as u8;
        self.sweep.decrease = (val & 0x08) != 0;
        self.sweep.period = ((val >> 4) & 0x07) as u8;
    }

    pub fn write_cnt_h(&mut self, val: u16) {
        self.cnt_h = val;
        let len = (val & 0x3F) as u16;
        self.length_counter = 64 - len;
        self.duty = ((val >> 6) & 0x03) as usize;

        self.envelope.period = ((val >> 8) & 0x07) as u8;
        self.envelope.direction_inc = (val & 0x0800) != 0;
        self.envelope.initial_volume = ((val >> 12) & 0x0F) as u8;
        if !self.envelope.dac_enabled() {
            self.active = false;
        }
    }

    pub fn write_cnt_x(&mut self, val: u16) {
        self.frequency = val & 0x07FF;
        self.length_enabled = (val & 0x4000) != 0;
        self.cnt_x = val & 0x47FF;

        // Trigger
        if (val & 0x8000) != 0 {
            self.trigger();
        }
    }

    pub fn step_timer(&mut self, cycles: u32) {
        if !self.active {
            return;
        }
        self.timer -= cycles as i32;
        while self.timer <= 0 {
            let period = (2048 - self.frequency as i32) * 16;
            self.timer += if period > 0 { period } else { 16 };
            self.duty_step = (self.duty_step + 1) & 7;
        }
    }

    pub fn step_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.active = false;
            }
        }
    }

    pub fn step_sweep(&mut self) {
        if self.active {
            self.sweep.step(&mut self.frequency, &mut self.active);
        }
    }

    pub fn step_envelope(&mut self) {
        if self.active {
            self.envelope.step();
        }
    }

    pub fn sample(&self) -> f32 {
        if !self.active || self.envelope.volume == 0 {
            return 0.0;
        }
        let bit = DUTY_TABLE[self.duty][self.duty_step];
        let amp = self.envelope.volume as f32 / 15.0;
        if bit != 0 { amp } else { -amp }
    }
}

/// Channel 2: Square Wave with Envelope (no sweep)
#[derive(Clone, Debug)]
pub struct Channel2 {
    pub cnt_l: u16,
    pub cnt_h: u16,
    pub duty: usize,
    pub duty_step: usize,
    pub frequency: u16,
    pub timer: i32,
    pub length_counter: u16,
    pub length_enabled: bool,
    pub active: bool,
    pub envelope: Envelope,
    pub trigger_count: u64,
}

impl Default for Channel2 {
    fn default() -> Self {
        Self {
            cnt_l: 0,
            cnt_h: 0,
            duty: 0,
            duty_step: 0,
            frequency: 0,
            timer: 0,
            length_counter: 0,
            length_enabled: false,
            active: false,
            envelope: Envelope::default(),
            trigger_count: 0,
        }
    }
}

impl Channel2 {
    pub fn trigger(&mut self) {
        self.trigger_count += 1;
        self.active = true;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.timer = (2048 - self.frequency as i32) * 16;
        self.envelope.trigger();

        if self.envelope.initial_volume == 0 && !self.envelope.direction_inc {
            self.active = false;
        }
    }

    /// See `Channel1::power_off_reset`.
    pub fn power_off_reset(&mut self) {
        self.duty = 0;
        self.duty_step = 0;
        self.frequency = 0;
        self.length_enabled = false;
        self.active = false;
        self.envelope = Envelope::default();
    }

    pub fn write_cnt_l_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            let len = (val & 0x3F) as u16;
            self.length_counter = 64 - len;
            self.duty = ((val >> 6) & 0x03) as usize;
            self.cnt_l = (self.cnt_l & 0xFF00) | (val as u16);
        } else {
            self.envelope.period = val & 0x07;
            self.envelope.direction_inc = (val & 0x08) != 0;
            self.envelope.initial_volume = (val >> 4) & 0x0F;
            self.cnt_l = (self.cnt_l & 0x00FF) | ((val as u16) << 8);
            if !self.envelope.dac_enabled() {
                self.active = false;
            }
        }
    }

    pub fn write_cnt_h_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            // NR23: Frequency low 8 bits (DOES NOT TRIGGER!)
            self.frequency = (self.frequency & 0x0700) | (val as u16);
            self.cnt_h = (self.cnt_h & 0xFF00) | (val as u16);
        } else {
            // NR24: Frequency high 3 bits, length enable, trigger
            self.frequency = (self.frequency & 0x00FF) | (((val & 0x07) as u16) << 8);
            self.length_enabled = (val & 0x40) != 0;
            self.cnt_h = (self.cnt_h & 0x00FF) | (((val & 0x47) as u16) << 8);
            if (val & 0x80) != 0 {
                self.trigger();
            }
        }
    }

    pub fn write_cnt_l(&mut self, val: u16) {
        self.cnt_l = val;
        let len = (val & 0x3F) as u16;
        self.length_counter = 64 - len;
        self.duty = ((val >> 6) & 0x03) as usize;

        self.envelope.period = ((val >> 8) & 0x07) as u8;
        self.envelope.direction_inc = (val & 0x0800) != 0;
        self.envelope.initial_volume = ((val >> 12) & 0x0F) as u8;
        if !self.envelope.dac_enabled() {
            self.active = false;
        }
    }

    pub fn write_cnt_h(&mut self, val: u16) {
        self.frequency = val & 0x07FF;
        self.length_enabled = (val & 0x4000) != 0;
        self.cnt_h = val & 0x47FF;

        // Trigger
        if (val & 0x8000) != 0 {
            self.trigger();
        }
    }

    pub fn step_timer(&mut self, cycles: u32) {
        if !self.active {
            return;
        }
        self.timer -= cycles as i32;
        while self.timer <= 0 {
            let period = (2048 - self.frequency as i32) * 16;
            self.timer += if period > 0 { period } else { 16 };
            self.duty_step = (self.duty_step + 1) & 7;
        }
    }

    pub fn step_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.active = false;
            }
        }
    }

    pub fn step_envelope(&mut self) {
        if self.active {
            self.envelope.step();
        }
    }

    pub fn sample(&self) -> f32 {
        if !self.active || self.envelope.volume == 0 {
            return 0.0;
        }
        let bit = DUTY_TABLE[self.duty][self.duty_step];
        let amp = self.envelope.volume as f32 / 15.0;
        if bit != 0 { amp } else { -amp }
    }
}

/// Channel 3: Programmable Wave Output
#[derive(Clone, Debug)]
pub struct Channel3 {
    pub cnt_l: u16,
    pub cnt_h: u16,
    pub cnt_x: u16,
    /// Two independent 16-byte (32 x 4-bit sample) banks. `bank` selects
    /// which one is currently played back; CPU reads/writes always target
    /// the *other* (inactive) bank, letting a game prepare new waveform
    /// data while the hardware plays the currently-selected one.
    pub wave_ram: [[u8; 16]; 2],
    pub bank: usize,
    pub two_banks: bool,
    pub master_enable: bool,
    pub volume_code: u8,
    pub force_75: bool,
    pub frequency: u16,
    pub timer: i32,
    pub sample_index: usize,
    pub length_counter: u16,
    pub length_enabled: bool,
    pub active: bool,
    pub trigger_count: u64,
}

impl Default for Channel3 {
    fn default() -> Self {
        Self {
            cnt_l: 0,
            cnt_h: 0,
            cnt_x: 0,
            wave_ram: [[0; 16]; 2],
            bank: 0,
            two_banks: false,
            master_enable: false,
            volume_code: 0,
            force_75: false,
            frequency: 0,
            timer: 0,
            sample_index: 0,
            length_counter: 0,
            length_enabled: false,
            active: false,
            trigger_count: 0,
        }
    }
}

impl Channel3 {
    pub fn trigger(&mut self) {
        self.trigger_count += 1;
        self.active = self.master_enable;
        if self.length_counter == 0 {
            self.length_counter = 256;
        }
        self.timer = (2048 - self.frequency as i32) * 8;
        self.sample_index = 0;
    }

    /// See `Channel1::power_off_reset`. Wave RAM contents are not an NRx
    /// register and persist through power-off on real hardware.
    pub fn power_off_reset(&mut self) {
        self.bank = 0;
        self.two_banks = false;
        self.master_enable = false;
        self.volume_code = 0;
        self.force_75 = false;
        self.frequency = 0;
        self.length_enabled = false;
        self.active = false;
    }

    pub fn write_cnt_l_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            self.two_banks = (val & 0x20) != 0;
            self.bank = ((val >> 6) & 1) as usize;
            self.master_enable = (val & 0x80) != 0;
            if !self.master_enable {
                self.active = false;
            }
            self.cnt_l = (self.cnt_l & 0xFF00) | (val as u16);
        }
    }

    pub fn write_cnt_h_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            self.length_counter = 256 - (val as u16);
            self.cnt_h = (self.cnt_h & 0xFF00) | (val as u16);
        } else {
            self.volume_code = ((val >> 5) & 0x03) as u8;
            self.force_75 = (val & 0x80) != 0;
            self.cnt_h = (self.cnt_h & 0x00FF) | ((val as u16) << 8);
        }
    }

    pub fn write_cnt_x_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            // NR33: Frequency low 8 bits (DOES NOT TRIGGER!)
            self.frequency = (self.frequency & 0x0700) | (val as u16);
            self.cnt_x = (self.cnt_x & 0xFF00) | (val as u16);
        } else {
            // NR34: Frequency high 3 bits, length enable, trigger
            self.frequency = (self.frequency & 0x00FF) | (((val & 0x07) as u16) << 8);
            self.length_enabled = (val & 0x40) != 0;
            self.cnt_x = (self.cnt_x & 0x00FF) | (((val & 0x47) as u16) << 8);
            if (val & 0x80) != 0 {
                self.trigger();
            }
        }
    }

    pub fn write_cnt_l(&mut self, val: u16) {
        self.cnt_l = val;
        self.two_banks = (val & 0x0020) != 0;
        self.bank = ((val >> 6) & 1) as usize;
        self.master_enable = (val & 0x0080) != 0;
        if !self.master_enable {
            self.active = false;
        }
    }

    pub fn write_cnt_h(&mut self, val: u16) {
        self.cnt_h = val;
        let len = (val & 0xFF) as u16;
        self.length_counter = 256 - len;
        self.volume_code = ((val >> 13) & 0x03) as u8;
        self.force_75 = (val & 0x8000) != 0;
    }

    pub fn write_cnt_x(&mut self, val: u16) {
        self.frequency = val & 0x07FF;
        self.length_enabled = (val & 0x4000) != 0;
        self.cnt_x = val & 0x47FF;

        // Trigger
        if (val & 0x8000) != 0 {
            self.trigger();
        }
    }

    /// CPU wave RAM access always targets the *inactive* bank (the one
    /// opposite of `bank`, which the DAC is currently playing from).
    pub fn read_wave_ram(&self, offset: usize) -> u8 {
        self.wave_ram[1 - self.bank][offset & 0x0F]
    }

    pub fn write_wave_ram(&mut self, offset: usize, val: u8) {
        self.wave_ram[1 - self.bank][offset & 0x0F] = val;
    }

    pub fn step_timer(&mut self, cycles: u32) {
        if !self.active || !self.master_enable {
            return;
        }
        self.timer -= cycles as i32;
        let sample_count = if self.two_banks { 64 } else { 32 };
        while self.timer <= 0 {
            let period = (2048 - self.frequency as i32) * 8;
            self.timer += if period > 0 { period } else { 8 };
            self.sample_index = (self.sample_index + 1) % sample_count;
        }
    }

    pub fn step_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.active = false;
            }
        }
    }

    pub fn sample(&self) -> f32 {
        if !self.active || !self.master_enable || (self.volume_code == 0 && !self.force_75) {
            return 0.0;
        }

        // In two-bank mode, `bank` selects which bank plays first (samples
        // 0-31); the other bank is appended after it (samples 32-63),
        // forming one continuous 64-sample waveform.
        let (bank, local_index) = if !self.two_banks || self.sample_index < 32 {
            (self.bank, self.sample_index)
        } else {
            (1 - self.bank, self.sample_index - 32)
        };
        let byte = self.wave_ram[bank][local_index / 2];
        let raw_sample = if (local_index & 1) == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        };

        // Center 4-bit unsigned sample (0..15) symmetrically around zero (-1.0..+1.0)
        let bipolar = (raw_sample as f32 - 7.5) / 7.5;
        if self.force_75 {
            bipolar * 0.75
        } else {
            match self.volume_code {
                1 => bipolar,        // 100%
                2 => bipolar * 0.5,  // 50%
                3 => bipolar * 0.25, // 25%
                _ => 0.0,
            }
        }
    }
}

/// Channel 4: White Noise with LFSR & Envelope
#[derive(Clone, Debug)]
pub struct Channel4 {
    pub cnt_l: u16,
    pub cnt_h: u16,
    pub lfsr: u16,
    pub width_7bit: bool,
    pub ratio: u8,
    pub shift_clock: u8,
    pub timer: i32,
    pub length_counter: u16,
    pub length_enabled: bool,
    pub active: bool,
    pub envelope: Envelope,
    pub trigger_count: u64,
}

impl Default for Channel4 {
    fn default() -> Self {
        Self {
            cnt_l: 0,
            cnt_h: 0,
            lfsr: 0x7FFF,
            width_7bit: false,
            ratio: 0,
            shift_clock: 0,
            timer: 0,
            length_counter: 0,
            length_enabled: false,
            active: false,
            envelope: Envelope::default(),
            trigger_count: 0,
        }
    }
}

impl Channel4 {
    pub fn trigger(&mut self) {
        self.trigger_count += 1;
        self.active = true;
        self.lfsr = 0x7FFF;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.timer = self.calc_period();
        self.envelope.trigger();

        if self.envelope.initial_volume == 0 && !self.envelope.direction_inc {
            self.active = false;
        }
    }

    /// See `Channel1::power_off_reset`.
    pub fn power_off_reset(&mut self) {
        self.ratio = 0;
        self.width_7bit = false;
        self.shift_clock = 0;
        self.length_enabled = false;
        self.active = false;
        self.envelope = Envelope::default();
    }

    pub fn write_cnt_l_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            let len = (val & 0x3F) as u16;
            self.length_counter = 64 - len;
            self.cnt_l = (self.cnt_l & 0xFF00) | (val as u16);
        } else {
            self.envelope.period = val & 0x07;
            self.envelope.direction_inc = (val & 0x08) != 0;
            self.envelope.initial_volume = (val >> 4) & 0x0F;
            self.cnt_l = (self.cnt_l & 0x00FF) | ((val as u16) << 8);
            if !self.envelope.dac_enabled() {
                self.active = false;
            }
        }
    }

    pub fn write_cnt_h_byte(&mut self, byte_idx: u8, val: u8) {
        if byte_idx == 0 {
            // NR43: ratio, width, shift clock (DOES NOT TRIGGER!)
            self.ratio = val & 0x07;
            self.width_7bit = (val & 0x08) != 0;
            self.shift_clock = (val >> 4) & 0x0F;
            self.cnt_h = (self.cnt_h & 0xFF00) | (val as u16);
        } else {
            // NR44: length enable, trigger
            self.length_enabled = (val & 0x40) != 0;
            self.cnt_h = (self.cnt_h & 0x00FF) | (((val & 0x40) as u16) << 8);
            if (val & 0x80) != 0 {
                self.trigger();
            }
        }
    }

    pub fn write_cnt_l(&mut self, val: u16) {
        self.cnt_l = val;
        let len = (val & 0x3F) as u16;
        self.length_counter = 64 - len;

        self.envelope.period = ((val >> 8) & 0x07) as u8;
        self.envelope.direction_inc = (val & 0x0800) != 0;
        self.envelope.initial_volume = ((val >> 12) & 0x0F) as u8;
        if !self.envelope.dac_enabled() {
            self.active = false;
        }
    }

    pub fn write_cnt_h(&mut self, val: u16) {
        self.ratio = (val & 0x07) as u8;
        self.width_7bit = (val & 0x08) != 0;
        self.shift_clock = ((val >> 4) & 0x0F) as u8;
        self.length_enabled = (val & 0x4000) != 0;
        self.cnt_h = val & 0x40FF;

        // Trigger
        if (val & 0x8000) != 0 {
            self.trigger();
        }
    }

    fn calc_period(&self) -> i32 {
        // Base divisors in GBA CPU cycles: [32, 64, 128, 192, 256, 320, 384, 448]
        let base_div: i32 = match self.ratio {
            0 => 32,
            1 => 64,
            2 => 128,
            3 => 192,
            4 => 256,
            5 => 320,
            6 => 384,
            _ => 448,
        };
        base_div << self.shift_clock
    }

    pub fn step_timer(&mut self, cycles: u32) {
        if !self.active {
            return;
        }
        self.timer -= cycles as i32;
        while self.timer <= 0 {
            let period = self.calc_period();
            self.timer += if period > 0 { period } else { 32 };

            // Clock 15-bit / 7-bit LFSR
            let feedback = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
            self.lfsr = (self.lfsr >> 1) | (feedback << 14);
            if self.width_7bit {
                self.lfsr = (self.lfsr & !0x0040) | (feedback << 6);
            }
        }
    }

    pub fn step_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.active = false;
            }
        }
    }

    pub fn step_envelope(&mut self) {
        if self.active {
            self.envelope.step();
        }
    }

    pub fn sample(&self) -> f32 {
        if !self.active || self.envelope.volume == 0 {
            return 0.0;
        }
        let amp = self.envelope.volume as f32 / 15.0;
        // Bit 0 of LFSR is active low
        if (self.lfsr & 1) == 0 {
            amp
        } else {
            -amp
        }
    }
}

/// Comprehensive DMG Audio Subsystem
#[derive(Clone, Debug, Default)]
pub struct DmgAudio {
    pub ch1: Channel1,
    pub ch2: Channel2,
    pub ch3: Channel3,
    pub ch4: Channel4,
    frame_sequencer_timer: u32,
    frame_sequencer_step: u8,
}

impl DmgAudio {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn step(&mut self, cycles: u32) {
        // Step individual channel audio timers
        self.ch1.step_timer(cycles);
        self.ch2.step_timer(cycles);
        self.ch3.step_timer(cycles);
        self.ch4.step_timer(cycles);

        // Step 512 Hz Frame Sequencer
        self.frame_sequencer_timer += cycles;
        while self.frame_sequencer_timer >= FRAME_SEQUENCER_CYCLES {
            self.frame_sequencer_timer -= FRAME_SEQUENCER_CYCLES;
            self.clock_frame_sequencer();
        }
    }

    fn clock_frame_sequencer(&mut self) {
        match self.frame_sequencer_step {
            0 => {
                // Length
                self.ch1.step_length();
                self.ch2.step_length();
                self.ch3.step_length();
                self.ch4.step_length();
            }
            1 => {}
            2 => {
                // Length + Sweep
                self.ch1.step_length();
                self.ch2.step_length();
                self.ch3.step_length();
                self.ch4.step_length();
                self.ch1.step_sweep();
            }
            3 => {}
            4 => {
                // Length
                self.ch1.step_length();
                self.ch2.step_length();
                self.ch3.step_length();
                self.ch4.step_length();
            }
            5 => {}
            6 => {
                // Length + Sweep
                self.ch1.step_length();
                self.ch2.step_length();
                self.ch3.step_length();
                self.ch4.step_length();
                self.ch1.step_sweep();
            }
            7 => {
                // Volume Envelope
                self.ch1.step_envelope();
                self.ch2.step_envelope();
                self.ch4.step_envelope();
            }
            _ => {}
        }
        self.frame_sequencer_step = (self.frame_sequencer_step + 1) & 7;
    }

    /// Returns current samples from channels 1, 2, 3, 4
    pub fn get_samples(&self) -> (f32, f32, f32, f32) {
        (
            self.ch1.sample(),
            self.ch2.sample(),
            self.ch3.sample(),
            self.ch4.sample(),
        )
    }

    /// Bits 0-3 for SOUNDCNT_X indicating channel on/off status
    pub fn soundcnt_x_bits(&self) -> u16 {
        let mut bits = 0u16;
        if self.ch1.active { bits |= 1 << 0; }
        if self.ch2.active { bits |= 1 << 1; }
        if self.ch3.active { bits |= 1 << 2; }
        if self.ch4.active { bits |= 1 << 3; }
        bits
    }
}

#[cfg(test)]
mod channel3_wave_bank_tests {
    use super::*;

    #[test]
    fn writes_target_the_inactive_bank_not_the_playing_one() {
        let mut ch3 = Channel3::default();
        ch3.bank = 0; // bank 0 is selected for playback

        // Writing wave RAM must land in bank 1 (the inactive one), not
        // stomp the bank currently being played.
        ch3.write_wave_ram(0, 0xAB);
        assert_eq!(ch3.wave_ram[1][0], 0xAB);
        assert_eq!(ch3.wave_ram[0][0], 0x00, "playing bank must be untouched");
        assert_eq!(ch3.read_wave_ram(0), 0xAB, "reads must also target the inactive bank");
    }

    fn bipolar(raw_sample: u8) -> f32 {
        (raw_sample as f32 - 7.5) / 7.5
    }

    #[test]
    fn flipping_bank_swaps_which_bank_plays() {
        let mut ch3 = Channel3::default();
        ch3.master_enable = true;
        ch3.volume_code = 1; // 100%
        ch3.active = true;

        ch3.wave_ram[0] = [0x11; 16];
        ch3.wave_ram[1] = [0x22; 16];

        ch3.bank = 0;
        ch3.sample_index = 0;
        assert_eq!(ch3.sample(), bipolar(0x1));

        ch3.bank = 1;
        assert_eq!(ch3.sample(), bipolar(0x2));
    }

    #[test]
    fn two_bank_mode_plays_64_continuous_samples() {
        let mut ch3 = Channel3::default();
        ch3.two_banks = true;
        ch3.bank = 0; // bank 0 plays first, then bank 1 is appended

        // step_timer should wrap sample_index at 64, not 32, in two-bank mode.
        ch3.master_enable = true;
        ch3.active = true;
        ch3.frequency = 2047; // shortest possible period (8 cycles) for a fast test
        ch3.sample_index = 63;
        ch3.timer = 1;
        ch3.step_timer(1);
        assert_eq!(ch3.sample_index, 0, "must wrap at 64 samples, not 32");

        // Sample 32 (first sample of the appended bank) must come from bank 1.
        ch3.wave_ram[0] = [0x00; 16];
        ch3.wave_ram[1][0] = 0xF0;
        ch3.volume_code = 1;
        ch3.sample_index = 32;
        assert_eq!(ch3.sample(), bipolar(0xF));
    }
}

// ---- Save states (ROADMAP M2) --------------------------------------------
use crate::gba::state::{Snapshot, StateReader, StateWriter};

impl Snapshot for Envelope {
    fn save(&self, w: &mut StateWriter) {
        w.u8(self.initial_volume); w.bool(self.direction_inc); w.u8(self.period);
        w.u8(self.volume); w.u8(self.timer);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.initial_volume = r.u8()?; self.direction_inc = r.bool()?; self.period = r.u8()?;
        self.volume = r.u8()?; self.timer = r.u8()?;
        Some(())
    }
}

impl Snapshot for Sweep {
    fn save(&self, w: &mut StateWriter) {
        w.u8(self.shift); w.bool(self.decrease); w.u8(self.period); w.u8(self.timer);
        w.bool(self.enabled); w.u16(self.shadow_freq);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.shift = r.u8()?; self.decrease = r.bool()?; self.period = r.u8()?; self.timer = r.u8()?;
        self.enabled = r.bool()?; self.shadow_freq = r.u16()?;
        Some(())
    }
}

impl Snapshot for Channel1 {
    fn save(&self, w: &mut StateWriter) {
        w.u16(self.cnt_l); w.u16(self.cnt_h); w.u16(self.cnt_x);
        w.u32(self.duty as u32); w.u32(self.duty_step as u32); w.u16(self.frequency);
        w.i32(self.timer); w.u16(self.length_counter); w.bool(self.length_enabled); w.bool(self.active);
        self.envelope.save(w); self.sweep.save(w);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.cnt_l = r.u16()?; self.cnt_h = r.u16()?; self.cnt_x = r.u16()?;
        self.duty = (r.u32()? & 3) as usize; self.duty_step = (r.u32()? & 7) as usize; self.frequency = r.u16()?;
        self.timer = r.i32()?; self.length_counter = r.u16()?; self.length_enabled = r.bool()?; self.active = r.bool()?;
        self.envelope.load(r)?; self.sweep.load(r)
    }
}

impl Snapshot for Channel2 {
    fn save(&self, w: &mut StateWriter) {
        w.u16(self.cnt_l); w.u16(self.cnt_h);
        w.u32(self.duty as u32); w.u32(self.duty_step as u32); w.u16(self.frequency);
        w.i32(self.timer); w.u16(self.length_counter); w.bool(self.length_enabled); w.bool(self.active);
        self.envelope.save(w);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.cnt_l = r.u16()?; self.cnt_h = r.u16()?;
        self.duty = (r.u32()? & 3) as usize; self.duty_step = (r.u32()? & 7) as usize; self.frequency = r.u16()?;
        self.timer = r.i32()?; self.length_counter = r.u16()?; self.length_enabled = r.bool()?; self.active = r.bool()?;
        self.envelope.load(r)
    }
}

impl Snapshot for Channel3 {
    fn save(&self, w: &mut StateWriter) {
        w.u16(self.cnt_l); w.u16(self.cnt_h); w.u16(self.cnt_x);
        w.bytes(&self.wave_ram[0]); w.bytes(&self.wave_ram[1]);
        w.u32(self.bank as u32); w.bool(self.two_banks); w.bool(self.master_enable);
        w.u8(self.volume_code); w.bool(self.force_75); w.u16(self.frequency); w.i32(self.timer);
        w.u32(self.sample_index as u32); w.u16(self.length_counter); w.bool(self.length_enabled); w.bool(self.active);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.cnt_l = r.u16()?; self.cnt_h = r.u16()?; self.cnt_x = r.u16()?;
        r.bytes_into(&mut self.wave_ram[0])?; r.bytes_into(&mut self.wave_ram[1])?;
        self.bank = (r.u32()? & 1) as usize; self.two_banks = r.bool()?; self.master_enable = r.bool()?;
        self.volume_code = r.u8()?; self.force_75 = r.bool()?; self.frequency = r.u16()?; self.timer = r.i32()?;
        self.sample_index = (r.u32()? & 63) as usize; self.length_counter = r.u16()?;
        self.length_enabled = r.bool()?; self.active = r.bool()?;
        Some(())
    }
}

impl Snapshot for Channel4 {
    fn save(&self, w: &mut StateWriter) {
        w.u16(self.cnt_l); w.u16(self.cnt_h); w.u16(self.lfsr); w.bool(self.width_7bit);
        w.u8(self.ratio); w.u8(self.shift_clock); w.i32(self.timer);
        w.u16(self.length_counter); w.bool(self.length_enabled); w.bool(self.active);
        self.envelope.save(w);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.cnt_l = r.u16()?; self.cnt_h = r.u16()?; self.lfsr = r.u16()?; self.width_7bit = r.bool()?;
        self.ratio = r.u8()?; self.shift_clock = r.u8()?; self.timer = r.i32()?;
        self.length_counter = r.u16()?; self.length_enabled = r.bool()?; self.active = r.bool()?;
        self.envelope.load(r)
    }
}

impl Snapshot for DmgAudio {
    fn save(&self, w: &mut StateWriter) {
        self.ch1.save(w); self.ch2.save(w); self.ch3.save(w); self.ch4.save(w);
        w.u32(self.frame_sequencer_timer); w.u8(self.frame_sequencer_step);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.ch1.load(r)?; self.ch2.load(r)?; self.ch3.load(r)?; self.ch4.load(r)?;
        self.frame_sequencer_timer = r.u32()?; self.frame_sequencer_step = r.u8()? & 7;
        Some(())
    }
}
