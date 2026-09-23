//! Integration tests for Nintendo M4A ("Sappy") Audio Interception & HD Re-Synthesis (ROADMAP M9).
//!
//! Validates:
//! 1. Database lookup and verified profiles (Emerald, Minish Cap, Fire Emblem, Castlevania, Metroid, Golden Sun).
//! 2. Universal signature search and auto-detection from ROM bytes.
//! 3. WaveData and ToneData parsing (8-bit PCM, loop points, sample rates, ADSR envelopes).
//! 4. 48 kHz stereo floating-point sampler (Linear, Cubic Hermite spline, Band-limited Sinc interpolation).
//! 5. ADSR envelope state machine and polyphony voice stealing.
//! 6. Sappy bytecode track sequencer and event dispatch.
//! 7. Standard MIDI File Type 1 (.mid) binary export.
//! 8. Multi-track 48 kHz WAV stem export and RIFF/WAVE file writer.
//! 9. Real Pokémon Emerald ROM playback, Jukebox, MIDI export, and stem export (when available).
//! 10. Non-M4A graceful fallback and live audio mode toggling.
//! 11. Save state and rewind audio synchronization.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use gba_simulator::gba::{
    m4a::{
        export_song_to_midi, render_song_to_stems, write_wav_file, AudioEngineMode,
        HdM4aSampler, M4aDatabase, M4aEvent, M4aInterpolation, M4aSequencer,
        SongHeader, ToneData, WaveData, CLOCK_TABLE,
    },
    Gba,
};

#[test]
fn test_m4a_database_lookups_and_profiles() {
    // 1. Pokémon Emerald
    let emerald = M4aDatabase::lookup("BPEE").expect("Emerald BPEE profile missing");
    assert_eq!(emerald.game_code, "BPEE");
    assert_eq!(emerald.song_table_offset, 0x6B49F0);
    assert_eq!(emerald.song_count, 610);

    let emerald_eu = M4aDatabase::lookup("BPEP").expect("Emerald BPEP profile missing");
    assert_eq!(emerald_eu.song_table_offset, 0x6B49F0);

    let emerald_jp = M4aDatabase::lookup("BPEJ").expect("Emerald BPEJ profile missing");
    assert_eq!(emerald_jp.song_table_offset, 0x63C2AC);

    // 2. The Legend of Zelda: The Minish Cap
    let minish = M4aDatabase::lookup("BZME").expect("Minish Cap BZME profile missing");
    assert_eq!(minish.song_table_offset, 0xA11DBC);
    assert!(minish.song_count >= 500);

    let minish_eu = M4aDatabase::lookup("BZMP").expect("Minish Cap BZMP profile missing");
    assert_eq!(minish_eu.song_table_offset, 0xB1D414);

    // 3. Fire Emblem: The Sacred Stones
    let fe8 = M4aDatabase::lookup("BE8E").expect("FE8 BE8E profile missing");
    assert_eq!(fe8.song_table_offset, 0x224470);
    assert!(fe8.song_count >= 1000);

    // 4. Fire Emblem: The Blazing Blade
    let fe7 = M4aDatabase::lookup("AE7E").expect("FE7 AE7E profile missing");
    assert_eq!(fe7.song_table_offset, 0x69D6D8);

    // 5. Metroid Fusion & Castlevania
    let metroid = M4aDatabase::lookup("AMFE").expect("Metroid AMFE profile missing");
    assert_eq!(metroid.song_table_offset, 0x71794C);

    let castlevania = M4aDatabase::lookup("AANE").expect("Castlevania AANE profile missing");
    assert_eq!(castlevania.song_table_offset, 0x4FA908);

    // 6. Unknown game code returns None
    assert!(M4aDatabase::lookup("ZZZZ").is_none());
    assert!(M4aDatabase::lookup("").is_none());
}

