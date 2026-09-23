//! Standard MIDI File (SMF Type 1) Exporter
//!
//! Converts M4A / Sappy song headers and tracks into standard .mid files
//! with accurate tempo, note velocities, pitch bends, panning, and instruments.

use super::song::{SongHeader, CLOCK_TABLE};
use super::voice::rom_ptr_to_offset;

/// Export an M4A song as Standard MIDI File Type 1 (.mid) bytes
pub fn export_song_to_midi(rom: &[u8], song_header: &SongHeader, song_name: &str) -> Result<Vec<u8>, String> {
    let mut midi = Vec::with_capacity(8192);

    let num_midi_tracks = (song_header.track_count as usize) + 1; // 1 conductor + N music tracks
    const PPQN: u16 = 24; // 24 ticks per quarter note in Sappy

    // 1. MThd Header Chunk
    midi.extend_from_slice(b"MThd");
    midi.extend_from_slice(&6u32.to_be_bytes()); // chunk length
    midi.extend_from_slice(&1u16.to_be_bytes()); // Format 1: Multi-track
    midi.extend_from_slice(&(num_midi_tracks as u16).to_be_bytes());
    midi.extend_from_slice(&PPQN.to_be_bytes()); // Division: 24 PPQN

    // 2. Conductor Track (Track 0: Tempo and Title)
    let mut conductor_events = Vec::new();
    // Song Name meta event
    write_vlq(&mut conductor_events, 0);
    conductor_events.push(0xFF);
    conductor_events.push(0x03); // Sequence Name
    let title_bytes = song_name.as_bytes();
    write_vlq(&mut conductor_events, title_bytes.len() as u32);
    conductor_events.extend_from_slice(title_bytes);

    // Initial Tempo (default 120 BPM if not changed in track)
    let initial_bpm = detect_initial_tempo(rom, song_header).unwrap_or(120);
    let us_per_quarter = 60_000_000 / (initial_bpm as u32);
    write_vlq(&mut conductor_events, 0);
    conductor_events.push(0xFF);
    conductor_events.push(0x51); // Set Tempo
    conductor_events.push(0x03);
    conductor_events.push(((us_per_quarter >> 16) & 0xFF) as u8);
    conductor_events.push(((us_per_quarter >> 8) & 0xFF) as u8);
    conductor_events.push((us_per_quarter & 0xFF) as u8);

    // End of Track
    write_vlq(&mut conductor_events, 0);
    conductor_events.push(0xFF);
    conductor_events.push(0x2F);
    conductor_events.push(0x00);

    write_track_chunk(&mut midi, &conductor_events);

    // 3. Music Tracks (Tracks 1..=N)
    for (tr_idx, &track_ptr) in song_header.track_ptrs.iter().enumerate() {
        let midi_ch = (tr_idx % 16) as u8;
        let mut tr_events = Vec::new();

        // Track Name
        write_vlq(&mut tr_events, 0);
        tr_events.push(0xFF);
        tr_events.push(0x03);
        let tr_name = format!("Track {} (Ch {})", tr_idx + 1, midi_ch + 1);
        write_vlq(&mut tr_events, tr_name.len() as u32);
        tr_events.extend_from_slice(tr_name.as_bytes());

        // Parse track bytecode into MIDI events
        if let Some(start_off) = rom_ptr_to_offset(rom, track_ptr) {
            parse_track_to_midi_events(rom, start_off, midi_ch, &mut tr_events);
        } else {
            // Empty track fallback
            write_vlq(&mut tr_events, 0);
            tr_events.push(0xFF);
            tr_events.push(0x2F);
            tr_events.push(0x00);
        }

        write_track_chunk(&mut midi, &tr_events);
    }

    Ok(midi)
}

fn write_track_chunk(out: &mut Vec<u8>, events: &[u8]) {
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(events.len() as u32).to_be_bytes());
    out.extend_from_slice(events);
}

fn write_vlq(out: &mut Vec<u8>, mut val: u32) {
    let mut buf = [0u8; 5];
    let mut len = 0;
    buf[len] = (val & 0x7F) as u8;
    val >>= 7;
    while val > 0 {
        len += 1;
        buf[len] = ((val & 0x7F) | 0x80) as u8;
        val >>= 7;
    }
    for i in (0..=len).rev() {
        out.push(buf[i]);
    }
}

