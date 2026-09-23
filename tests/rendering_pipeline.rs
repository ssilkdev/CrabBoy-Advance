//! Tests for GBA Rendering Pipeline (ROADMAP M5).
//!
//! Validates:
//! 1. PPU layer capture, isolated buffers, and draw commands without altering composited output.
//! 2. Frame blending algorithms (Simple50, SmartDeFlicker 30Hz, LcdGhosting).
//! 3. Shader pipeline presets (Crisp, LcdGrid, LcdSubpixel, CrtScanlines, CrtGeom).
//! 4. Custom shader parameter parsing (JSON and key=value) and execution.

use gba_simulator::gba::{
    frame_blend::{blend_50_50, FrameBlendMode, FrameBlender, FRAME_PIXELS},
    ppu::{
        layers::{LayerKind, PpuLayer},
        SCREEN_HEIGHT, SCREEN_WIDTH,
    },
    shader::{apply_shader, CustomShaderParams, ShaderPreset},
    Gba,
};

#[test]
fn ppu_layer_capture_isolates_layers_without_altering_framebuffer() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    // Infinite loop at 0x0800_0000: B . (0xEAFFFFFE)
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);
    // Enable BG0 in Mode 0 (clear forced blank bit 7)
    gba.mmu.write16(0x0400_0000, 0x0100);

    // Run 1 frame without layer capture
    gba.run_frame();
    let baseline_fb = *gba.get_framebuffer();
    assert!(gba.get_layer_framebuffers().is_none());
    assert!(gba.get_draw_commands().is_empty());

    // Enable layer capture and run 1 frame
    gba.set_layer_capture(true);
    gba.run_frame();
    let captured_fb = *gba.get_framebuffer();

    // Verify composited framebuffer is bit-identical
    assert_eq!(
        baseline_fb, captured_fb,
        "Native composited framebuffer must remain bit-identical with layer capture active"
    );

    // Verify layer buffers exist and are populated
    let layer_buffers = gba.get_layer_framebuffers().expect("layer buffers should exist");
    let backdrop = layer_buffers.get_layer(PpuLayer::Backdrop);
    // Backdrop pixels should be fully opaque (alpha == 0xFF)
    assert_eq!(
        backdrop[0] & 0xFF00_0000,
        0xFF00_0000,
        "Backdrop should be fully opaque"
    );

    // Verify get_layer_framebuffer helper matches
    let backdrop_direct = gba
        .get_layer_framebuffer(PpuLayer::Backdrop)
        .expect("backdrop layer framebuffer");
    assert_eq!(backdrop[0], backdrop_direct[0]);

    // Check BG and OBJ layers are available
    for layer in [
        PpuLayer::Bg0,
        PpuLayer::Bg1,
        PpuLayer::Bg2,
        PpuLayer::Bg3,
        PpuLayer::Obj,
    ] {
        assert!(gba.get_layer_framebuffer(layer).is_some());
    }

    // Verify draw commands were recorded
    let commands = gba.get_draw_commands();
    assert!(
        !commands.is_empty(),
        "Draw commands should be emitted during active frame"
    );
    // Check command structure integrity
    for cmd in commands {
        assert!(cmd.scanline < SCREEN_HEIGHT as u16);
        assert!(matches!(
            cmd.kind,
            LayerKind::Backdrop | LayerKind::Text | LayerKind::Affine | LayerKind::Bitmap | LayerKind::Obj
        ));
    }

    // Disable layer capture
    gba.set_layer_capture(false);
    assert!(gba.get_layer_framebuffers().is_none());
    assert!(gba.get_draw_commands().is_empty());
}

#[test]
fn frame_blend_simple_50_50() {
    let mut blender = FrameBlender::new();

    let frame1 = [0xFF00_00FF; FRAME_PIXELS]; // Solid Red
    let frame2 = [0xFFFF_0000; FRAME_PIXELS]; // Solid Blue

    // First frame has no history, returns verbatim
    let out1 = blender.blend(&frame1, FrameBlendMode::Simple50);
    assert_eq!(out1[0], frame1[0]);

    // Second frame blends 50/50 with frame 1
    let out2 = blender.blend(&frame2, FrameBlendMode::Simple50);
    let expected = blend_50_50(frame2[0], frame1[0]);
    assert_eq!(out2[0], expected);
    assert_eq!(out2[0] & 0xFF, 0x7F, "Red channel 50%");
    assert_eq!((out2[0] >> 16) & 0xFF, 0x7F, "Blue channel 50%");
    assert_eq!((out2[0] >> 24) & 0xFF, 0xFF, "Alpha channel remains 0xFF");
}

