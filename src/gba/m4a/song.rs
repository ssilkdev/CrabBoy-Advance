//! M4A / Sappy Song Header, Song Table, Bytecode Commands, and Track Sequencer
//!
//! Decodes Sappy music bytecode, tracking note gate times, wait ticks,
//! volume fades, pitch bends, subroutines (PATT/PEND), and loops (GOTO/REPT).

use super::voice::rom_ptr_to_offset;

/// Standard M4A clock table mapping note commands (0xD0..=0xFF) to tick durations
pub const CLOCK_TABLE: [u8; 48] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x1C, 0x1E, 0x20, 0x24, 0x28, 0x2A, 0x2C, 0x30,
    0x34, 0x36, 0x38, 0x3C, 0x40, 0x42, 0x44, 0x48,
    0x4C, 0x4E, 0x50, 0x54, 0x58, 0x5A, 0x5C, 0x60,
];

/// Song table entry (8 bytes in GBA ROM)
#[derive(Debug, Clone, Copy)]
pub struct SongTableEntry {
    /// Pointer to SongHeader in ROM (0x08xxxxxx)
    pub header_ptr: u32,
    /// Music player info index (0 = BGM, 1..=3 = SFX)
    pub ms: u16,
    /// Music player info index
    pub me: u16,
}

impl SongTableEntry {
    pub fn parse(rom: &[u8], offset: usize) -> Option<Self> {
        if offset + 8 > rom.len() {
            return None;
        }
        let header_ptr = u32::from_le_bytes([
            rom[offset],
            rom[offset + 1],
            rom[offset + 2],
            rom[offset + 3],
        ]);
        let ms = u16::from_le_bytes([rom[offset + 4], rom[offset + 5]]);
        let me = u16::from_le_bytes([rom[offset + 6], rom[offset + 7]]);
        Some(Self { header_ptr, ms, me })
    }
}

/// SongHeader structure in ROM
#[derive(Debug, Clone)]
pub struct SongHeader {
    /// Number of active tracks in this song
    pub track_count: u8,
    /// Block count (usually 0)
    pub block_count: u8,
    /// Playback priority
    pub priority: u8,
    /// Reverb flags and level (bit 7 set indicates custom reverb level)
    pub reverb: u8,
    /// Pointer to instrument table / voicegroup (0x08xxxxxx)
    pub tone_ptr: u32,
    /// Pointers to each track's sequence bytecode (0x08xxxxxx)
    pub track_ptrs: Vec<u32>,
}

impl SongHeader {
    pub fn parse(rom: &[u8], ptr: u32) -> Option<Self> {
        let offset = rom_ptr_to_offset(rom, ptr)?;
        if offset + 8 > rom.len() {
            return None;
        }

        let track_count = rom[offset];
        let block_count = rom[offset + 1];
        let priority = rom[offset + 2];
        let reverb = rom[offset + 3];
        let tone_ptr = u32::from_le_bytes([
            rom[offset + 4],
            rom[offset + 5],
            rom[offset + 6],
            rom[offset + 7],
        ]);

        if track_count == 0 || track_count > 16 {
            return None;
        }

        let mut track_ptrs = Vec::with_capacity(track_count as usize);
        for i in 0..track_count as usize {
            let tr_off = offset + 8 + i * 4;
            if tr_off + 4 > rom.len() {
                return None;
            }
            let tr_ptr = u32::from_le_bytes([
                rom[tr_off],
                rom[tr_off + 1],
                rom[tr_off + 2],
                rom[tr_off + 3],
            ]);
            track_ptrs.push(tr_ptr);
        }

        Some(Self {
            track_count,
            block_count,
            priority,
            reverb,
            tone_ptr,
            track_ptrs,
        })
    }
}

/// Events emitted during track sequence playback
#[derive(Debug, Clone, PartialEq)]
pub enum M4aEvent {
    NoteOn {
        channel: usize,
        key: u8,
        velocity: u8,
        gate_ticks: u32,
    },
    NoteOff {
        channel: usize,
        key: u8,
    },
    VoiceChange {
        channel: usize,
        voice: u8,
    },
    VolumeChange {
        channel: usize,
        volume: u8,
    },
    PanChange {
        channel: usize,
        pan: u8,
    },
    PitchBend {
        channel: usize,
        bend: i8,
    },
    TempoChange {
        bpm: u16,
    },
    TrackEnd {
        channel: usize,
    },
    SongLoop {
        target_offset: usize,
    },
}

