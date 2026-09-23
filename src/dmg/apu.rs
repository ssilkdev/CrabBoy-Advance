//! Game Boy APU (NR10-NR52 + wave RAM).
//!
//! The four PSG channels are byte-for-byte the same silicon the GBA inherited,
//! so this module reuses `gba::apu::dmg::DmgAudio` rather than duplicating the
//! square/wave/noise generators. Only two things differ and both are handled
//! at the boundary:
//!
//! 1. **Clock.** The GB core runs at 4,194,304 Hz; the GBA channel code is
//!    written in 16,777,216 Hz units (period `(2048-f)*16`, frame sequencer
//!    every 32768). Feeding it `gb_cycles * 4` makes every derived period and
//!    the 512 Hz sequencer exactly correct with no duplicated constants.
//! 2. **Register layout.** GB exposes NR10..NR52 as individual 8-bit ports at
//!    0xFF10-0xFF26; the GBA packs the same fields into 16-bit SOUNDxCNT_*
//!    registers. The map below is the byte-lane translation.
//!
//! Wave RAM is accessed directly on bank 0: the GBA's two-bank "write the
//! inactive bank" behaviour is a GBA-only extension and would corrupt GB
//! playback.

use crate::gba::apu::audio_output::AudioOutput;
use crate::gba::apu::dmg::DmgAudio;

/// GB CPU frequency. One T-cycle here equals four of the GBA channel
/// code's internal units.
pub const GB_CLOCK_HZ: f64 = 4_194_304.0;

pub struct GbApu {
    pub psg: DmgAudio,
    pub audio_output: AudioOutput,

    /// NR50: master volume + VIN enables.
    pub nr50: u8,
    /// NR51: per-channel left/right routing.
    pub nr51: u8,
    /// NR52 bit 7: APU master power. Writing 0 clears every register.
    pub power: bool,

    sample_timer: f64,
    cycles_per_sample: f64,
    sample_batch: Vec<f32>,
    /// Samples handed to the diagnostics linter, same contract as the GBA APU.
    pub pending_diagnostic_samples: Vec<f32>,
}

impl GbApu {
    pub fn new() -> Self {
        let audio_output = AudioOutput::new();
        // Fixed core rate (ROADMAP M2); AudioOutput resamples to the device.
        let cycles_per_sample = GB_CLOCK_HZ / crate::gba::apu::resample::CORE_SAMPLE_RATE as f64;
        Self {
            psg: DmgAudio::new(),
            audio_output,
            nr50: 0x77,
            nr51: 0xF3,
            power: true,
            sample_timer: 0.0,
            cycles_per_sample,
            sample_batch: Vec::with_capacity(4096),
            pending_diagnostic_samples: Vec::with_capacity(4096),
        }
    }

    /// Advance by `cycles` GB T-cycles (always CPU-speed cycles, i.e. the
    /// caller must halve them in CGB double-speed mode -- the APU is not
    /// affected by the speed switch).
    pub fn step(&mut self, cycles: u32) {
        if self.power {
            self.psg.step(cycles * 4);
        }

        self.sample_timer += cycles as f64;
        while self.sample_timer >= self.cycles_per_sample {
            self.sample_timer -= self.cycles_per_sample;
            self.mix_sample();
        }
    }

    fn mix_sample(&mut self) {
        if !self.power {
            self.sample_batch.push(0.0);
            self.sample_batch.push(0.0);
            return;
        }

        let (c1, c2, c3, c4) = self.psg.get_samples();
        let chans = [c1, c2, c3, c4];

        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for (i, &s) in chans.iter().enumerate() {
            // The UI mixer mutes PSG channels 1-4 as mask bits 2-5, matching
            // the GBA channel ordering (0,1 are the Direct Sound FIFOs).
            if self.audio_output.is_channel_muted(i + 2) {
                continue;
            }
            if (self.nr51 & (1 << (i + 4))) != 0 {
                left += s;
            }
            if (self.nr51 & (1 << i)) != 0 {
                right += s;
            }
        }

        // NR50 master volume: 3 bits per side, 0 = 1/8 not silent.
        let vol_l = ((self.nr50 >> 4) & 0x07) as f32 + 1.0;
        let vol_r = (self.nr50 & 0x07) as f32 + 1.0;
        // /4 channels, /8 volume steps.
        left = (left * vol_l) / 32.0;
        right = (right * vol_r) / 32.0;

        self.sample_batch.push(left.clamp(-1.0, 1.0));
        self.sample_batch.push(right.clamp(-1.0, 1.0));
    }