#[test]
fn test_m4a_universal_signature_auto_detection() {
    let mut fake_rom = vec![0u8; 0x10000]; // 64 KB

    // Non-M4A empty ROM should return None
    assert!(M4aDatabase::detect(&fake_rom, "TEST", "Test Game").is_none());

    // Inject CLOCK_TABLE signature at offset 0x2000
    let clock_offset = 0x2000;
    fake_rom[clock_offset..clock_offset + CLOCK_TABLE.len()].copy_from_slice(&CLOCK_TABLE);

    // Inject a candidate song table at offset 0x4000
    let table_offset = 0x4000;
    for i in 0..15 {
        let entry_off = table_offset + i * 8;
        let ptr = 0x08005000u32 + (i as u32 * 0x100);
        fake_rom[entry_off..entry_off + 4].copy_from_slice(&ptr.to_le_bytes());
        fake_rom[entry_off + 4] = 0; // ms
        fake_rom[entry_off + 5] = 0;
        fake_rom[entry_off + 6] = 0; // me
        fake_rom[entry_off + 7] = 0;

        // Populate valid header at pointed location (0x5000 + i*0x100)
        let header_off = (ptr & 0x01FFFFFF) as usize;
        if header_off + 16 <= fake_rom.len() {
            fake_rom[header_off] = 2; // track_count = 2
            fake_rom[header_off + 1] = 0; // block_count
            fake_rom[header_off + 2] = 0; // priority
            fake_rom[header_off + 3] = 0; // reverb
            let tone_ptr = 0x08006000u32;
            fake_rom[header_off + 4..header_off + 8].copy_from_slice(&tone_ptr.to_le_bytes());
            let tr1_ptr = 0x08007000u32;
            let tr2_ptr = 0x08007100u32;
            fake_rom[header_off + 8..header_off + 12].copy_from_slice(&tr1_ptr.to_le_bytes());
            fake_rom[header_off + 12..header_off + 16].copy_from_slice(&tr2_ptr.to_le_bytes());
        }
    }

    let detected = M4aDatabase::detect(&fake_rom, "CUST", "Custom M4A Title")
        .expect("Failed to detect M4A from signature and candidate table");
    assert_eq!(detected.game_code, "CUST");
    assert_eq!(detected.song_table_offset, table_offset);
    assert_eq!(detected.song_count, 200);
}

#[test]
fn test_m4a_voice_and_sample_parsing() {
    let mut rom = vec![0u8; 0x8000];

    // Build WaveData at ROM offset 0x1000 (GBA ptr 0x08001000)
    let wave_off = 0x1000;
    let wave_ptr = 0x08000000 + wave_off as u32;

    let sample_rate = 13379u32;
    let freq = sample_rate * 4096;
    let loop_start = 16u32;
    let sample_count = 64u32;

    rom[wave_off..wave_off + 2].copy_from_slice(&0u16.to_le_bytes()); // wav_type
    rom[wave_off + 2..wave_off + 4].copy_from_slice(&0x4000u16.to_le_bytes()); // status (looped)
    rom[wave_off + 4..wave_off + 8].copy_from_slice(&freq.to_le_bytes());
    rom[wave_off + 8..wave_off + 12].copy_from_slice(&loop_start.to_le_bytes());
    rom[wave_off + 12..wave_off + 16].copy_from_slice(&sample_count.to_le_bytes());

    // Generate ramp waveform (-64 to +63)
    for i in 0..sample_count as usize {
        rom[wave_off + 16 + i] = ((i as i8) * 2 - 64) as u8;
    }

    let parsed_wave = WaveData::parse_from_rom(&rom, wave_ptr)
        .expect("Failed to parse valid WaveData from ROM");

    assert!(parsed_wave.is_looped());
    assert_eq!(parsed_wave.loop_start, 16);
    assert_eq!(parsed_wave.size, 64);
    assert_eq!(parsed_wave.samples.len(), 64);
    assert!((parsed_wave.base_sample_rate() - 13379.0).abs() < 1.0);

    // Build ToneData at ROM offset 0x2000 (GBA ptr 0x08002000)
    let tone_off = 0x2000;
    let tone_ptr = 0x08000000 + tone_off as u32;

    rom[tone_off] = 0x00; // DirectSound PCM
    rom[tone_off + 1] = 60; // Root key = middle C
    rom[tone_off + 2] = 0; // length
    rom[tone_off + 3] = 0; // pan_sweep
    rom[tone_off + 4..tone_off + 8].copy_from_slice(&wave_ptr.to_le_bytes());
    rom[tone_off + 8] = 250; // Attack
    rom[tone_off + 9] = 180; // Decay
    rom[tone_off + 10] = 200; // Sustain
    rom[tone_off + 11] = 120; // Release

    let parsed_tone = ToneData::parse_from_rom(&rom, tone_ptr, 0)
        .expect("Failed to parse ToneData from ROM");

    assert_eq!(parsed_tone.tone_type, 0);
    assert_eq!(parsed_tone.key, 60);
    assert_eq!(parsed_tone.attack, 250);
    assert_eq!(parsed_tone.decay, 180);
    assert_eq!(parsed_tone.sustain, 200);
    assert_eq!(parsed_tone.release, 120);
    assert!(parsed_tone.wave.is_some());
    assert_eq!(parsed_tone.wave.as_ref().unwrap().size, 64);
}