#[test]
fn frame_blend_smart_deflicker_distinguishes_motion_and_flicker() {
    let mut blender = FrameBlender::new();

    let red = [0xFF00_00FF; FRAME_PIXELS];
    let blue = [0xFFFF_0000; FRAME_PIXELS];
    let green = [0xFF00_FF00; FRAME_PIXELS];

    // Scenario 1: Static scene (Red -> Red)
    // Should NOT blend or blur
    let _ = blender.blend(&red, FrameBlendMode::SmartDeFlicker);
    let out_static = blender.blend(&red, FrameBlendMode::SmartDeFlicker);
    assert_eq!(out_static[0], red[0], "Static scene stays pristine");

    // Scenario 2: Normal motion (Red -> Blue -> Green)
    // Non-oscillating motion should stay sharp (not blended)
    blender.reset();
    let _ = blender.blend(&red, FrameBlendMode::SmartDeFlicker);
    let _ = blender.blend(&blue, FrameBlendMode::SmartDeFlicker);
    let out_motion = blender.blend(&green, FrameBlendMode::SmartDeFlicker);
    assert_eq!(
        out_motion[0], green[0],
        "Normal motion should remain unblended"
    );

    // Scenario 3: 30 Hz flicker (Red -> Blue -> Red)
    // Frame t == Frame t-2, but != Frame t-1.
    // Must trigger flicker elimination!
    blender.reset();
    let _ = blender.blend(&red, FrameBlendMode::SmartDeFlicker);
    let _ = blender.blend(&blue, FrameBlendMode::SmartDeFlicker);
    let out_flicker = blender.blend(&red, FrameBlendMode::SmartDeFlicker);
    let expected = blend_50_50(red[0], blue[0]);
    assert_eq!(
        out_flicker[0], expected,
        "30 Hz flicker must blend to smooth 50% transparency"
    );
}

#[test]
fn frame_blend_lcd_ghosting_decay() {
    let mut blender = FrameBlender::new();

    let white = [0xFFFF_FFFF; FRAME_PIXELS];
    let black = [0xFF00_0000; FRAME_PIXELS];

    // Frame 1: White
    let _ = blender.blend(&white, FrameBlendMode::LcdGhosting { decay: 0.50 });

    // Frame 2: Switch to Black, with decay 0.50 (50% current black + 50% prev white)
    let out1 = blender.blend(&black, FrameBlendMode::LcdGhosting { decay: 0.50 });
    let r1 = out1[0] & 0xFF;
    assert!(
        (120..=136).contains(&r1),
        "First ghosted frame should be ~128, got {r1}"
    );

    // Frame 3: Black again (50% current black + 50% prev ~128 => ~64)
    let out2 = blender.blend(&black, FrameBlendMode::LcdGhosting { decay: 0.50 });
    let r2 = out2[0] & 0xFF;
    assert!(
        (60..=70).contains(&r2),
        "Second ghosted frame should decay to ~64, got {r2}"
    );
}

#[test]
fn shader_presets_execution_and_geometry() {
    let mut src = [0xFF80_8080; FRAME_PIXELS]; // Flat 50% gray
    // Put a bright pixel at (0, 0) and (1, 1)
    src[0] = 0xFFFF_FFFF;
    src[SCREEN_WIDTH + 1] = 0xFFFF_FFFF;

    let mut dst = [0u32; FRAME_PIXELS];

    // 1. Crisp preset: exact copy
    apply_shader(&src, &mut dst, ShaderPreset::Crisp, None);
    assert_eq!(dst[0], src[0]);
    assert_eq!(dst[1], src[1]);

    // 2. LCD Grid preset: grid lines on odd rows or odd columns are darkened
    apply_shader(&src, &mut dst, ShaderPreset::LcdGrid, None);
    let even_even = dst[0]; // (0, 0): unmodified
    let odd_even = dst[1];  // (1, 0): grid column, darkened
    let even_odd = dst[SCREEN_WIDTH]; // (0, 1): grid row, darkened

    assert_eq!(even_even & 0xFF, 0xFF);
    assert!((odd_even & 0xFF) < (src[1] & 0xFF));
    assert!((even_odd & 0xFF) < (src[SCREEN_WIDTH] & 0xFF));
    assert_eq!(dst[0] & 0xFF00_0000, 0xFF00_0000, "Alpha stays 0xFF");

    // 3. LCD Subpixel preset: micro-bias by x % 3
    apply_shader(&src, &mut dst, ShaderPreset::LcdSubpixel, None);
    // Subpixel 0 has red bias, subpixel 1 has green bias, subpixel 2 has blue bias
    let pix_r = dst[0];
    let pix_g = dst[1];
    let pix_b = dst[2];
    assert!((pix_r & 0xFF) >= ((pix_r >> 8) & 0xFF), "Subpixel 0 should be red-biased");
    assert!(((pix_g >> 8) & 0xFF) >= (pix_g & 0xFF), "Subpixel 1 should be green-biased");
    assert!(((pix_b >> 16) & 0xFF) >= (pix_b & 0xFF), "Subpixel 2 should be blue-biased");

    // 4. CRT Scanlines preset: odd scanlines darkened
    apply_shader(&src, &mut dst, ShaderPreset::CrtScanlines, None);
    let scanline_row0 = dst[SCREEN_WIDTH / 2];
    let scanline_row1 = dst[SCREEN_WIDTH + SCREEN_WIDTH / 2];
    assert!(
        (scanline_row1 & 0xFF) < (scanline_row0 & 0xFF),
        "Odd scanlines must be dimmed"
    );

    // 5. CRT Geom preset: scanlines + aperture grille
    apply_shader(&src, &mut dst, ShaderPreset::CrtGeom, None);
    assert_eq!(dst[0] & 0xFF00_0000, 0xFF00_0000);
}

