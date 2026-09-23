//! M4A / Sappy Voice, ToneData, and WaveData definitions
//!
//! Handles 8-bit signed PCM WaveData parsing, ADSR envelope decoding,
//! key-split multi-instruments (0x40), and rhythm/drum tables (0x80).

use std::sync::Arc;

/// WaveData header and raw 8-bit signed PCM audio data.
#[derive(Debug, Clone)]
pub struct WaveData {
    /// Data type / format flag
    pub wav_type: u16,
    /// Status flags (0x4000 = forward loop, 0x0000 = one-shot)
    pub status: u16,
    /// Base frequency multiplier: sample_rate * 2^((180 - original_midi_key) / 12)
    /// In GBA M4A, typically sample_rate * 4096 (e.g. 13379 * 4096 = 54800384)
    pub freq: u32,
    /// Loop start sample index
    pub loop_start: u32,
    /// Total number of 8-bit signed samples
    pub size: u32,
    /// 8-bit signed PCM waveform samples (-128..127)
    pub samples: Vec<i8>,
}

impl WaveData {
    /// Whether this sample repeats infinitely while note is held
    pub fn is_looped(&self) -> bool {
        (self.status & 0x4000) != 0 && self.loop_start < self.size
    }

    /// Calculate base sample rate in Hz (assuming standard middle C / key 60 or stored multiplier)
    pub fn base_sample_rate(&self) -> f64 {
        if self.freq == 0 {
            return 13379.0;
        }
        // GBA M4A stores freq as: sample_rate * 2^((180 - original_midi_key) / 12)
        // For middle C (key 60): 180 - 60 = 120 -> 2^(120/12) = 2^10 = 1024
        // With standard 4x oversampling multiplier: 1024 * 4 = 4096
        let rate = self.freq as f64 / 4096.0;
        if rate >= 4000.0 && rate <= 96000.0 {
            rate
        } else {
            // Alternative scaling: freq / 1024.0
            let alt_rate = self.freq as f64 / 1024.0;
            if alt_rate >= 4000.0 && alt_rate <= 96000.0 {
                alt_rate
            } else {
                13379.0
            }
        }
    }

    /// Parse a WaveData structure from ROM at GBA address `ptr` (0x08xxxxxx)
    pub fn parse_from_rom(rom: &[u8], ptr: u32) -> Option<Self> {
        let rom_offset = rom_ptr_to_offset(rom, ptr)?;
        if rom_offset + 16 > rom.len() {
            return None;
        }

        let wav_type = u16::from_le_bytes([rom[rom_offset], rom[rom_offset + 1]]);
        let status = u16::from_le_bytes([rom[rom_offset + 2], rom[rom_offset + 3]]);
        let freq = u32::from_le_bytes([
            rom[rom_offset + 4],
            rom[rom_offset + 5],
            rom[rom_offset + 6],
            rom[rom_offset + 7],
        ]);
        let loop_start = u32::from_le_bytes([
            rom[rom_offset + 8],
            rom[rom_offset + 9],
            rom[rom_offset + 10],
            rom[rom_offset + 11],
        ]);
        let size = u32::from_le_bytes([
            rom[rom_offset + 12],
            rom[rom_offset + 13],
            rom[rom_offset + 14],
            rom[rom_offset + 15],
        ]);

        // Sanity checks on size (max 8MB for single sample)
        if size == 0 || size > 0x800000 {
            return None;
        }

        let data_start = rom_offset + 16;
        let data_end = (data_start + size as usize).min(rom.len());
        if data_start >= rom.len() {
            return None;
        }

        let samples: Vec<i8> = rom[data_start..data_end]
            .iter()
            .map(|&b| b as i8)
            .collect();

        Some(Self {
            wav_type,
            status,
            freq,
            loop_start: loop_start.min(size.saturating_sub(1)),
            size: samples.len() as u32,
            samples,
        })
    }
}

/// ToneData instrument definition (12 bytes in M4A voicegroup)
#[derive(Debug, Clone)]
pub struct ToneData {
    /// Instrument type:
    /// - 0x00 / 0x08: DirectSound PCM
    /// - 0x01 / 0x09: CGB Channel 1 (Square with sweep)
    /// - 0x02 / 0x0A: CGB Channel 2 (Square)
    /// - 0x03 / 0x0B: CGB Channel 3 (Programmable Wave)
    /// - 0x04 / 0x0C: CGB Channel 4 (Noise)
    /// - 0x40: Multi-sample / Key Split table
    /// - 0x80: Drum / Rhythm table
    pub tone_type: u8,
    /// Root key / base note
    pub key: u8,
    /// Sound length (for compatible sound)
    pub length: u8,
    /// Pan or sweep
    pub pan_sweep: u8,
    /// Pointer to WaveData, Key Split table, or Drum table
    pub wav_ptr: u32,
    /// Attack rate (0..255)
    pub attack: u8,
    /// Decay rate (0..255)
    pub decay: u8,
    /// Sustain level (0..255)
    pub sustain: u8,
    /// Release rate (0..255)
    pub release: u8,
    /// Parsed PCM wave if this is a direct PCM instrument
    pub wave: Option<Arc<WaveData>>,
}