#[test]
fn test_m4a_sampler_interpolation_and_adsr() {
    let mut sampler = HdM4aSampler::new(16, 48000);
    sampler.reverb_enabled = false;
    sampler.master_volume = 1.0;

    // Build synthetic wave and tone
    let samples: Vec<i8> = (0..256).map(|i| ((i as f32 * std::f32::consts::TAU / 256.0).sin() * 120.0) as i8).collect();
    let wave = Arc::new(WaveData {
        wav_type: 0,
        status: 0x4000,
        freq: 13379 * 4096,
        loop_start: 0,
        size: 256,
        samples,
    });

    let tone = ToneData {
        tone_type: 0,
        key: 60,
        length: 0,
        pan_sweep: 0,
        wav_ptr: 0,
        attack: 255,
        decay: 200,
        sustain: 200,
        release: 200,
        wave: Some(wave.clone()),
    };

    let interpolation_modes = [
        M4aInterpolation::Linear,
        M4aInterpolation::CubicHermite,
        M4aInterpolation::Sinc,
    ];

    for &interp in &interpolation_modes {
        sampler.stop_all();
        sampler.interpolation = interp;

        // Trigger note 60 (Middle C), vel 127, pan 64 (center), vol 100
        sampler.note_on(0, 60, 127, &tone, Some(wave.clone()), 64, 100, 0, 0);
        let active_count = sampler.voices.iter().filter(|v| v.is_active).count();
        assert_eq!(active_count, 1);

        // Step 480 samples (10ms @ 48 kHz)
        let mut has_audio = false;
        for _ in 0..480 {
            let (l, r) = sampler.render_sample();
            assert!(l.is_finite() && !l.is_nan());
            assert!(r.is_finite() && !r.is_nan());
            assert!(l.abs() <= 1.0);
            assert!(r.abs() <= 1.0);
            if l.abs() > 0.01 {
                has_audio = true;
            }
        }
        assert!(has_audio, "Expected audible output for {:?}", interp);

        // Note off -> enters release phase
        sampler.note_off(0, 60);
        let mut decaying = false;
        let mut prev_l = 1.0;
        for _ in 0..2000 {
            let (l, _) = sampler.render_sample();
            if l.abs() < prev_l {
                decaying = true;
            }
            prev_l = l.abs();
        }
        assert!(decaying, "Envelope should decay on note off");
    }
}

#[test]
fn test_m4a_polyphony_voice_stealing() {
    let mut sampler = HdM4aSampler::new(4, 48000); // Only 4 voices maximum
    sampler.reverb_enabled = false;

    let wave = Arc::new(WaveData {
        wav_type: 0,
        status: 0x4000,
        freq: 13379 * 4096,
        loop_start: 0,
        size: 32,
        samples: vec![60; 32],
    });

    let tone = ToneData {
        tone_type: 0,
        key: 60,
        length: 0,
        pan_sweep: 0,
        wav_ptr: 0,
        attack: 255,
        decay: 255,
        sustain: 255,
        release: 255,
        wave: Some(wave.clone()),
    };

    // Trigger 8 notes rapidly
    for key in 60..68 {
        sampler.note_on(0, key, 100, &tone, Some(wave.clone()), 64, 100, 0, 0);
    }

    // Must be clamped to max capacity of 4 voices
    let active_count = sampler.voices.iter().filter(|v| v.is_active).count();
    assert_eq!(active_count, 4);

    // Sampler steps cleanly without crash
    for _ in 0..100 {
        let (l, r) = sampler.render_sample();
        assert!(l.is_finite() && r.is_finite());
    }
}