#[test]
fn custom_shader_configuration_json_and_key_value() {
    // 1. Test key=value format with comments
    let kv_config = r#"
        # Authentic AGS-101 Profile
        name = "AGS-101 Backlit Warm"
        scanline_intensity = 0.20
        grid_intensity = 0.40
        aperture_grille = 0.10
        brightness_boost = 1.30
        color_temperature = 0.25
        bloom = 0.05
    "#;

    let parsed_kv = CustomShaderParams::parse(kv_config).expect("parse key-value shader");
    assert_eq!(parsed_kv.name, "AGS-101 Backlit Warm");
    assert_eq!(parsed_kv.scanline_intensity, 0.20);
    assert_eq!(parsed_kv.grid_intensity, 0.40);
    assert_eq!(parsed_kv.aperture_grille, 0.10);
    assert_eq!(parsed_kv.brightness_boost, 1.30);
    assert_eq!(parsed_kv.color_temperature, 0.25);
    assert_eq!(parsed_kv.bloom, 0.05);

    // 2. Test JSON format
    let json_config = r#"{
        "name": "Cool Arcade CRT",
        "scanline_intensity": 0.60,
        "grid_intensity": 0.0,
        "aperture_grille": 0.35,
        "brightness_boost": 1.20,
        "color_temperature": -0.40,
        "bloom": 0.20
    }"#;

    let parsed_json = CustomShaderParams::parse(json_config).expect("parse JSON shader");
    assert_eq!(parsed_json.name, "Cool Arcade CRT");
    assert_eq!(parsed_json.scanline_intensity, 0.60);
    assert_eq!(parsed_json.color_temperature, -0.40);

    // 3. Test Custom Shader Execution with warm temperature
    let src = [0xFF80_8080; FRAME_PIXELS]; // Gray 128
    let mut dst = [0u32; FRAME_PIXELS];

    apply_shader(&src, &mut dst, ShaderPreset::Custom, Some(&parsed_kv));
    // Even coordinate (0, 0)
    let p = dst[0];
    let r = p & 0xFF;
    let b = (p >> 16) & 0xFF;
    assert!(
        r > b,
        "Positive color temperature (+0.25) should result in warmer tint (R > B): r={r}, b={b}"
    );

    // Test Cool temperature
    apply_shader(&src, &mut dst, ShaderPreset::Custom, Some(&parsed_json));
    let p_cool = dst[0];
    let r_cool = p_cool & 0xFF;
    let b_cool = (p_cool >> 16) & 0xFF;
    assert!(
        b_cool > r_cool,
        "Negative color temperature (-0.40) should result in cooler tint (B > R): r={r_cool}, b={b_cool}"
    );
}

#[test]
fn ppu_layer_capture_forced_blank() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Initial state on boot is forced blank (0x0080)
    gba.set_layer_capture(true);
    gba.run_frame();

    let lb = gba.get_layer_framebuffers().expect("layer buffers");
    let backdrop = lb.get_layer(PpuLayer::Backdrop);
    // Forced blank fills backdrop with white
    assert_eq!(backdrop[0], 0xFFFF_FFFF);
    // During forced blank, no BG or OBJ draw commands should be generated
    assert!(gba.get_draw_commands().is_empty());
}

#[test]
fn preset_enumerations_and_display_names() {
    for preset in ShaderPreset::ALL {
        assert!(!preset.display_name().is_empty());
    }
    for mode in FrameBlendMode::ALL {
        assert!(!mode.display_name().is_empty());
    }
}

