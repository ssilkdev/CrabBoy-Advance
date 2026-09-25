//! Nintendo M4A ("Sappy") Audio Interception & HD Re-Synthesis (ROADMAP M9)
//!
//! Provides:
//! - Auto-detection of Nintendo's shared M4A music engine and per-game song table database
//! - Interception of active note, instrument, and envelope data from GBA memory
//! - High-quality 48 kHz stereo floating-point sampler engine (Linear, Cubic Hermite, Sinc)
//! - Standard MIDI File (.mid) Type 1 export
//! - Multi-track 48 kHz WAV stem export
//! - Seamless switching between HD re-synthesis and native hardware audio
//! - Graceful fallback to hardware audio for non-M4A games

pub mod voice;
pub mod song;
pub mod sampler;
pub mod midi_export;
pub mod stem_export;

pub use voice::{ToneData, WaveData};
pub use song::{SongHeader, SongTableEntry, M4aSequencer, M4aEvent, CLOCK_TABLE};
pub use sampler::{HdM4aSampler, M4aInterpolation, Voice, AdsrState};
pub use midi_export::export_song_to_midi;
pub use stem_export::{render_song_to_stems, write_wav_file, StemTrack};

/// Audio engine output mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioEngineMode {
    /// 100% bit-identical native hardware audio (DirectSound 8-bit FIFO + DMG PSG)
    #[default]
    HardwareOnly,
    /// 48 kHz high-precision floating point re-synthesis (M4A Sappy games)
    HdReSynthesis,
}

impl AudioEngineMode {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::HardwareOnly => "Native Hardware Audio (APU 8-bit)",
            Self::HdReSynthesis => "HD Music Re-synthesis (48 kHz Sappy / M4A)",
        }
    }
}

/// Configuration for HD M4A audio re-synthesis
#[derive(Debug, Clone)]
pub struct M4aConfig {
    /// Master toggle for HD re-synthesis
    pub enabled: bool,
    /// Interpolation algorithm for sample resampling
    pub interpolation: M4aInterpolation,
    /// Whether stereo reverb effect is enabled
    pub reverb_enabled: bool,
    /// Reverb send level (0.0 .. 1.0)
    pub reverb_level: f32,
    /// Master volume scaling factor
    pub master_volume: f32,
    /// Whether to mix hardware sound effects (DMG channels & non-BGM DirectSound)
    pub mix_hardware_sfx: bool,
    /// Maximum polyphony (simultaneous voices)
    pub max_polyphony: usize,
}

impl Default for M4aConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interpolation: M4aInterpolation::CubicHermite,
            reverb_enabled: true,
            reverb_level: 0.25,
            master_volume: 1.0,
            mix_hardware_sfx: true,
            max_polyphony: 32,
        }
    }
}

/// Verified profile for an M4A game in the database
#[derive(Debug, Clone)]
pub struct M4aGameProfile {
    pub game_code: String,
    pub title: String,
    pub song_table_offset: usize,
    pub song_count: usize,
}

/// Per-game database of safe M4A song tables and universal auto-detector
pub struct M4aDatabase;