#[test]
fn test_m4a_sequencer_bytecode_dispatch() {
    let mut rom = vec![0u8; 0x4000];

    // Track 1 bytecode at 0x1000:
    // TEMPO 120 (0xBB, 120)
    // VOICE 5   (0xBD, 5)
    // VOL 110   (0xBE, 110)
    // PAN 32    (0xBF, 32)
    // BEND +8   (0xC0, 8)
    // Note C4   (0xD0, 60, 127)
    // Wait tick (0x01)
    // FINE      (0xB1)
    let tr_off = 0x1000;
    let track_data = [
        0xBB, 60,        // TEMPO 120 BPM (60 * 2)
        0xBD, 5,         // VOICE 5
        0xBE, 110,       // VOL 110
        0xBF, 32,        // PAN 32
        0xC0, 72,        // BEND +8 (center 64 + 8)
        0xD0, 60, 127,   // Note command (duration 1, key 60, vel 127)
        0x01,            // Wait tick
        0xB1,            // FINE
    ];
    rom[tr_off..tr_off + track_data.len()].copy_from_slice(&track_data);

    let song_header = SongHeader {
        track_count: 1,
        block_count: 0,
        priority: 0,
        reverb: 0,
        tone_ptr: 0x08002000,
        track_ptrs: vec![0x08000000 + tr_off as u32],
    };

    let mut sequencer = M4aSequencer::new(song_header, &rom);
    assert!(!sequencer.is_finished);

    let mut saw_tempo = false;
    let mut saw_voice = false;
    let mut saw_vol = false;
    let mut saw_pan = false;
    let mut saw_bend = false;
    let mut saw_note_on = false;

    // Step across ticks
    for _ in 0..5000 {
        let events = sequencer.step_sample(&rom, 48000);
        for ev in events {
            match ev {
                M4aEvent::TempoChange { bpm } if bpm == 120 => saw_tempo = true,
                M4aEvent::VoiceChange { voice, .. } if voice == 5 => saw_voice = true,
                M4aEvent::VolumeChange { volume, .. } if volume == 110 => saw_vol = true,
                M4aEvent::PanChange { pan, .. } if pan == 32 => saw_pan = true,
                M4aEvent::PitchBend { bend, .. } if bend == 8 => saw_bend = true,
                M4aEvent::NoteOn { key, velocity, .. } if key == 60 && velocity == 127 => saw_note_on = true,
                _ => {}
            }
        }
        if sequencer.is_finished {
            break;
        }
    }

    assert!(saw_tempo, "Expected TempoChange event");
    assert!(saw_voice, "Expected VoiceChange event");
    assert!(saw_vol, "Expected VolumeChange event");
    assert!(saw_pan, "Expected PanChange event");
    assert!(saw_bend, "Expected PitchBend event");
    assert!(saw_note_on, "Expected NoteOn event");
    assert!(sequencer.is_finished, "Sequencer should reach FINE and finish");
}