impl ToneData {
    /// Parse a 12-byte ToneData entry from ROM
    pub fn parse_from_rom(rom: &[u8], tone_ptr: u32, inst_idx: u8) -> Option<Self> {
        let base_offset = rom_ptr_to_offset(rom, tone_ptr)?;
        let entry_offset = base_offset + (inst_idx as usize) * 12;
        if entry_offset + 12 > rom.len() {
            return None;
        }

        let tone_type = rom[entry_offset];
        let key = rom[entry_offset + 1];
        let length = rom[entry_offset + 2];
        let pan_sweep = rom[entry_offset + 3];
        let wav_ptr = u32::from_le_bytes([
            rom[entry_offset + 4],
            rom[entry_offset + 5],
            rom[entry_offset + 6],
            rom[entry_offset + 7],
        ]);
        let attack = rom[entry_offset + 8];
        let decay = rom[entry_offset + 9];
        let sustain = rom[entry_offset + 10];
        let release = rom[entry_offset + 11];

        let wave = if (tone_type & 0xC0) == 0 && (tone_type & 0x07) == 0 {
            // DirectSound PCM
            WaveData::parse_from_rom(rom, wav_ptr).map(Arc::new)
        } else {
            None
        };

        Some(Self {
            tone_type,
            key,
            length,
            pan_sweep,
            wav_ptr,
            attack,
            decay,
            sustain,
            release,
            wave,
        })
    }

    /// Resolve effective ToneData and WaveData for a specific MIDI key
    pub fn resolve_for_key(&self, rom: &[u8], midi_key: u8) -> Option<(ToneData, Option<Arc<WaveData>>)> {
        if self.tone_type == 0x40 {
            // Key Split: wav_ptr points to key split table (array of tone pointers or sub-tones)
            if let Some(table_off) = rom_ptr_to_offset(rom, self.wav_ptr) {
                // Table has 128 bytes mapping each MIDI key (0..127) to an instrument index,
                // followed by sub-tone definitions, or array of pointers
                if table_off + 128 <= rom.len() {
                    let sub_idx = rom[table_off + (midi_key as usize).min(127)];
                    // Read sub-instrument at table_off + 128 + sub_idx * 12
                    let sub_tone_ptr = (self.wav_ptr + 128) + (sub_idx as u32) * 12;
                    if let Some(sub_tone) = ToneData::parse_from_rom(rom, sub_tone_ptr, 0) {
                        let wav = sub_tone.wave.clone().or_else(|| {
                            WaveData::parse_from_rom(rom, sub_tone.wav_ptr).map(Arc::new)
                        });
                        return Some((sub_tone, wav));
                    }
                }
            }
        } else if self.tone_type == 0x80 {
            // Drum Table: wav_ptr points to drum table (each key has a 12-byte ToneData)
            let drum_inst_ptr = self.wav_ptr + (midi_key as u32) * 12;
            if let Some(drum_tone) = ToneData::parse_from_rom(rom, drum_inst_ptr, 0) {
                let wav = drum_tone.wave.clone().or_else(|| {
                    WaveData::parse_from_rom(rom, drum_tone.wav_ptr).map(Arc::new)
                });
                return Some((drum_tone, wav));
            }
        }

        // Standard instrument
        let wav = self.wave.clone().or_else(|| {
            if (self.tone_type & 0xC0) == 0 && (self.tone_type & 0x07) == 0 {
                WaveData::parse_from_rom(rom, self.wav_ptr).map(Arc::new)
            } else {
                None
            }
        });
        Some((self.clone(), wav))
    }
}

/// Helper converting a 32-bit GBA ROM pointer (0x08xxxxxx or 0x09xxxxxx) to file offset
pub fn rom_ptr_to_offset(rom: &[u8], ptr: u32) -> Option<usize> {
    if (0x08000000..0x0A000000).contains(&ptr) {
        let off = (ptr & 0x01FFFFFF) as usize;
        if off < rom.len() {
            return Some(off);
        }
    }
    None
}