impl M4aDatabase {
    /// Look up known game profile by 4-character game code
    pub fn lookup(game_code: &str) -> Option<M4aGameProfile> {
        let code = game_code.trim();
        let (title, offset, count) = match code {
            // Pokémon Emerald (USA, Europe, Japan, etc.)
            "BPEE" | "BPEP" => ("Pokémon Emerald Version", 0x6B49F0, 610),
            "BPEJ" => ("Pokémon Emerald Version (Japan)", 0x63C2AC, 610),
            "BPED" => ("Pokémon Emerald Version (Germany)", 0x6C5BDC, 610),
            "BPEF" => ("Pokémon Emerald Version (France)", 0x6B890C, 610),
            "BPES" => ("Pokémon Emerald Version (Spain)", 0x6B6FA4, 610),
            "BPEI" => ("Pokémon Emerald Version (Italy)", 0x6B1004, 610),

            // Pokémon FireRed & LeafGreen
            "BPRE" => ("Pokémon FireRed", 0x4A32CC, 500),
            "BPGE" => ("Pokémon LeafGreen", 0x4A2A2C, 500),

            // Pokémon Ruby & Sapphire
            "AXVE" => ("Pokémon Ruby", 0x4548E0, 450),
            "AXPE" => ("Pokémon Sapphire", 0x454508, 450),

            // The Legend of Zelda: The Minish Cap
            "BZME" => ("The Legend of Zelda: The Minish Cap (USA)", 0xA11DBC, 546),
            "BZMP" => ("The Legend of Zelda: The Minish Cap (Europe)", 0xB1D414, 546),
            "BZMJ" => ("The Legend of Zelda: The Minish Cap (Japan)", 0x9F3D3C, 546),

            // Fire Emblem: The Sacred Stones (FE8)
            "BE8E" => ("Fire Emblem: The Sacred Stones (USA)", 0x224470, 1000),
            "BE8P" => ("Fire Emblem: The Sacred Stones (Europe)", 0x42FFB0, 1000),
            "BE8J" => ("Fire Emblem: The Sacred Stones (Japan)", 0x214120, 1000),

            // Fire Emblem: The Blazing Blade (FE7)
            "AE7E" => ("Fire Emblem: The Blazing Blade (USA)", 0x69D6D8, 1001),
            "AE7X" => ("Fire Emblem: The Blazing Blade (Europe)", 0x6805F4, 1001),
            "AE7J" => ("Fire Emblem: The Blazing Blade (Japan)", 0x6EA8C8, 1001),

            // Fire Emblem: The Binding Blade (FE6)
            "AFEJ" => ("Fire Emblem: The Binding Blade (Japan)", 0x3994D8, 621),

            // Metroid Fusion & Zero Mission
            "AMFE" | "AMFP" | "AMFJ" => ("Metroid Fusion", 0x71794C, 300),
            "BMXE" | "BMXP" | "BMXJ" => ("Metroid: Zero Mission", 0x794020, 300),

            // Castlevania: Aria of Sorrow
            "AANE" | "AANP" | "AANJ" => ("Castlevania: Aria of Sorrow", 0x4FA908, 200),

            // Golden Sun & Golden Sun: The Lost Age
            "AGSE" | "AGSP" | "AGSJ" => ("Golden Sun", 0x153A00, 300),
            "AGFE" | "AGFP" | "AGFJ" => ("Golden Sun: The Lost Age", 0x127A00, 300),

            _ => return None,
        };

        Some(M4aGameProfile {
            game_code: code.to_string(),
            title: title.to_string(),
            song_table_offset: offset,
            song_count: count,
        })
    }