#[test]
fn test_m4a_midi_export() {
    let mut rom = vec![0u8; 0x4000];
    let tr_off = 0x1000;
    let track_data = [
        0xBB, 140,       // TEMPO 140
        0xBD, 10,        // VOICE 10
        0xBE, 127,       // VOL 127
        0xD4, 64, 100,   // Note command
        0x04,            // Wait 4 ticks
        0xB1,            // FINE
    ];
    rom[tr_off..tr_off + track_data.len()].copy_from_slice(&track_data);

    let song_header = SongHeader {
        track_count: 1,
        block_count: 0,
        priority: 0,
        reverb: 0,
        tone_ptr: 0x08002000,
        track_ptrs: vec![0x08000000 + tr_off as u32],
    };

    let midi_bytes = export_song_to_midi(&rom, &song_header, "Test Song")
        .expect("MIDI export failed");
    assert!(midi_bytes.len() > 14);

    // Check MIDI Header Chunk "MThd"
    assert_eq!(&midi_bytes[0..4], b"MThd");
    let chunk_len = u32::from_be_bytes([midi_bytes[4], midi_bytes[5], midi_bytes[6], midi_bytes[7]]);
    assert_eq!(chunk_len, 6);

    let format = u16::from_be_bytes([midi_bytes[8], midi_bytes[9]]);
    assert_eq!(format, 1, "Expected MIDI File Format 1");

    let num_tracks = u16::from_be_bytes([midi_bytes[10], midi_bytes[11]]);
    assert_eq!(num_tracks, 2, "Expected 2 tracks (Tempo track + Track 1)");

    let division = u16::from_be_bytes([midi_bytes[12], midi_bytes[13]]);
    assert_eq!(division, 24, "Expected 24 PPQN");

    // Check that track chunk starts with "MTrk"
    assert_eq!(&midi_bytes[14..18], b"MTrk");

    // Track must end with End of Track meta event: FF 2F 00
    assert!(midi_bytes.windows(3).any(|w| w == [0xFF, 0x2F, 0x00]));
}

#[test]
fn test_m4a_stem_export_and_wav_writer() {
    let mut rom = vec![0u8; 0x4000];
    let tr_off = 0x1000;
    let track_data = [
        0xBB, 120,
        0xBD, 0,
        0xD0, 60, 100,
        0x02,
        0xB1,
    ];
    rom[tr_off..tr_off + track_data.len()].copy_from_slice(&track_data);

    let song_header = SongHeader {
        track_count: 1,
        block_count: 0,
        priority: 0,
        reverb: 0,
        tone_ptr: 0x08002000,
        track_ptrs: vec![0x08000000 + tr_off as u32],
    };

    let stems = render_song_to_stems(
        &rom,
        &song_header,
        0.1, // 100ms
        48000,
        M4aInterpolation::CubicHermite,
    );

    assert_eq!(stems.len(), 2); // Master Mix + 1 Track stem
    assert_eq!(stems[0].name, "Master Mix");
    assert_eq!(stems[0].samples.len(), 4800 * 2); // 4800 stereo samples = 9600 floats
    assert_eq!(stems[1].name, "Stem 01 Track 1");

    // Test writing WAV file to disk
    let temp_dir = std::env::temp_dir();
    let wav_path = temp_dir.join("test_stem_render.wav");

    write_wav_file(&wav_path, &stems[0].samples, 48000).expect("Failed to write WAV file");

    let wav_bytes = fs::read(&wav_path).expect("Failed to read back written WAV file");
    assert_eq!(&wav_bytes[0..4], b"RIFF");
    assert_eq!(&wav_bytes[8..12], b"WAVE");
    assert_eq!(&wav_bytes[12..16], b"fmt ");

    // Audio format = 1 (PCM), channels = 2, sample_rate = 48000, bits = 16
    let channels = u16::from_le_bytes([wav_bytes[22], wav_bytes[23]]);
    let rate = u32::from_le_bytes([wav_bytes[24], wav_bytes[25], wav_bytes[26], wav_bytes[27]]);
    let bits = u16::from_le_bytes([wav_bytes[34], wav_bytes[35]]);
    assert_eq!(channels, 2);
    assert_eq!(rate, 48000);
    assert_eq!(bits, 16);

    // Verify "data" sub-chunk
    assert_eq!(&wav_bytes[36..40], b"data");
    let data_len = u32::from_le_bytes([wav_bytes[40], wav_bytes[41], wav_bytes[42], wav_bytes[43]]);
    assert_eq!(data_len, 4800 * 2 * 2); // 4800 frames * 2 channels * 2 bytes/sample

    let _ = fs::remove_file(wav_path);
}

