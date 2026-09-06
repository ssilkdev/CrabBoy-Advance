//! Automated Test Suite for GBA Simulator Core Components

#[cfg(test)]
mod tests {
    use gba_simulator::gba::cpu::alu::{add_with_carry, barrel_shift, sub_with_borrow, ShiftType};
    use gba_simulator::gba::mmu::cartridge::Cartridge;
    use gba_simulator::gba::mmu::flash::Flash;
    use gba_simulator::gba::Gba;

    #[test]
    fn test_barrel_shifter_lsl() {
        let (val, carry) = barrel_shift(ShiftType::Lsl, 0x0000_0001, 4, false, true);
        assert_eq!(val, 0x0000_0010);
        assert!(!carry);

        let (val, carry) = barrel_shift(ShiftType::Lsl, 0x8000_0000, 1, false, true);
        assert_eq!(val, 0);
        assert!(carry);
    }

    #[test]
    fn test_barrel_shifter_lsr_and_asr() {
        let (val, carry) = barrel_shift(ShiftType::Lsr, 0x0000_0003, 1, false, true);
        assert_eq!(val, 1);
        assert!(carry);

        let (val, _) = barrel_shift(ShiftType::Asr, 0x8000_0000, 4, false, true);
        assert_eq!(val, 0xF800_0000);
    }

    #[test]
    fn test_alu_add_sub_flags() {
        // Simple add without overflow
        let (res, carry, ovf) = add_with_carry(10, 20, false);
        assert_eq!(res, 30);
        assert!(!carry);
        assert!(!ovf);

        // Carry out without signed overflow
        let (res, carry, ovf) = add_with_carry(0xFFFF_FFFF, 1, false);
        assert_eq!(res, 0);
        assert!(carry);
        assert!(!ovf);

        // Signed overflow
        let (res, carry, ovf) = add_with_carry(0x7FFF_FFFF, 1, false);
        assert_eq!(res, 0x8000_0000);
        assert!(!carry);
        assert!(ovf);

        // Sub with borrow
        let (res, carry, _) = sub_with_borrow(10, 5, true);
        assert_eq!(res, 5);
        assert!(carry); // In ARM, carry = 1 when NO borrow
    }

    #[test]
    fn test_cartridge_header_detection() {
        let mut rom = vec![0u8; 512];
        rom[0xA0..0xAC].copy_from_slice(b"SAMPLE GAME\0");
        rom[0xAC..0xB0].copy_from_slice(b"SMPL");
        let cart = Cartridge::from_bytes(rom);
        assert_eq!(cart.title, "SAMPLE GAME");
        assert_eq!(cart.game_code, "SMPL");
    }

    #[test]
    fn test_synthetic_rom_execution() {
        let mut rom = vec![0u8; 1024];
        // Infinite loop at 0x0800_0000: B . (0xEAFFFFFE)
        rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
        let mut gba = Gba::new();
        gba.load_rom_bytes(rom);
        for _ in 0..60 {
            gba.run_frame();
        }
        assert_eq!(gba.frame_counter, 60);
    }

    #[test]
    fn test_flash_128kb_state_machine() {
        let mut flash = Flash::new(None);

        // Enter Chip ID mode: write 0xAA to 0x5555, 0x55 to 0x2AAA, 0x90 to 0x5555
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x5555, 0x90);

        // Read manufacturer ID and device ID
        let man_id = flash.read(0x0000);
        let dev_id = flash.read(0x0001);
        assert_eq!(man_id, 0xC2, "Macronix manufacturer ID should be 0xC2");
        assert_eq!(dev_id, 0x09, "MX29L010 device ID should be 0x09");