    pub fn flush_samples(&mut self) {
        if self.sample_batch.is_empty() {
            return;
        }
        self.pending_diagnostic_samples
            .extend_from_slice(&self.sample_batch);
        self.audio_output.push_sample_batch(&self.sample_batch);
        self.sample_batch.clear();
    }

    // --- Register ports -------------------------------------------------

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF10 => (self.psg.ch1.cnt_l as u8) | 0x80,
            0xFF11 => ((self.psg.ch1.cnt_h as u8) & 0xC0) | 0x3F,
            0xFF12 => (self.psg.ch1.cnt_h >> 8) as u8,
            0xFF13 => 0xFF, // write-only
            0xFF14 => ((self.psg.ch1.cnt_x >> 8) as u8 & 0x40) | 0xBF,

            0xFF16 => ((self.psg.ch2.cnt_l as u8) & 0xC0) | 0x3F,
            0xFF17 => (self.psg.ch2.cnt_l >> 8) as u8,
            0xFF18 => 0xFF,
            0xFF19 => ((self.psg.ch2.cnt_h >> 8) as u8 & 0x40) | 0xBF,

            0xFF1A => {
                if self.psg.ch3.master_enable {
                    0xFF
                } else {
                    0x7F
                }
            }
            0xFF1B => 0xFF,
            0xFF1C => ((self.psg.ch3.volume_code << 5) | 0x9F) as u8,
            0xFF1D => 0xFF,
            0xFF1E => ((self.psg.ch3.cnt_x >> 8) as u8 & 0x40) | 0xBF,

            0xFF20 => 0xFF,
            0xFF21 => (self.psg.ch4.cnt_l >> 8) as u8,
            0xFF22 => self.psg.ch4.cnt_h as u8,
            0xFF23 => ((self.psg.ch4.cnt_h >> 8) as u8 & 0x40) | 0xBF,

            0xFF24 => self.nr50,
            0xFF25 => self.nr51,
            0xFF26 => {
                // Bits 0-3 are read-only channel-active flags; 4-6 read as 1.
                let mut v = 0x70;
                if self.power {
                    v |= 0x80;
                    v |= self.psg.soundcnt_x_bits() as u8 & 0x0F;
                }
                v
            }
            0xFF30..=0xFF3F => self.psg.ch3.wave_ram[0][(addr - 0xFF30) as usize],
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        // While powered off only NR52 and wave RAM are writable; every other
        // port ignores writes (this is what `dmg_sound/09-wave read while on`
        // and friends check, and what stops a reset game re-triggering notes).
        if !self.power && addr != 0xFF26 && !(0xFF30..=0xFF3F).contains(&addr) {
            return;
        }