fn detect_initial_tempo(rom: &[u8], header: &SongHeader) -> Option<u16> {
    for &tr_ptr in &header.track_ptrs {
        if let Some(mut off) = rom_ptr_to_offset(rom, tr_ptr) {
            let end = (off + 64).min(rom.len());
            while off < end {
                let cmd = rom[off];
                off += 1;
                if cmd == 0xBB && off < rom.len() {
                    return Some((rom[off] as u16) * 2);
                } else if cmd == 0xB1 {
                    break;
                }
            }
        }
    }
    None
}

struct PendingNoteOff {
    tick: u32,
    key: u8,
}

fn parse_track_to_midi_events(rom: &[u8], start_offset: usize, channel: u8, out: &mut Vec<u8>) {
    let mut offset = start_offset;
    let mut current_tick: u32 = 0;
    let mut last_event_tick: u32 = 0;

    let mut running_status = 0u8;
    let mut last_key = 60u8;
    let mut last_vel = 100u8;
    let mut key_shift: i8 = 0;
    let mut call_stack = Vec::new();
    let mut pending_note_offs: Vec<PendingNoteOff> = Vec::new();

    let mut loop_count = 0;
    let mut ended = false;

    while !ended && offset < rom.len() && loop_count < 5000 {
        loop_count += 1;

        // Drain any pending note offs that are due at or before current_tick
        pending_note_offs.sort_by_key(|n| n.tick);
        while let Some(first) = pending_note_offs.first() {
            if first.tick <= current_tick {
                let note = pending_note_offs.remove(0);
                let delta = note.tick.saturating_sub(last_event_tick);
                write_vlq(out, delta);
                out.push(0x80 | channel);
                out.push(note.key);
                out.push(0);
                last_event_tick = note.tick;
            } else {
                break;
            }
        }

        let cmd = rom[offset];
        offset += 1;

        if (0x80..=0xB0).contains(&cmd) {
            // Wait command
            let wait = (cmd - 0x80) as u32;
            current_tick += wait;
        } else if cmd <= 0x7F {
            // Parameter repetition
            match running_status {
                0xBD => {
                    let delta = current_tick.saturating_sub(last_event_tick);
                    write_vlq(out, delta);
                    out.push(0xC0 | channel);
                    out.push(cmd.min(127));
                    last_event_tick = current_tick;
                }
                0xBE => {
                    let delta = current_tick.saturating_sub(last_event_tick);
                    write_vlq(out, delta);
                    out.push(0xB0 | channel);
                    out.push(0x07); // Volume
                    out.push(cmd.min(127));
                    last_event_tick = current_tick;
                }
                0xBF => {
                    let delta = current_tick.saturating_sub(last_event_tick);
                    write_vlq(out, delta);
                    out.push(0xB0 | channel);
                    out.push(0x0A); // Pan
                    out.push(cmd.min(127));
                    last_event_tick = current_tick;
                }
                _ => {}
            }
        } else {
            match cmd {
                // FINE: End of track
                0xB1 => {
                    ended = true;
                }
                // GOTO: Loop or branch
                0xB2 => {
                    if offset + 4 <= rom.len() {
                        let ptr = u32::from_le_bytes([rom[offset], rom[offset + 1], rom[offset + 2], rom[offset + 3]]);
                        offset += 4;
                        if let Some(target) = rom_ptr_to_offset(rom, ptr) {
                            if target < offset {
                                // Backward loop -> terminate MIDI track to avoid infinite output
                                ended = true;
                            } else {
                                offset = target;
                            }
                        } else {
                            ended = true;
                        }
                    }
                }
                // PATT: Subroutine call
                0xB3 => {
                    if offset + 4 <= rom.len() {
                        let ptr = u32::from_le_bytes([rom[offset], rom[offset + 1], rom[offset + 2], rom[offset + 3]]);
                        offset += 4;
                        if let Some(target) = rom_ptr_to_offset(rom, ptr) {
                            call_stack.push(offset);
                            offset = target;
                        }
                    }
                }
                // PEND: Subroutine return
                0xB4 => {
                    if let Some(ret) = call_stack.pop() {
                        offset = ret;
                    } else {
                        ended = true;
                    }
                }
                // KEYSH: Transpose
                0xBC => {
                    if offset < rom.len() {
                        key_shift = rom[offset] as i8;
                        offset += 1;
                    }
                }
                // VOICE: Instrument change
                0xBD => {
                    if offset < rom.len() {
                        let voice = rom[offset].min(127);
                        offset += 1;
                        running_status = 0xBD;
                        let delta = current_tick.saturating_sub(last_event_tick);
                        write_vlq(out, delta);
                        out.push(0xC0 | channel);
                        out.push(voice);
                        last_event_tick = current_tick;
                    }
                }
                // VOL: Volume
                0xBE => {
                    if offset < rom.len() {
                        let vol = rom[offset].min(127);
                        offset += 1;
                        running_status = 0xBE;
                        let delta = current_tick.saturating_sub(last_event_tick);
                        write_vlq(out, delta);
                        out.push(0xB0 | channel);
                        out.push(0x07);
                        out.push(vol);
                        last_event_tick = current_tick;
                    }
                }
                // PAN: Panning
                0xBF => {
                    if offset < rom.len() {
                        let pan = rom[offset].min(127);
                        offset += 1;
                        running_status = 0xBF;
                        let delta = current_tick.saturating_sub(last_event_tick);
                        write_vlq(out, delta);
                        out.push(0xB0 | channel);
                        out.push(0x0A);
                        out.push(pan);
                        last_event_tick = current_tick;
                    }
                }
                // BEND: Pitch bend
                0xC0 => {
                    if offset < rom.len() {
                        let bend = rom[offset];
                        offset += 1;
                        let bend_val = (bend as u16) << 7; // map 0..127 to 14-bit
                        let delta = current_tick.saturating_sub(last_event_tick);
                        write_vlq(out, delta);
                        out.push(0xE0 | channel);
                        out.push((bend_val & 0x7F) as u8);
                        out.push(((bend_val >> 7) & 0x7F) as u8);
                        last_event_tick = current_tick;
                    }
                }
                // Note commands: TIE (0xCF) or 0xD0..=0xFF
                0xCF..=0xFF => {
                    running_status = cmd;
                    let duration = if cmd == 0xCF {
                        24 // Default quarter note for tie
                    } else {
                        let idx = (cmd - 0xD0) as usize;
                        if idx < CLOCK_TABLE.len() {
                            CLOCK_TABLE[idx] as u32
                        } else {
                            24
                        }
                    };

                    let mut key = last_key;
                    let mut vel = last_vel;
                    let mut gate = duration;

                    if offset < rom.len() && rom[offset] < 0x80 {
                        key = rom[offset];
                        offset += 1;
                        last_key = key;

                        if offset < rom.len() && rom[offset] < 0x80 {
                            vel = rom[offset];
                            offset += 1;
                            last_vel = vel;

                            if offset < rom.len() && rom[offset] < 0x80 {
                                gate = rom[offset] as u32;
                                offset += 1;
                            }
                        }
                    }

                    let effective_key = ((key as i32) + (key_shift as i32)).clamp(0, 127) as u8;

                    // Note On event
                    let delta = current_tick.saturating_sub(last_event_tick);
                    write_vlq(out, delta);
                    out.push(0x90 | channel);
                    out.push(effective_key);
                    out.push(vel.clamp(1, 127));
                    last_event_tick = current_tick;

                    // Schedule Note Off event
                    pending_note_offs.push(PendingNoteOff {
                        tick: current_tick + gate.max(1),
                        key: effective_key,
                    });
                }
                _ => {}
            }
        }
    }

    // Flush any remaining note offs
    pending_note_offs.sort_by_key(|n| n.tick);
    for note in pending_note_offs {
        let delta = note.tick.saturating_sub(last_event_tick);
        write_vlq(out, delta);
        out.push(0x80 | channel);
        out.push(note.key);
        out.push(0);
        last_event_tick = note.tick;
    }

    // End of Track meta-event
    write_vlq(out, 0);
    out.push(0xFF);
    out.push(0x2F);
    out.push(0x00);
}