/// State of a single track within the M4A sequencer
#[derive(Debug, Clone)]
pub struct SequencerTrack {
    pub channel_id: usize,
    pub cmd_offset: usize,
    pub wait_ticks: u32,
    pub gate_ticks: u32,
    pub active_key: Option<u8>,
    pub running_status: u8,
    pub voice: u8,
    pub volume: u8,
    pub pan: u8,
    pub pitch_bend: i8,
    pub bend_range: u8,
    pub key_shift: i8,
    pub tune: i8,
    pub last_key: u8,
    pub last_velocity: u8,
    pub call_stack: Vec<usize>,
    pub repeat_count: u8,
    pub repeat_target: usize,
    pub ended: bool,
}

impl SequencerTrack {
    pub fn new(channel_id: usize, start_offset: usize) -> Self {
        Self {
            channel_id,
            cmd_offset: start_offset,
            wait_ticks: 0,
            gate_ticks: 0,
            active_key: None,
            running_status: 0,
            voice: 0,
            volume: 100,
            pan: 64, // Center
            pitch_bend: 0,
            bend_range: 2,
            key_shift: 0,
            tune: 0,
            last_key: 60, // Middle C
            last_velocity: 100,
            call_stack: Vec::with_capacity(4),
            repeat_count: 0,
            repeat_target: 0,
            ended: false,
        }
    }

    /// Step this track by 1 tick, returning any note-off event if gate expired
    pub fn step_tick(&mut self) -> Option<M4aEvent> {
        if self.ended {
            return None;
        }

        let mut event = None;
        if self.gate_ticks > 0 {
            self.gate_ticks -= 1;
            if self.gate_ticks == 0 {
                if let Some(key) = self.active_key.take() {
                    event = Some(M4aEvent::NoteOff {
                        channel: self.channel_id,
                        key,
                    });
                }
            }
        }

        if self.wait_ticks > 0 {
            self.wait_ticks -= 1;
        }

        event
    }