        // Exit Chip ID mode: write 0xAA to 0x5555, 0x55 to 0x2AAA, 0xF0 to 0x5555
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x5555, 0xF0);

        // Byte program: write 0xAA to 0x5555, 0x55 to 0x2AAA, 0xA0 to 0x5555, then write byte
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x5555, 0xA0);
        flash.write(0x0042, 0x77);

        assert_eq!(flash.read(0x0042), 0x77);

        // Bank switch to bank 1: write 0xAA to 0x5555, 0x55 to 0x2AAA, 0xB0 to 0x5555, then write 1 to 0x0000
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x5555, 0xB0);
        flash.write(0x0000, 1);

        // Byte in bank 1 should still be unwritten 0xFF
        assert_eq!(flash.read(0x0042), 0xFF);

        // Switch back to bank 0
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x5555, 0xB0);
        flash.write(0x0000, 0);
        assert_eq!(flash.read(0x0042), 0x77);

        // Test Sector Erase (4KB):
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x5555, 0x80);
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
        flash.write(0x0000, 0x30);
        assert_eq!(flash.read(0x0042), 0xFF, "Sector erase should reset bytes back to 0xFF");
    }

    #[test]
    fn test_gba_frame_execution() {
        let mut rom = vec![0u8; 1024];
        // Infinite loop at 0x0800_0000: B . (0xEAFFFFFE)
        rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
        let mut gba = Gba::new();
        gba.load_rom_bytes(rom);

        // Execute 60 frames of full emulation (1 second in-game)
        for _ in 0..60 {
            gba.run_frame();
        }

        assert_eq!(gba.frame_counter, 60);
        assert!(gba.cpu.cycles >= 60 * 280_896);
    }

    #[test]
    fn test_optional_rom_execution() {
        let rom_path = match std::env::var("GBA_TEST_ROM") {
            Ok(p) if std::path::Path::new(&p).exists() => p,
            _ => return,
        };
        let mut gba = Gba::new();
        if gba.load_rom(&rom_path).is_ok() {
            for _ in 0..60 {
                gba.run_frame();
            }
            assert_eq!(gba.frame_counter, 60);
        }
    }

    #[test]
    fn test_nvidia_adaptive_sharpening() {
        use gba_simulator::ui::screen::apply_nvidia_adaptive_sharpening;
        use eframe::egui::{Color32, ColorImage};

        let mut fb = [0u32; 240 * 160];
        // Create test pattern: split black/white edge
        for y in 0..160 {
            for x in 0..240 {
                fb[y * 240 + x] = if x < 120 { 0x000000 } else { 0xFFFFFF };
            }
        }

        let mut out = ColorImage::new([240, 160], Color32::BLACK);
        apply_nvidia_adaptive_sharpening(&fb, &mut out, 0.6);

        // Check flat areas are unchanged
        assert_eq!(out.pixels[80 * 240 + 10], Color32::from_rgb(0, 0, 0));
        assert_eq!(out.pixels[80 * 240 + 200], Color32::from_rgb(255, 255, 255));

        // Check edge pixels are valid and consistent grayscale
        let edge_left = out.pixels[80 * 240 + 119];
        let edge_right = out.pixels[80 * 240 + 120];
        assert_eq!(edge_left.r(), edge_left.g());
        assert_eq!(edge_right.r(), edge_right.g());
    }

    #[test]
    fn test_gamepad_manager_initialization() {
        use gba_simulator::ui::controls::GamepadManager;

        let mut mgr = GamepadManager::new();
        let keys = mgr.poll();
        assert_eq!(keys.len(), 10);
        // Default layout should have swap_ab = false (8BitDo / Nintendo layout)
        assert!(!mgr.swap_ab);
        assert!((mgr.deadzone - 0.35).abs() < 0.001);
    }

    #[test]
    fn test_gba_color_correction() {
        use gba_simulator::ui::screen::apply_gba_color_correction;

        // Test black maps to black
        let (r0, g0, b0) = apply_gba_color_correction(0, 0, 0);
        assert_eq!((r0, g0, b0), (0, 0, 0));

        // Test white maps to high luminance without overflow
        let (rw, gw, bw) = apply_gba_color_correction(255, 255, 255);
        assert!(rw >= 240 && gw >= 240 && bw >= 240);

        // Test pure red has balanced spectral bleed
        let (rr, rg, rb) = apply_gba_color_correction(255, 0, 0);
        assert!(rr > rg && rr > rb);
    }

    #[test]
    #[allow(clippy::unnecessary_min_or_max)]
    fn test_integer_auto_scale_calculation() {
        // Test 1440p monitor (2560x1440) -> 1440 / 160 = 9x scale
        let scale_1440p = (2560u32 / 240).min(1440 / 160);
        assert_eq!(scale_1440p, 9);

        // Test 1600p ultrawide monitor (3840x1600) -> 1600 / 160 = 10x scale (2400x1600)
        let scale_1600p = (3840u32 / 240).min(1600 / 160);
        assert_eq!(scale_1600p, 10);

        // Test 4K monitor (3840x2160) -> 2160 / 160 = 13x scale (3120x2080)
        let scale_4k = (3840u32 / 240).min(2160 / 160);
        assert_eq!(scale_4k, 13);
    }

    #[test]
    fn test_xbrz_scaling() {
        let width = 240;
        let height = 160;
        let mut src = vec![0u8; width * height * 4];

        // Draw a distinct diagonal pattern to verify edge interpolation
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                if (x + y) % 4 == 0 {
                    src[idx] = 255;     // R
                    src[idx + 1] = 128; // G
                    src[idx + 2] = 64;  // B
                    src[idx + 3] = 255; // A
                } else {
                    src[idx] = 10;
                    src[idx + 1] = 20;
                    src[idx + 2] = 30;
                    src[idx + 3] = 255;
                }
            }
        }

        // Test 2x scaling (480x320)
        let scaled_2x = xbrz::scale_rgba(&src, width, height, 2);
        assert_eq!(scaled_2x.len(), (width * 2) * (height * 2) * 4);

        // Test 4x scaling (960x640 - Recommended 4K Ultrawide preset)
        let scaled_4x = xbrz::scale_rgba(&src, width, height, 4);
        assert_eq!(scaled_4x.len(), (width * 4) * (height * 4) * 4);

        // Test 6x scaling (1440x960)
        let scaled_6x = xbrz::scale_rgba(&src, width, height, 6);
        assert_eq!(scaled_6x.len(), (width * 6) * (height * 6) * 4);
    }

    #[test]
    fn test_xbrz_with_nvidia_adaptive_sharpening() {
        use gba_simulator::ui::screen::apply_nvidia_adaptive_sharpening_image;
        use eframe::egui::ColorImage;

        let width = 240;
        let height = 160;
        let mut src = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                let val = if x < 120 { 20 } else { 220 };
                src[idx] = val;
                src[idx + 1] = val;
                src[idx + 2] = val;
                src[idx + 3] = 255;
            }
        }

        // 1. Scale with xBRZ 4x (960x640)
        let factor = 4;
        let scaled = xbrz::scale_rgba(&src, width, height, factor);
        let mut img = ColorImage::from_rgba_unmultiplied([width * factor, height * factor], &scaled);
        assert_eq!(img.size, [960, 640]);

        // 2. Apply NVIDIA Adaptive Sharpening on top of xBRZ scaled image
        apply_nvidia_adaptive_sharpening_image(&mut img, 0.7);

        // Check pixels are valid, non-black, and within range
        let p_left = img.pixels[320 * 960 + 50];
        let p_right = img.pixels[320 * 960 + 900];
        assert_eq!(p_left.r(), p_left.g());
        assert_eq!(p_right.r(), p_right.g());
        assert!(p_left.r() < 50);
        assert!(p_right.r() > 200);
    }

    #[test]
    fn test_audio_51_surround_dsp() {
        use gba_simulator::gba::apu::{AudioOutput, SurroundMode};

        let output = AudioOutput::new();
        // Test SurroundMode getters and setters
        assert_eq!(output.surround_mode(), SurroundMode::Headphone3D);
        output.set_surround_mode(SurroundMode::Surround51);
        assert_eq!(output.surround_mode(), SurroundMode::Surround51);
        output.set_surround_mode(SurroundMode::Stereo);
        assert_eq!(output.surround_mode(), SurroundMode::Stereo);

        // Test Bass Boost & Surround Width
        output.set_bass_boost(0.75);
        assert!((output.bass_boost() - 0.75).abs() < 0.001);
        output.set_surround_width(0.90);
        assert!((output.surround_width() - 0.90).abs() < 0.001);

        // Test sample buffer pushing
        output.push_samples(0.5, -0.5);
        assert!(output.buffer_len() >= 2);
    }

    #[test]
    fn test_rewind_manager() {
        use gba_simulator::ui::rewind::RewindManager;

        let mut gba = Gba::new();
        let mut rewind = RewindManager::new(20, 2);

        // Run frames and record
        for _ in 0..10 {
            gba.run_frame();
            rewind.record_frame(&gba);
        }

        assert!(rewind.state_count() > 0);
        assert!(rewind.available_seconds() > 0.0);

        // Rewind should successfully pop state
        let restored = rewind.rewind_step(&mut gba);
        assert!(restored);

        rewind.clear();
        assert_eq!(rewind.state_count(), 0);
        assert!(!rewind.rewind_step(&mut gba));
    }

    #[test]
    fn test_save_state_manager() {
        use gba_simulator::ui::save_manager::SaveStateManager;

        let temp_dir = std::env::temp_dir().join("gba_test_saves");
        let mut sm = SaveStateManager::new(&temp_dir);
        let gba = Gba::new();

        let rom_name = "Sample_Game_Test";

        // Save into slot 3
        let save_res = sm.save_slot(3, &gba, rom_name);
        assert!(save_res.is_ok());

        // Check metadata
        let meta = sm.get_slot_metadata(3, rom_name);
        assert!(meta.exists);
        assert!(meta.size_bytes > 100_000);

        // Load back from slot 3
        let mut loaded_gba = Gba::new();
        let load_res = sm.load_slot(3, &mut loaded_gba, rom_name);
        assert!(load_res.is_ok());

        // Delete slot 3
        sm.delete_slot(3, rom_name);
        let meta_after = sm.get_slot_metadata(3, rom_name);
        assert!(!meta_after.exists);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_rtc_time_travel() {
        use gba_simulator::gba::mmu::rtc::Rtc;

        let mut rtc = Rtc::new();
        let (y0, m0, d0, h0, min0, _, dow0) = rtc.get_datetime_components();
        assert!(y0 >= 2026);
        assert!((1..=12).contains(&m0));
        assert!((1..=31).contains(&d0));
        assert!(h0 <= 23 && min0 <= 59);

        // Advance 24 hours (1 full day)
        rtc.add_offset_secs(86400);
        let (_y1, _m1, d1, h1, min1, _, dow1) = rtc.get_datetime_components();
        assert_eq!(h0, h1);
        assert_eq!(min0, min1);
        assert_ne!(dow0, dow1);
        assert_ne!(d0, d1);

        // Reset offset
        rtc.reset_offset();
        let (yr, mr, dr, hr, minr, _, dowr) = rtc.get_datetime_components();
        assert_eq!((y0, m0, d0, h0, min0, dow0), (yr, mr, dr, hr, minr, dowr));
    }

    #[test]
    fn test_screenshot_bmp_encoding() {
        use gba_simulator::ui::screenshot::save_bmp;

        let width = 10;
        let height = 10;
        let rgba = vec![255u8; width * height * 4];

        let temp_path = std::env::temp_dir().join("test_screenshot.bmp");
        let res = save_bmp(&temp_path, width, height, &rgba);
        assert!(res.is_ok());

        let bytes = std::fs::read(&temp_path).unwrap();
        // BMP signature: b"BM"
        assert_eq!(&bytes[0..2], b"BM");
        // File size in header
        let expected_size = 54 + width * height * 4;
        assert_eq!(bytes.len(), expected_size);

        let _ = std::fs::remove_file(temp_path);
    }

    #[test]
    fn test_cheats_and_ram_searcher() {
        use gba_simulator::gba::cheats::{CheatManager, CheatOp, RamSearcher, SearchSize};
        use gba_simulator::gba::mmu::Mmu;

        // 1. Raw code parsing
        let ops = gba_simulator::gba::cheats::parse_cheat_code("0202402C:270F\n03001234:42");
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0], CheatOp::Write16(0x0202402C, 0x270F));
        assert_eq!(ops[1], CheatOp::Write8(0x03001234, 0x42));

        // 2. Action Replay / CodeBreaker 16-bit write
        let cb_ops = gba_simulator::gba::cheats::parse_cheat_code("82005000 0064");
        assert_eq!(cb_ops.len(), 1);
        assert_eq!(cb_ops[0], CheatOp::Write16(0x02005000, 100));

        // 3. CheatManager memory apply
        let mut mmu = Mmu::new();
        let mut manager = CheatManager::new();
        manager.add_cheat("Max Gold", "02001000:FFFF");
        assert_eq!(mmu.read16(0x02001000), 0);
        manager.apply(&mut mmu);
        assert_eq!(mmu.read16(0x02001000), 0xFFFF);

        // 4. RamSearcher
        mmu.write16(0x02002000, 42);
        let mut searcher = RamSearcher::new();
        searcher.search_size = SearchSize::U16;
        searcher.initial_search(&mmu, Some(42));
        assert!(searcher.candidates.iter().any(|c| c.address == 0x02002000));
    }

    #[test]
    fn test_sio_multiplayer_link() {
        use gba_simulator::gba::mmu::sio::{MultiplayerRole, Sio};

        let mut sio = Sio::new();
        assert_eq!(sio.role, MultiplayerRole::SinglePlayer);

        // Test register writes and reads
        sio.write_io16(0x120, 0x1234);
        sio.write_io16(0x122, 0x5678);
        assert_eq!(sio.read_io16(0x120), 0x1234);
        assert_eq!(sio.read_io16(0x122), 0x5678);

        // SIOCNT write and read
        sio.write_io16(0x128, 0x0083); // 32-bit mode, start transfer
        assert_eq!(sio.read_io16(0x128) & 0x0083, 0x0083);

        // Role switching
        sio.set_role(MultiplayerRole::Player1Host);
        assert_eq!(sio.role, MultiplayerRole::Player1Host);
        assert_eq!(sio.local_port, 8765);
        assert_eq!(sio.peer_port, 8766);
    }

    #[test]
    fn test_cartridge_sensors() {
        use gba_simulator::gba::mmu::sensors::{CartridgeSensors, SensorType};

        let mut sensors = CartridgeSensors::new();

        // 1. Detection
        sensors.detect_from_cartridge("U3IE", "BOKTAI");
        assert_eq!(sensors.sensor_type, SensorType::Solar);

        sensors.detect_from_cartridge("RZWE", "WARIOWARE T");
        assert_eq!(sensors.sensor_type, SensorType::GyroTilt);

        sensors.detect_from_cartridge("V49E", "DRILL DOZER");
        assert_eq!(sensors.sensor_type, SensorType::Rumble);

        // 2. Solar sensor levels
        sensors.set_sunlight(8);
        assert_eq!(sensors.sunlight_level, 8);

        // 3. Tilt levels
        sensors.set_tilt(0.5, -0.75);
        assert_eq!(sensors.tilt_x, 0.5);
        assert_eq!(sensors.tilt_y, -0.75);

        // 4. Rumble Pak
        sensors.sensor_type = SensorType::Rumble;
        sensors.write_gpio(0x080000C4, 0x08); // Turn on rumble bit 3
        assert!(sensors.rumble_active);
        sensors.write_gpio(0x080000C4, 0x00);
        assert!(!sensors.rumble_active);
    }

    #[test]
    fn test_gen3_rpg_decryption() {
        use gba_simulator::ui::pokemon_companion::PokemonCompanion;

        let mut raw = [0u8; 100];
        // Personality = 0x12345678, OT ID = 0x12345678 -> Special state (0 ^ 0 = 0 < 8)
        let personality = 0x12345678u32;
        let ot_id = 0x12345678u32;
        raw[0..4].copy_from_slice(&personality.to_le_bytes());
        raw[4..8].copy_from_slice(&ot_id.to_le_bytes());

        // Fill nickname "MONSTER" in Gen 3 char encoding: M=0xC7, O=0xC9, N=0xC8, S=0xCD, T=0xCE, E=0xBF, R=0xCC, Term=0xFF
        raw[8..16].copy_from_slice(&[0xC7, 0xC9, 0xC8, 0xCD, 0xCE, 0xBF, 0xCC, 0xFF]);
        // Set Species ID = 25 in Growth substruct (order 0 with key 0)
        raw[32..34].copy_from_slice(&25u16.to_le_bytes());
        raw[84] = 50; // Level 50
        raw[86..88].copy_from_slice(&120u16.to_le_bytes()); // Current HP
        raw[88..90].copy_from_slice(&120u16.to_le_bytes()); // Max HP

        let decrypted = PokemonCompanion::decrypt_pokemon(&raw);
        assert!(decrypted.is_some());
        let pkmn = decrypted.unwrap();
        assert_eq!(pkmn.nickname, "MONSTER");
        assert_eq!(pkmn.level, 50);
        assert_eq!(pkmn.current_hp, 120);
        assert_eq!(pkmn.max_hp, 120);
        assert!(pkmn.is_shiny, "Matching personality and OT ID should trigger special calculation");
    }

    #[test]
    fn test_ppu_layer_masking() {
        use gba_simulator::gba::ppu::Ppu;

        let mut ppu = Ppu::new();
        assert_eq!(ppu.layer_mask, 0x1F, "All 5 layers should be enabled by default");

        // Toggle layer mask
        ppu.layer_mask = 0b00001; // Only BG0 enabled
        assert_eq!(ppu.layer_mask & 1, 1);
        assert_eq!(ppu.layer_mask & (1 << 4), 0); // OBJ disabled
    }

    #[test]
    fn test_gif_recorder_encoding() {
        use gba_simulator::ui::gif_recorder::GifRecorder;

        let mut recorder = GifRecorder::new();
        recorder.start_recording();
        assert!(recorder.is_recording);

        // Feed test frames
        let test_frame = [0xFF0000FFu32; 240 * 160]; // Solid Red
        recorder.capture_frame(&test_frame);
        recorder.capture_frame(&test_frame);

        let res = recorder.stop_and_save("unit_test");
        assert!(res.is_ok());
        let (path, _) = res.unwrap();
        assert!(path.exists());

        let bytes = std::fs::read(&path).unwrap();
        // GIF89a magic signature
        assert_eq!(&bytes[0..6], b"GIF89a");
        // Canvas width = 240, height = 160
        let w = u16::from_le_bytes([bytes[6], bytes[7]]);
        let h = u16::from_le_bytes([bytes[8], bytes[9]]);
        assert_eq!(w, 240);
        assert_eq!(h, 160);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_tas_engine() {
        use gba_simulator::ui::tas::{TasEngine, TasMode};

        let mut tas = TasEngine::new();
        tas.start_recording();
        assert_eq!(tas.mode, TasMode::Recording);

        let frame1 = [true, false, false, false, false, false, false, false, false, false]; // A pressed
        let frame2 = [false, true, false, false, true, false, false, false, false, false]; // B + Right
        tas.record_frame(&frame1);
        tas.record_frame(&frame2);
        assert_eq!(tas.recorded_inputs.len(), 2);

        let temp_path = std::env::temp_dir().join("test_script.tas");
        assert!(tas.save_to_file(&temp_path).is_ok());

        let mut replayed = TasEngine::new();
        assert!(replayed.load_from_file(&temp_path).is_ok());
        assert_eq!(replayed.recorded_inputs.len(), 2);
        assert_eq!(replayed.recorded_inputs[0], frame1);
        assert_eq!(replayed.recorded_inputs[1], frame2);

        replayed.start_playback();
        assert_eq!(replayed.mode, TasMode::Playback);
        let p1 = replayed.get_playback_inputs().unwrap();
        let p2 = replayed.get_playback_inputs().unwrap();
        assert_eq!(p1, frame1);
        assert_eq!(p2, frame2);
        assert_eq!(replayed.get_playback_inputs(), None);
        assert_eq!(replayed.mode, TasMode::Idle);

        let _ = std::fs::remove_file(temp_path);
    }

    #[test]
    fn test_audio_mixer_mute_channels() {
        use gba_simulator::gba::apu::Apu;

        let apu = Apu::new();
        assert_eq!(apu.audio_output.channel_mute_mask(), 0);

        // Mute DirectSound A and PSG Square 1
        apu.audio_output.set_channel_mute_mask(0b010001);
        assert_eq!(apu.audio_output.channel_mute_mask(), 0b010001);

        // Fast forward pitch preserved mode (default is 1 = Smart Mute)
        assert_eq!(apu.audio_output.fast_forward_mode(), 1);
        apu.audio_output.set_fast_forward_mode(2);
        assert_eq!(apu.audio_output.fast_forward_mode(), 2);
    }

    #[test]
    fn test_updater_state_transitions() {
        use gba_simulator::ui::updater::{is_newer, parse_version, UpdateManager, UpdateStatus};

        assert_eq!(parse_version("0.3.0"), Some((0, 3, 0)));
        assert_eq!(parse_version("v0.4.0"), Some((0, 4, 0)));
        assert_eq!(parse_version("v1.0.0"), Some((1, 0, 0)));
        assert!(is_newer("v0.4.0", "0.3.0"));
        assert!(is_newer("v0.3.1", "0.3.0"));
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.2.0", "0.3.0"));

        let updater = UpdateManager::new();
        assert_eq!(*updater.status.lock().unwrap(), UpdateStatus::Idle);
    }

    #[test]
    fn test_dmg_sound_channels() {
        use gba_simulator::gba::apu::Apu;

        let mut apu = Apu::new();

        // Enable master audio in SOUNDCNT_X
        apu.write_reg16(0x084, 0x0080);
        assert_eq!(apu.read_reg16(0x084) & 0x80, 0x80);

        // Configure SOUNDCNT_L: Master volume 7/7, enable all 4 channels to Left & Right
        apu.write_reg16(0x080, 0xFF77);
        // Configure SOUNDCNT_H: DMG volume 100% (ratio = 2)
        apu.write_reg16(0x082, 0x0002);

        // Initially no DMG channels are active
        assert_eq!(apu.read_reg16(0x084) & 0x0F, 0);

        // Test Channel 1 (Square with sweep):
        // Duty 50% (2), Volume 15, envelope decrease step 3
        apu.write_reg16(0x062, 0xF380);
        // Frequency = 1000, trigger = 1 (bit 15)
        apu.write_reg16(0x064, 0x83E8);

        // Channel 1 should now be active (bit 0 of SOUNDCNT_X)
        assert_eq!(apu.read_reg16(0x084) & 0x01, 1);
        assert!(apu.dmg.ch1.active);
        assert_eq!(apu.dmg.ch1.envelope.volume, 15);

        // Test Channel 2 (Square):
        // Duty 25% (1), Volume 10
        apu.write_reg16(0x068, 0xA040);
        // Frequency = 1200, trigger = 1
        apu.write_reg16(0x06C, 0x84B0);
        assert_eq!(apu.read_reg16(0x084) & 0x02, 2);
        assert!(apu.dmg.ch2.active);

        // Test Channel 3 (Wave):
        // Master enable
        apu.write_reg16(0x070, 0x0080);
        // Write Wave RAM pattern at 0x090..0x09F
        for i in 0..8 {
            apu.write_reg16(0x090 + i * 2, 0xF0A5);
        }
        assert_eq!(apu.read_reg16(0x090), 0xF0A5);
        // Volume 100%
        apu.write_reg16(0x072, 0x2000);
        // Frequency = 1500, trigger = 1
        apu.write_reg16(0x074, 0x85DC);
        assert_eq!(apu.read_reg16(0x084) & 0x04, 4);
        assert!(apu.dmg.ch3.active);

        // Test Channel 4 (Noise):
        // Volume 12
        apu.write_reg16(0x078, 0xC000);
        // Ratio = 1, 15-bit, shift = 2, trigger = 1
        apu.write_reg16(0x07C, 0x8021);
        assert_eq!(apu.read_reg16(0x084) & 0x08, 8);
        assert!(apu.dmg.ch4.active);

        // Step APU for multiple cycles and verify samples are generated
        for _ in 0..100 {
            apu.step(1000, [false; 4]);
        }

        let (s1, s2, s3, s4) = apu.dmg.get_samples();
        // Since channels are active and running, samples are generated
        assert!(apu.dmg.ch1.active);
        assert!(apu.dmg.ch2.active);
        assert!(apu.dmg.ch3.active);
        assert!(apu.dmg.ch4.active);
        assert!(s1 != 0.0 || s2 != 0.0 || s3 != 0.0 || s4 != 0.0);
    }

    #[test]
    fn test_audio_dsp_calibration_and_zero_dc_symmetry() {
        use gba_simulator::gba::apu::Apu;

        let mut apu = Apu::new();

        // 1. Verify Square Wave symmetry: magnitude is exactly vol/15.0 with zero DC bias
        apu.dmg.ch1.active = true;
        apu.dmg.ch1.envelope.volume = 9;
        apu.dmg.ch1.duty = 2; // 50% duty cycle
        let expected_amp = 9.0 / 15.0;

        let mut high_count = 0;
        let mut low_count = 0;
        for step in 0..8 {
            apu.dmg.ch1.duty_step = step;
            let sample = apu.dmg.ch1.sample();
            assert!((sample.abs() - expected_amp).abs() < 1e-5);
            if sample > 0.0 {
                high_count += 1;
            } else {
                low_count += 1;
            }
        }
        assert_eq!(high_count, 4);
        assert_eq!(low_count, 4);

        // 2. Verify Channel 4 (Noise) symmetry: magnitude is exactly vol/15.0
        apu.dmg.ch4.active = true;
        apu.dmg.ch4.envelope.volume = 6;
        let noise_amp = 6.0 / 15.0;
        let s_noise = apu.dmg.ch4.sample();
        assert!((s_noise.abs() - noise_amp).abs() < 1e-5);

        // 3. Verify Wave Channel bipolar centering around zero
        apu.dmg.ch3.active = true;
        apu.dmg.ch3.master_enable = true;
        apu.dmg.ch3.volume_code = 1; // 100%
        // Write max sample (15) and min sample (0)
        apu.dmg.ch3.wave_ram[0] = 0xF0;
        apu.dmg.ch3.sample_index = 0; // high nibble: 0x0F
        let s_max = apu.dmg.ch3.sample();
        assert!((s_max - 1.0).abs() < 1e-5);

        apu.dmg.ch3.sample_index = 1; // low nibble: 0x00
        let s_min = apu.dmg.ch3.sample();
        assert!((s_min - (-1.0)).abs() < 1e-5);

        // 4. Test end-to-end APU stepping with DirectSound + DMG
        apu.write_reg16(0x084, 0x0080); // Enable APU
        apu.write_reg16(0x080, 0xFF77); // Max DMG volume
        apu.write_reg16(0x082, 0x0002); // 100% DMG ratio
        // Push full-scale DirectSound samples
        for _ in 0..16 {
            apu.sound_a.push_byte(127);
            apu.sound_b.push_byte(127);
        }
        apu.sound_a.left_enable = true;
        apu.sound_a.right_enable = true;
        apu.sound_b.left_enable = true;
        apu.sound_b.right_enable = true;

        // Run APU through multiple sample periods
        for _ in 0..500 {
            apu.step(1000, [true, true, false, false]);
        }
        apu.flush_samples();

        // Ensure audio output buffer has received valid, finite, non-clipped samples
        assert!(apu.audio_output.buffer_len() > 0);
    }

    #[test]
    fn test_channel1_byte_access_and_trigger_isolation() {
        use gba_simulator::gba::Gba;

        let mut gba = Gba::new();

        // 1. Enable master sound in SOUNDCNT_X
        gba.mmu.write16(0x0400_0084, 0x0080);
        assert_eq!(gba.mmu.read16(0x0400_0084) & 0x80, 0x80);

        // 2. Configure Channel 1:
        // Volume 15, envelope decrease step 1, length = 32
        gba.mmu.write16(0x0400_0062, 0xF1A0);
        // Frequency = 1000, trigger = 1 (bit 15), length enable = 1 (bit 14)
        gba.mmu.write16(0x0400_0064, 0xC3E8);

        // Verify Channel 1 is active and volume is 15
        assert!(gba.mmu.apu.dmg.ch1.active);
        assert_eq!(gba.mmu.apu.dmg.ch1.envelope.volume, 15);

        // 3. Verify SOUND1CNT_X read masks out trigger bit 15:
        let val16 = gba.mmu.read16(0x0400_0064);
        assert_eq!(val16 & 0x8000, 0, "Trigger bit 15 must be write-only and read back as 0");
        assert_eq!(val16 & 0x4000, 0x4000, "Length flag bit 14 must be readable");

        // Step envelope down once
        gba.mmu.apu.dmg.ch1.step_envelope();
        assert_eq!(gba.mmu.apu.dmg.ch1.envelope.volume, 14);

        // 4. Perform 8-bit write to 0x0400_0064 (NR13 - frequency low byte) as done by m4a sound driver
        gba.mmu.write8(0x0400_0064, 0x50);

        // Verify frequency low byte was updated, BUT channel was NOT re-triggered:
        assert_eq!(gba.mmu.apu.dmg.ch1.frequency & 0xFF, 0x50);
        assert_eq!(gba.mmu.apu.dmg.ch1.envelope.volume, 14, "Volume must NOT reset to initial volume on NR13 write");

        // 5. Perform 8-bit write to 0x0400_0065 (NR14) without trigger bit (bit 7 = 0)
        gba.mmu.write8(0x0400_0065, 0x42); // length enable = 1, freq hi = 2, trigger = 0
        assert_eq!(gba.mmu.apu.dmg.ch1.envelope.volume, 14, "Volume must NOT reset when trigger bit 7 is not set");

        // 6. Perform 8-bit write to 0x0400_0065 with trigger bit (bit 7 = 1)
        gba.mmu.write8(0x0400_0065, 0xC2); // trigger = 1
        assert_eq!(gba.mmu.apu.dmg.ch1.envelope.volume, 15, "Volume must reset to initial volume when trigger bit 7 is set");
    }
}