    /// Auto-detect M4A sound engine from ROM bytes
    pub fn detect(rom: &[u8], game_code: &str, title: &str) -> Option<M4aGameProfile> {
        // 1. Check known database profile first
        if let Some(profile) = Self::lookup(game_code) {
            return Some(profile);
        }

        // 2. Universal signature search for standard M4A CLOCK_TABLE
        let clock_idx = find_subsequence(rom, &CLOCK_TABLE)?;

        // If clock table is found, scan for candidate song table
        if let Some(song_table_off) = scan_candidate_song_table(rom) {
            return Some(M4aGameProfile {
                game_code: game_code.to_string(),
                title: if !title.is_empty() { title.to_string() } else { "Generic M4A Game".to_string() },
                song_table_offset: song_table_off,
                song_count: 200,
            });
        }

        // Fallback profile if clock table was present
        Some(M4aGameProfile {
            game_code: game_code.to_string(),
            title: if !title.is_empty() { title.to_string() } else { "M4A Sappy Title".to_string() },
            song_table_offset: clock_idx + 0x1000,
            song_count: 50,
        })
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn scan_candidate_song_table(rom: &[u8]) -> Option<usize> {
    if rom.len() < 800 {
        return None;
    }
    for offset in (0..rom.len() - 800).step_by(4) {
        let mut valid = 0;
        for i in 0..15 {
            let entry_off = offset + i * 8;
            if entry_off + 8 > rom.len() { break; }
            let ptr = u32::from_le_bytes([rom[entry_off], rom[entry_off+1], rom[entry_off+2], rom[entry_off+3]]);
            let ms = u16::from_le_bytes([rom[entry_off+4], rom[entry_off+5]]);
            let me = u16::from_le_bytes([rom[entry_off+6], rom[entry_off+7]]);

            if (0x08000000..0x0A000000).contains(&ptr) && ms <= 10 && me <= 10 {
                let rom_off = (ptr & 0x01FFFFFF) as usize;
                if rom_off + 8 < rom.len() {
                    let tracks = rom[rom_off];
                    let block = rom[rom_off + 1];
                    let tone_ptr = u32::from_le_bytes([rom[rom_off+4], rom[rom_off+5], rom[rom_off+6], rom[rom_off+7]]);
                    if (1..=16).contains(&tracks) && block == 0 && (0x08000000..0x0A000000).contains(&tone_ptr) {
                        valid += 1;
                    }
                }
            } else {
                break;
            }
        }
        if valid >= 10 {
            return Some(offset);
        }
    }
    None
}

/// The M4A Audio Interceptor and High-Resolution Synthesizer Engine
pub struct HdM4aEngine {
    pub config: M4aConfig,
    pub profile: Option<M4aGameProfile>,
    pub sampler: HdM4aSampler,
    pub active_song_id: Option<u16>,
    pub active_song_header: Option<SongHeader>,
    pub standalone_sequencer: Option<M4aSequencer>,
    pub is_playing: bool,
    /// Cached song table entries (ROM offset -> SongTableEntry)
    pub song_table: Vec<SongTableEntry>,
    /// Last detected active song pointer in WRAM
    last_wram_song_ptr: u32,
    /// Last observed track positions to detect note changes
    last_track_cmd_ptrs: [u32; 16],
}

impl Default for HdM4aEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HdM4aEngine {
    pub fn new() -> Self {
        Self {
            config: M4aConfig::default(),
            profile: None,
            sampler: HdM4aSampler::new(32, 48_000),
            active_song_id: None,
            active_song_header: None,
            standalone_sequencer: None,
            is_playing: false,
            song_table: Vec::new(),
            last_wram_song_ptr: 0,
            last_track_cmd_ptrs: [0; 16],
        }
    }

    /// Whether a game with the M4A engine is loaded
    pub fn is_m4a_game(&self) -> bool {
        self.profile.is_some()
    }

    /// Whether HD re-synthesis is currently enabled and active
    pub fn is_active(&self) -> bool {
        self.config.enabled && self.is_m4a_game()
    }

    /// Detect and initialize M4A engine when a cartridge is loaded
    pub fn detect_and_init(&mut self, rom: &[u8], game_code: &str, title: &str) {
        self.profile = M4aDatabase::detect(rom, game_code, title);
        self.sampler.stop_all();
        self.standalone_sequencer = None;
        self.is_playing = false;
        self.active_song_id = None;
        self.active_song_header = None;
        self.song_table.clear();
        self.last_wram_song_ptr = 0;
        self.last_track_cmd_ptrs = [0; 16];

        if let Some(ref prof) = self.profile {
            let offset = prof.song_table_offset;
            let count = prof.song_count.min(1000);
            for i in 0..count {
                let entry_off = offset + i * 8;
                if let Some(entry) = SongTableEntry::parse(rom, entry_off) {
                    if (0x08000000..0x0A000000).contains(&entry.header_ptr) {
                        self.song_table.push(entry);
                    } else if entry.header_ptr == 0 && i < 20 {
                        // Dummy / empty entry
                        self.song_table.push(entry);
                    } else if i > 20 {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }

    /// Synchronize note and instrument events from emulated GBA WRAM
    /// Called periodically / once per frame during emulation
    pub fn sync_from_wram(&mut self, ewram: &[u8], iwram: &[u8], rom: &[u8]) {
        if !self.is_active() || self.standalone_sequencer.is_some() {
            return;
        }

        // Search for active MusicPlayerInfo in WRAM (ident == 0x68736D53 = 'Smsh')
        // In GBA: EWRAM is at 0x02000000, IWRAM is at 0x03000000
        const SMSH_IDENT: u32 = 0x68736D53;

        let find_mplay = |ram: &[u8]| -> Option<usize> {
            if ram.len() < 64 { return None; }
            for off in (0..ram.len() - 64).step_by(4) {
                // Check if ident is at off + 56 (offset of ident in MusicPlayerInfo)
                let ident = u32::from_le_bytes([ram[off + 56], ram[off + 57], ram[off + 58], ram[off + 59]]);
                if ident == SMSH_IDENT {
                    // Validate songHeader pointer
                    let song_hdr_ptr = u32::from_le_bytes([ram[off], ram[off + 1], ram[off + 2], ram[off + 3]]);
                    if (0x08000000..0x0A000000).contains(&song_hdr_ptr) {
                        return Some(off);
                    }
                }
            }
            None
        };

        let mplay_info = find_mplay(iwram).or_else(|| find_mplay(ewram));
        if let Some(off) = mplay_info {
            let ram = if off < iwram.len() { iwram } else { ewram };
            let song_hdr_ptr = u32::from_le_bytes([ram[off], ram[off + 1], ram[off + 2], ram[off + 3]]);
            let status = u32::from_le_bytes([ram[off + 4], ram[off + 5], ram[off + 6], ram[off + 7]]);
            let track_count = ram[off + 8].min(16);
            let tone_ptr = u32::from_le_bytes([ram[off + 52], ram[off + 53], ram[off + 54], ram[off + 55]]);

            let is_paused = (status & 0x80000000) != 0; // MUSICPLAYER_STATUS_PAUSE

            if song_hdr_ptr != self.last_wram_song_ptr {
                self.last_wram_song_ptr = song_hdr_ptr;
                self.active_song_header = SongHeader::parse(rom, song_hdr_ptr);
                self.sampler.stop_all();
            }

            if is_paused {
                self.sampler.stop_all();
                return;
            }

            // Read tracks pointer at off + 48
            let tracks_ptr = u32::from_le_bytes([ram[off + 48], ram[off + 49], ram[off + 50], ram[off + 51]]);
            let tracks_off = if (0x02000000..0x02040000).contains(&tracks_ptr) {
                Some(((tracks_ptr - 0x02000000) as usize, ewram))
            } else if (0x03000000..0x03008000).contains(&tracks_ptr) {
                Some(((tracks_ptr - 0x03000000) as usize, iwram))
            } else {
                None
            };

            if let Some((tr_base, tr_ram)) = tracks_off {
                // Each MusicPlayerTrack is 80 bytes
                for i in 0..track_count as usize {
                    let tr_off = tr_base + i * 80;
                    if tr_off + 80 > tr_ram.len() { break; }

                    let flags = tr_ram[tr_off];
                    let gate_time = tr_ram[tr_off + 4];
                    let key = tr_ram[tr_off + 5];
                    let velocity = tr_ram[tr_off + 6];
                    let voice = tr_ram[tr_off + 32];
                    let vol = tr_ram[tr_off + 18];
                    let pan = tr_ram[tr_off + 20];
                    let bend = tr_ram[tr_off + 14] as i8;
                    let tune = tr_ram[tr_off + 12] as i8;
                    let cmd_ptr = u32::from_le_bytes([tr_ram[tr_off + 64], tr_ram[tr_off + 65], tr_ram[tr_off + 66], tr_ram[tr_off + 67]]);

                    if (flags & 0x80) != 0 { // MPT_FLG_EXIST
                        if cmd_ptr != self.last_track_cmd_ptrs[i] && gate_time > 0 {
                            self.last_track_cmd_ptrs[i] = cmd_ptr;

                            // Resolve instrument and play note
                            if let Some(base_tone) = ToneData::parse_from_rom(rom, tone_ptr, voice) {
                                if let Some((tone, wave)) = base_tone.resolve_for_key(rom, key) {
                                    self.sampler.note_on(i, key, velocity, &tone, wave, pan, vol, bend, tune);
                                }
                            }
                        } else if gate_time == 0 {
                            self.sampler.note_off(i, key);
                        }
                    }
                }
            }
        }
    }

    /// Render 1 stereo sample at 48 kHz
    pub fn render_sample(&mut self, rom: &[u8]) -> (f32, f32) {
        if let Some(ref mut seq) = self.standalone_sequencer {
            let events = seq.step_sample(rom, 48_000);
            let tone_ptr = seq.header.tone_ptr;

            for event in events {
                match event {
                    M4aEvent::NoteOn { channel, key, velocity, .. } => {
                        let voice = if channel < seq.tracks.len() { seq.tracks[channel].voice } else { 0 };
                        let (vol, pan, bend, tune) = if channel < seq.tracks.len() {
                            let tr = &seq.tracks[channel];
                            (tr.volume, tr.pan, tr.pitch_bend, tr.tune)
                        } else {
                            (100, 64, 0, 0)
                        };

                        if let Some(base_tone) = ToneData::parse_from_rom(rom, tone_ptr, voice) {
                            if let Some((tone, wave)) = base_tone.resolve_for_key(rom, key) {
                                self.sampler.note_on(channel, key, velocity, &tone, wave, pan, vol, bend, tune);
                            }
                        }
                    }
                    M4aEvent::NoteOff { channel, key } => {
                        self.sampler.note_off(channel, key);
                    }
                    M4aEvent::VolumeChange { channel, volume } => {
                        self.sampler.set_volume(channel, volume);
                    }
                    M4aEvent::PanChange { channel, pan } => {
                        self.sampler.set_pan(channel, pan);
                    }
                    _ => {}
                }
            }
        }

        self.sampler.render_sample()
    }

    /// Play a specific song by index directly using the standalone sequencer
    pub fn play_song(&mut self, rom: &[u8], song_id: u16) -> bool {
        let entry = if let Some(ref prof) = self.profile {
            let entry_off = prof.song_table_offset + (song_id as usize) * 8;
            SongTableEntry::parse(rom, entry_off)
        } else {
            None
        };

        if let Some(entry) = entry {
            if let Some(header) = SongHeader::parse(rom, entry.header_ptr) {
                self.sampler.stop_all();
                self.standalone_sequencer = Some(M4aSequencer::new(header.clone(), rom));
                self.active_song_id = Some(song_id);
                self.active_song_header = Some(header);
                self.is_playing = true;
                return true;
            }
        }
        false
    }

    /// Stop playback of standalone sequencer
    pub fn stop(&mut self) {
        self.standalone_sequencer = None;
        self.sampler.stop_all();
        self.is_playing = false;
        self.active_song_id = None;
        self.active_song_header = None;
    }

    /// Export currently playing or selected song to Standard MIDI File Type 1 (.mid)
    pub fn export_midi(&self, rom: &[u8], song_id: u16) -> Result<Vec<u8>, String> {
        let header = self.get_song_header(rom, song_id)
            .ok_or_else(|| format!("Could not parse SongHeader for song ID {}", song_id))?;
        let song_name = format!("Song {:03}", song_id);
        export_song_to_midi(rom, &header, &song_name)
    }

    /// Export currently playing or selected song to per-instrument 48 kHz WAV stems
    pub fn export_stems(&self, rom: &[u8], song_id: u16, duration_secs: f32) -> Result<Vec<StemTrack>, String> {
        let header = self.get_song_header(rom, song_id)
            .ok_or_else(|| format!("Could not parse SongHeader for song ID {}", song_id))?;
        Ok(render_song_to_stems(rom, &header, duration_secs, 48_000, self.config.interpolation))
    }

    fn get_song_header(&self, rom: &[u8], song_id: u16) -> Option<SongHeader> {
        if let Some(ref prof) = self.profile {
            let entry_off = prof.song_table_offset + (song_id as usize) * 8;
            let entry = SongTableEntry::parse(rom, entry_off)?;
            SongHeader::parse(rom, entry.header_ptr)
        } else {
            None
        }
    }
}