#[test]
fn test_m4a_non_m4a_graceful_fallback_and_mode_toggle() {
    let mut gba = Gba::new();
    let dummy_rom = vec![0u8; 0x10000];
    gba.load_rom_bytes(dummy_rom);

    // Non-M4A game: is_m4a_game() should be false
    assert!(!gba.is_m4a_game());

    // When set to HdReSynthesis mode, non-M4A game must gracefully fall back to native APU without panic
    gba.set_hd_audio_mode(AudioEngineMode::HdReSynthesis);
    assert_eq!(gba.hd_audio_mode(), AudioEngineMode::HdReSynthesis);

    for _ in 0..10 {
        gba.run_frame();
    }

    // Toggle back to HardwareOnly mode
    gba.set_hd_audio_mode(AudioEngineMode::HardwareOnly);
    assert_eq!(gba.hd_audio_mode(), AudioEngineMode::HardwareOnly);

    for _ in 0..10 {
        gba.run_frame();
    }
}

#[test]
fn test_m4a_pokemon_emerald_playback_and_export() {
    let rom_path = Path::new("/home/ssilk/Downloads/Pokemon - Emerald Version (USA, Europe).gba");
    if !rom_path.exists() {
        eprintln!("Skipping Pokémon Emerald test: ROM not found at {}", rom_path.display());
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(rom_path).expect("Failed to load Pokémon Emerald ROM");

    // 1. Verify auto-detection identified Emerald
    assert!(gba.is_m4a_game(), "Pokémon Emerald must be detected as an M4A game");
    let profile = gba.m4a.profile.as_ref().expect("Missing M4A profile for Emerald");
    assert_eq!(profile.game_code, "BPEE");
    assert_eq!(profile.song_table_offset, 0x6B49F0);

    // 2. Test Jukebox playback for Littleroot Town (Song 405)
    let played = gba.play_m4a_song(405);
    assert!(played, "Failed to start Littleroot Town playback via Jukebox");

    // Run emulator frames to let HD audio refill and mix
    let mut total_samples_refilled = 0;
    for _ in 0..30 {
        gba.run_frame();
        total_samples_refilled += gba.mmu.apu.hd_sample_stream.len();
    }
    assert!(total_samples_refilled > 0, "HD audio stream should have produced samples");

    // 3. Test Standard MIDI File export for Song 405
    let midi_bytes = gba.export_m4a_song_midi(405).expect("Failed to export Emerald Song 405 to MIDI");
    assert!(midi_bytes.len() > 100);
    assert_eq!(&midi_bytes[0..4], b"MThd");
    assert!(midi_bytes.windows(4).any(|w| w == b"MTrk"));

    // 4. Test multi-track stem export for 1.0 second
    let stems = gba.export_m4a_song_stems(405, 1.0).expect("Failed to export Emerald Song 405 stems");
    assert!(stems.len() >= 2, "Expected at least Master Mix and one track stem");
    assert_eq!(stems[0].name, "Master Mix");
    assert_eq!(stems[0].samples.len(), 48000 * 2);

    // Verify samples are non-silent
    let max_amp = stems[0].samples.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
    assert!(max_amp > 0.001, "Master mix stem should contain non-silent audio, got max_amp = {}", max_amp);

    gba.stop_m4a_song();
}

#[test]
fn test_m4a_save_state_synchronization() {
    let rom_path = Path::new("/home/ssilk/Downloads/Pokemon - Emerald Version (USA, Europe).gba");
    if !rom_path.exists() {
        eprintln!("Skipping save state test: ROM not found at {}", rom_path.display());
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(rom_path).expect("Failed to load Pokémon Emerald");
    gba.set_hd_audio_mode(AudioEngineMode::HdReSynthesis);

    // Play BGM and run 60 frames
    gba.play_m4a_song(413); // Title Theme
    for _ in 0..60 {
        gba.run_frame();
    }

    // Save state
    let state_blob = gba.save_state();
    assert!(!state_blob.is_empty());

    // Step further
    for _ in 0..30 {
        gba.run_frame();
    }

    // Restore state
    let mut restored = Gba::new();
    restored.load_rom(rom_path).expect("Failed to load ROM into restored instance");
    restored.set_hd_audio_mode(AudioEngineMode::HdReSynthesis);
    let success = restored.load_state(&state_blob);
    assert!(success, "load_state failed");

    // Stepping restored GBA should continue generating frames without panic
    for _ in 0..30 {
        restored.run_frame();
    }
}