    /// Process commands until the next wait tick or track end
    pub fn process_commands(&mut self, rom: &[u8], events: &mut Vec<M4aEvent>) {
        if self.ended || self.wait_ticks > 0 {
            return;
        }

        let mut loop_safety = 0;
        while self.wait_ticks == 0 && !self.ended && loop_safety < 1000 {
            loop_safety += 1;
            if self.cmd_offset >= rom.len() {
                self.ended = true;
                events.push(M4aEvent::TrackEnd { channel: self.channel_id });
                break;
            }

            let cmd = rom[self.cmd_offset];
            self.cmd_offset += 1;

            if cmd <= 0x7F {
                // Repeatable parameter / argument or rest
                self.handle_parameter(cmd, rom, events);
            } else if (0x80..=0xB0).contains(&cmd) {
                // Wait command: W00..=W48
                self.wait_ticks = (cmd - 0x80) as u32;
            } else {
                match cmd {
                    // FINE: Track end
                    0xB1 => {
                        self.ended = true;
                        events.push(M4aEvent::TrackEnd { channel: self.channel_id });
                        break;
                    }
                    // GOTO: Jump to address (song loop or sequence branch)
                    0xB2 => {
                        if self.cmd_offset + 4 <= rom.len() {
                            let target_ptr = u32::from_le_bytes([
                                rom[self.cmd_offset],
                                rom[self.cmd_offset + 1],
                                rom[self.cmd_offset + 2],
                                rom[self.cmd_offset + 3],
                            ]);
                            self.cmd_offset += 4;
                            if let Some(target_off) = rom_ptr_to_offset(rom, target_ptr) {
                                self.cmd_offset = target_off;
                                events.push(M4aEvent::SongLoop { target_offset: target_off });
                            } else {
                                self.ended = true;
                                break;
                            }
                        }
                    }
                    // PATT: Subroutine call
                    0xB3 => {
                        if self.cmd_offset + 4 <= rom.len() {
                            let target_ptr = u32::from_le_bytes([
                                rom[self.cmd_offset],
                                rom[self.cmd_offset + 1],
                                rom[self.cmd_offset + 2],
                                rom[self.cmd_offset + 3],
                            ]);
                            self.cmd_offset += 4;
                            if let Some(target_off) = rom_ptr_to_offset(rom, target_ptr) {
                                self.call_stack.push(self.cmd_offset);
                                self.cmd_offset = target_off;
                            }
                        }
                    }
                    // PEND: Subroutine return
                    0xB4 => {
                        if let Some(ret_addr) = self.call_stack.pop() {
                            self.cmd_offset = ret_addr;
                        } else {
                            self.ended = true;
                            break;
                        }
                    }
                    // REPT: Repeat loop
                    0xB5 => {
                        if self.cmd_offset + 5 <= rom.len() {
                            let count = rom[self.cmd_offset];
                            let target_ptr = u32::from_le_bytes([
                                rom[self.cmd_offset + 1],
                                rom[self.cmd_offset + 2],
                                rom[self.cmd_offset + 3],
                                rom[self.cmd_offset + 4],
                            ]);
                            self.cmd_offset += 5;
                            if let Some(target_off) = rom_ptr_to_offset(rom, target_ptr) {
                                if self.repeat_count == 0 {
                                    self.repeat_count = count;
                                    self.repeat_target = target_off;
                                }
                                self.repeat_count -= 1;
                                if self.repeat_count > 0 {
                                    self.cmd_offset = self.repeat_target;
                                }
                            }
                        }
                    }
                    // TEMPO: Set tempo
                    0xBB => {
                        if self.cmd_offset < rom.len() {
                            let tempo_val = rom[self.cmd_offset];
                            self.cmd_offset += 1;
                            // GBA tempo is tempo_val * 2 BPM
                            events.push(M4aEvent::TempoChange { bpm: (tempo_val as u16) * 2 });
                        }
                    }
                    // KEYSH: Key shift
                    0xBC => {
                        if self.cmd_offset < rom.len() {
                            self.key_shift = rom[self.cmd_offset] as i8;
                            self.cmd_offset += 1;
                        }
                    }
                    // VOICE: Program change / instrument
                    0xBD => {
                        if self.cmd_offset < rom.len() {
                            self.voice = rom[self.cmd_offset];
                            self.cmd_offset += 1;
                            self.running_status = 0xBD;
                            events.push(M4aEvent::VoiceChange {
                                channel: self.channel_id,
                                voice: self.voice,
                            });
                        }
                    }
                    // VOL: Volume
                    0xBE => {
                        if self.cmd_offset < rom.len() {
                            self.volume = rom[self.cmd_offset];
                            self.cmd_offset += 1;
                            self.running_status = 0xBE;
                            events.push(M4aEvent::VolumeChange {
                                channel: self.channel_id,
                                volume: self.volume,
                            });
                        }
                    }
                    // PAN: Panning
                    0xBF => {
                        if self.cmd_offset < rom.len() {
                            self.pan = rom[self.cmd_offset];
                            self.cmd_offset += 1;
                            self.running_status = 0xBF;
                            events.push(M4aEvent::PanChange {
                                channel: self.channel_id,
                                pan: self.pan,
                            });
                        }
                    }
                    // BEND: Pitch bend
                    0xC0 => {
                        if self.cmd_offset < rom.len() {
                            self.pitch_bend = (rom[self.cmd_offset] as i8).saturating_sub(64);
                            self.cmd_offset += 1;
                            events.push(M4aEvent::PitchBend {
                                channel: self.channel_id,
                                bend: self.pitch_bend,
                            });
                        }
                    }
                    // BENDR: Pitch bend range
                    0xC1 => {
                        if self.cmd_offset < rom.len() {
                            self.bend_range = rom[self.cmd_offset];
                            self.cmd_offset += 1;
                        }
                    }
                    // LFOS / LFODL / MOD / MODT
                    0xC2 | 0xC3 | 0xC4 | 0xC5 => {
                        if self.cmd_offset < rom.len() {
                            self.cmd_offset += 1;
                        }
                    }
                    // TUNE: Fine tuning
                    0xC8 => {
                        if self.cmd_offset < rom.len() {
                            self.tune = (rom[self.cmd_offset] as i8).saturating_sub(64);
                            self.cmd_offset += 1;
                        }
                    }
                    // XCMD: Extended command
                    0xCD => {
                        if self.cmd_offset < rom.len() {
                            let sub_cmd = rom[self.cmd_offset];
                            self.cmd_offset += 1;
                            match sub_cmd {
                                0x08 | 0x09 | 0x0A => { self.cmd_offset = (self.cmd_offset + 1).min(rom.len()); }
                                0x0D => { self.cmd_offset = (self.cmd_offset + 4).min(rom.len()); }
                                _ => {}
                            }
                        }
                    }
                    // EOT: End tie / Note off
                    0xCE => {
                        if let Some(key) = self.active_key.take() {
                            events.push(M4aEvent::NoteOff {
                                channel: self.channel_id,
                                key,
                            });
                        }
                    }
                    // TIE: Note tie (indefinite note until EOT)
                    0xCF => {
                        self.handle_note_command(cmd, 999999, rom, events);
                    }
                    // 0xD0..=0xFF: Note on with auto-timeout duration from CLOCK_TABLE
                    0xD0..=0xFF => {
                        let table_idx = (cmd - 0xD0) as usize;
                        let duration = if table_idx < CLOCK_TABLE.len() {
                            CLOCK_TABLE[table_idx] as u32
                        } else {
                            24
                        };
                        self.handle_note_command(cmd, duration, rom, events);
                    }
                    _ => {}
                }
            }
        }
    }