        match addr {
            0xFF10 => self.psg.ch1.write_cnt_l_byte(0, val),
            0xFF11 => self.psg.ch1.write_cnt_h_byte(0, val),
            0xFF12 => self.psg.ch1.write_cnt_h_byte(1, val),
            0xFF13 => self.psg.ch1.write_cnt_x_byte(0, val),
            0xFF14 => self.psg.ch1.write_cnt_x_byte(1, val),

            0xFF16 => self.psg.ch2.write_cnt_l_byte(0, val),
            0xFF17 => self.psg.ch2.write_cnt_l_byte(1, val),
            0xFF18 => self.psg.ch2.write_cnt_h_byte(0, val),
            0xFF19 => self.psg.ch2.write_cnt_h_byte(1, val),

            0xFF1A => {
                // GB NR30 has only the DAC-enable bit; the GBA's bank/two-bank
                // bits do not exist, so mask them off to keep bank 0 playing.
                self.psg.ch3.write_cnt_l_byte(0, val & 0x80);
                self.psg.ch3.bank = 0;
                self.psg.ch3.two_banks = false;
            }
            0xFF1B => self.psg.ch3.write_cnt_h_byte(0, val),
            0xFF1C => self.psg.ch3.write_cnt_h_byte(1, val),
            0xFF1D => self.psg.ch3.write_cnt_x_byte(0, val),
            0xFF1E => self.psg.ch3.write_cnt_x_byte(1, val),

            0xFF20 => self.psg.ch4.write_cnt_l_byte(0, val),
            0xFF21 => self.psg.ch4.write_cnt_l_byte(1, val),
            0xFF22 => self.psg.ch4.write_cnt_h_byte(0, val),
            0xFF23 => self.psg.ch4.write_cnt_h_byte(1, val),

            0xFF24 => self.nr50 = val,
            0xFF25 => self.nr51 = val,
            0xFF26 => {
                let on = (val & 0x80) != 0;
                if !on && self.power {
                    self.psg.ch1.power_off_reset();
                    self.psg.ch2.power_off_reset();
                    self.psg.ch3.power_off_reset();
                    self.psg.ch4.power_off_reset();
                    self.nr50 = 0;
                    self.nr51 = 0;
                }
                self.power = on;
            }
            0xFF30..=0xFF3F => {
                self.psg.ch3.wave_ram[0][(addr - 0xFF30) as usize] = val;
            }
            _ => {}
        }
    }
}

impl Default for GbApu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nr52_power_off_clears_channel_registers() {
        let mut apu = GbApu::new();
        apu.write(0xFF12, 0xF0); // ch1 envelope: volume 15
        apu.write(0xFF14, 0x80); // trigger
        assert!(apu.psg.ch1.active);
        apu.write(0xFF26, 0x00);
        assert!(!apu.psg.ch1.active);
        assert_eq!(apu.nr51, 0, "NR51 cleared on power off");
    }

    #[test]
    fn writes_are_ignored_while_powered_off() {
        let mut apu = GbApu::new();
        apu.write(0xFF26, 0x00);
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF14, 0x80);
        assert!(!apu.psg.ch1.active, "trigger must not work with APU off");
    }

    #[test]
    fn wave_ram_is_accessible_while_powered_off() {
        let mut apu = GbApu::new();
        apu.write(0xFF26, 0x00);
        apu.write(0xFF30, 0xAB);
        assert_eq!(apu.read(0xFF30), 0xAB);
        assert_eq!(apu.psg.ch3.wave_ram[0][0], 0xAB, "GB uses bank 0 directly");
    }

    #[test]
    fn nr52_reports_active_channels() {
        let mut apu = GbApu::new();
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF14, 0x80);
        assert_eq!(apu.read(0xFF26) & 0x81, 0x81, "power bit + ch1 active");
    }

    #[test]
    fn frame_sequencer_ticks_at_512hz_in_gb_cycles() {
        // 8192 GB T-cycles = one frame-sequencer step. A length-enabled
        // channel with counter 1 must go silent after exactly one step.
        let mut apu = GbApu::new();
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF11, 63); // length load -> counter = 1
        apu.write(0xFF14, 0xC0); // trigger + length enable
        assert!(apu.psg.ch1.active);
        apu.step(8191);
        assert!(apu.psg.ch1.active, "not yet: under one sequencer period");
        apu.step(2);
        assert!(!apu.psg.ch1.active, "length expired at 8192 GB cycles");
    }

    #[test]
    fn nr51_routing_pans_channels() {
        let mut apu = GbApu::new();
        apu.nr51 = 0x10; // ch1 left only
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF13, 0x00);
        apu.write(0xFF14, 0x87);
        apu.step(4096);
        apu.flush_samples();
        let s = &apu.pending_diagnostic_samples;
        assert!(!s.is_empty());
        let any_right = s.iter().skip(1).step_by(2).any(|&v| v.abs() > 0.001);
        assert!(!any_right, "right channel must be silent with NR51=0x10");
    }
}