    fn handle_parameter(&mut self, val: u8, _rom: &[u8], events: &mut Vec<M4aEvent>) {
        match self.running_status {
            0xBD => {
                self.voice = val;
                events.push(M4aEvent::VoiceChange {
                    channel: self.channel_id,
                    voice: self.voice,
                });
            }
            0xBE => {
                self.volume = val;
                events.push(M4aEvent::VolumeChange {
                    channel: self.channel_id,
                    volume: self.volume,
                });
            }
            0xBF => {
                self.pan = val;
                events.push(M4aEvent::PanChange {
                    channel: self.channel_id,
                    pan: self.pan,
                });
            }
            _ => {
                // If no running status, treat small values as note parameter / duration
            }
        }
    }

    fn handle_note_command(&mut self, cmd: u8, duration: u32, rom: &[u8], events: &mut Vec<M4aEvent>) {
        self.running_status = cmd;
        let mut key = self.last_key;
        let mut vel = self.last_velocity;
        let mut gate = duration;

        // Check if note is followed by key argument (< 0x80)
        if self.cmd_offset < rom.len() && rom[self.cmd_offset] < 0x80 {
            key = rom[self.cmd_offset];
            self.cmd_offset += 1;
            self.last_key = key;

            // Check if followed by velocity argument (< 0x80)
            if self.cmd_offset < rom.len() && rom[self.cmd_offset] < 0x80 {
                vel = rom[self.cmd_offset];
                self.cmd_offset += 1;
                self.last_velocity = vel;

                // Check if followed by gate time modifier (< 0x80)
                if self.cmd_offset < rom.len() && rom[self.cmd_offset] < 0x80 {
                    let gate_mod = rom[self.cmd_offset] as u32;
                    self.cmd_offset += 1;
                    gate = gate_mod;
                }
            }
        }

        // Send NoteOff for any currently active note on this channel
        if let Some(prev_key) = self.active_key.take() {
            events.push(M4aEvent::NoteOff {
                channel: self.channel_id,
                key: prev_key,
            });
        }

        let effective_key = ((key as i32) + (self.key_shift as i32)).clamp(0, 127) as u8;
        self.active_key = Some(effective_key);
        self.gate_ticks = gate;

        events.push(M4aEvent::NoteOn {
            channel: self.channel_id,
            key: effective_key,
            velocity: vel,
            gate_ticks: gate,
        });
    }
}

/// Multi-track M4A sequencer
#[derive(Debug, Clone)]
pub struct M4aSequencer {
    pub header: SongHeader,
    pub tracks: Vec<SequencerTrack>,
    pub bpm: u16,
    pub tick_accumulator: f64,
    pub is_looping: bool,
    pub loop_count: usize,
    pub is_finished: bool,
}

impl M4aSequencer {
    pub fn new(header: SongHeader, rom: &[u8]) -> Self {
        let mut tracks = Vec::with_capacity(header.track_ptrs.len());
        for (i, &ptr) in header.track_ptrs.iter().enumerate() {
            let offset = rom_ptr_to_offset(rom, ptr).unwrap_or(0);
            tracks.push(SequencerTrack::new(i, offset));
        }

        Self {
            header,
            tracks,
            bpm: 120,
            tick_accumulator: 0.0,
            is_looping: true,
            loop_count: 0,
            is_finished: false,
        }
    }

    /// Step sequencer by sample duration (at 48 kHz), returning emitted events
    pub fn step_sample(&mut self, rom: &[u8], sample_rate: u32) -> Vec<M4aEvent> {
        let mut events = Vec::new();
        if self.is_finished {
            return events;
        }

        // In standard M4A, 1 quarter note = 24 ticks.
        // Ticks per second = (BPM * 24) / 60 = BPM * 0.4
        let ticks_per_second = (self.bpm as f64) * 0.4;
        let ticks_per_sample = ticks_per_second / (sample_rate as f64);
        self.tick_accumulator += ticks_per_sample;

        while self.tick_accumulator >= 1.0 {
            self.tick_accumulator -= 1.0;
            self.step_tick(rom, &mut events);
        }

        events
    }

    /// Step sequencer by 1 tick
    pub fn step_tick(&mut self, rom: &[u8], events: &mut Vec<M4aEvent>) {
        let mut all_ended = true;

        for track in &mut self.tracks {
            if let Some(event) = track.step_tick() {
                events.push(event);
            }
            track.process_commands(rom, events);

            if !track.ended {
                all_ended = false;
            }
        }

        // Check if any track looped
        for event in events.iter() {
            if let M4aEvent::SongLoop { .. } = event {
                self.loop_count += 1;
                all_ended = false;
            } else if let M4aEvent::TempoChange { bpm } = event {
                self.bpm = *bpm;
            }
        }

        if all_ended {
            self.is_finished = true;
        }
    }
}
