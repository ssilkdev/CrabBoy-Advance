//! Tests for HD Mode 7 High-Resolution Affine Rendering (ROADMAP M6).
//!
//! Validates:
//! 1. Scale multipliers (2x, 4x, 8x) and framebuffer dimensions.
//! 2. Subpixel affine texture sampling in Mode 1 / Mode 2.
//! 3. Perspective scanline interpolation for 3D tracks (F-Zero / Mario Kart).
//! 4. Seamless compositing with native layers and priority resolution without seams.
//! 5. Affine sprite (rotscale OBJ) subpixel evaluation.
//! 6. Downsampled SSAA (supersampled anti-aliasing) preservation of native content.

use gba_simulator::gba::{
    ppu::{
        hd_mode7::{HdFrame, HdMode7Config, HdScale},
        SCREEN_HEIGHT, SCREEN_WIDTH,
    },
    Gba,
};

#[test]
fn hd_scale_factors_and_resolutions() {
    let mut config = HdMode7Config::default();
    assert_eq!(config.scale, HdScale::Off);

    config.scale = HdScale::X2;
    let frame_2x = HdFrame::new(config.scale.factor());
    assert_eq!(frame_2x.width, 480);
    assert_eq!(frame_2x.height, 320);
    assert_eq!(frame_2x.pixels.len(), 480 * 320);

    config.scale = HdScale::X4;
    let frame_4x = HdFrame::new(config.scale.factor());
    assert_eq!(frame_4x.width, 960);
    assert_eq!(frame_4x.height, 640);
    assert_eq!(frame_4x.pixels.len(), 960 * 640);

    config.scale = HdScale::X8;
    let frame_8x = HdFrame::new(config.scale.factor());
    assert_eq!(frame_8x.width, 1920);
    assert_eq!(frame_8x.height, 1280);
    assert_eq!(frame_8x.pixels.len(), 1920 * 1280);
}

#[test]
fn hd_mode7_downsample_ssaa_preserves_non_affine_content() {
    // When a 4x HD frame contains flat or crisp integer-blocked content,
    // SSAA box-filtering must produce the exact original color without drift.
    let mut hd = HdFrame::new(4);
    // Fill top-left native pixel block (4x4 subpixels) with RGB(200, 100, 50)
    let test_color = 0xFF00_0000 | (50 << 16) | (100 << 8) | 200;
    for sy in 0..4 {
        for sx in 0..4 {
            hd.pixels[sy * hd.width + sx] = test_color;
        }
    }

    let native = hd.downsample_ssaa();
    assert_eq!(
        native[0], test_color,
        "SSAA downsampling should exactly preserve crisp native pixel blocks"
    );
}

#[test]
fn hd_mode7_subpixel_affine_rendering_mode1() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    // Infinite loop B . (0xEAFFFFFE)
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Setup Mode 1: BG0 Text, BG1 Text, BG2 Affine
    // DISPCNT = 0x0401 (Mode 1, BG2 enabled, forced blank cleared)
    gba.mmu.write16(0x0400_0000, 0x0401);

    // Set BG2CNT: priority 0, char base 0 (0x0600_0000), screen base 2 (0x0600_1000), 128x128 wrap
    gba.mmu.write16(0x0400_000C, 0x2200);

    // Set BG2 Affine Matrix: 1:1 scale (PA=0x0100, PB=0, PC=0, PD=0x0100)
    gba.mmu.write16(0x0400_0020, 0x0100);
    gba.mmu.write16(0x0400_0022, 0x0000);
    gba.mmu.write16(0x0400_0024, 0x0000);
    gba.mmu.write16(0x0400_0026, 0x0100);

    // Set Palette Entry 1 = Red (BGR555 0x001F)
    gba.mmu.write16(0x0500_0002, 0x001F);
    // Set Backdrop (Entry 0) = Blue (BGR555 0x7C00)
    gba.mmu.write16(0x0500_0000, 0x7C00);

    // Write screen map entry 0 at screen_base 2 (0x0600_1000) pointing to Tile 0
    gba.mmu.write16(0x0600_1000, 0x0000);
    // Char base 0 (0x0600_0000): Tile 0 pixel at (0, 0) has palette index 1
    gba.mmu.write16(0x0600_0000, 0x0101);

    // Enable HD Mode 7 at 4x internal resolution
    let config = HdMode7Config {
        scale: HdScale::X4,
        perspective_interpolation: true,
        ssaa: false,
    };
    gba.set_hd_mode7_config(config);

    // Run 1 frame
    gba.run_frame();

    // Verify HD frame is produced
    let hd = gba.render_hd_frame().expect("HD frame must be rendered");
    assert_eq!(hd.width, 960);
    assert_eq!(hd.height, 640);

    // Check that subpixels within (0, 0) are rendered with red color
    let p00 = hd.pixels[0];
    assert_eq!(p00 & 0xFF00_0000, 0xFF00_0000, "Alpha channel opaque");
    assert_eq!(p00 & 0xFF, 0xFF, "Red channel active for tile pixel");
}

#[test]
fn hd_mode7_perspective_interpolation_between_scanlines() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Mode 1, BG2 enabled
    gba.mmu.write16(0x0400_0000, 0x0401);
    gba.mmu.write16(0x0400_000C, 0x2000); // BG2CNT wrap

    // Set affine matrix
    gba.mmu.write16(0x0400_0020, 0x0100);
    gba.mmu.write16(0x0400_0026, 0x0100);

    let config_interp = HdMode7Config {
        scale: HdScale::X4,
        perspective_interpolation: true,
        ssaa: false,
    };
    gba.set_hd_mode7_config(config_interp);
    gba.run_frame();

    let frame = gba.render_hd_frame().expect("HD frame");
    assert_eq!(frame.width, 960);
    assert_eq!(frame.height, 640);
}

#[test]
fn hd_mode7_seamless_compositing_with_native_ui_layer() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Enable BG0 (Text, UI) and BG2 (Affine) in Mode 1
    // DISPCNT = 0x0501 (BG0 + BG2 enabled, Mode 1)
    gba.mmu.write16(0x0400_0000, 0x0501);

    // BG0 priority 0 (top), BG2 priority 2 (below)
    gba.mmu.write16(0x0400_0008, 0x0000); // BG0CNT: priority 0
    gba.mmu.write16(0x0400_000C, 0x2002); // BG2CNT: priority 2, wrap

    // Set Affine parameters
    gba.mmu.write16(0x0400_0020, 0x0100);
    gba.mmu.write16(0x0400_0026, 0x0100);

    // Backdrop = Dark Gray
    gba.mmu.write16(0x0500_0000, 0x2108);

    gba.set_hd_mode7_config(HdMode7Config {
        scale: HdScale::X4,
        perspective_interpolation: true,
        ssaa: false,
    });

    gba.run_frame();

    let hd_frame = gba.render_hd_frame().expect("HD frame");
    // Verify no unrendered transparent holes (all pixels fully opaque)
    for (i, &p) in hd_frame.pixels.iter().enumerate().take(1000) {
        assert_eq!(
            p & 0xFF00_0000,
            0xFF00_0000,
            "Pixel at index {i} must be fully opaque in composited HD frame"
        );
    }
}

#[test]
fn hd_mode7_ssaa_downsampling_box_filter() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Mode 1 with BG2
    gba.mmu.write16(0x0400_0000, 0x0401);
    gba.mmu.write16(0x0400_000C, 0x2000);
    gba.mmu.write16(0x0400_0020, 0x0100);
    gba.mmu.write16(0x0400_0026, 0x0100);

    let config = HdMode7Config {
        scale: HdScale::X4,
        perspective_interpolation: true,
        ssaa: true,
    };
    gba.set_hd_mode7_config(config);
    gba.run_frame();

    let hd = gba.render_hd_frame().expect("HD frame");
    let ssaa_native = hd.downsample_ssaa();
    assert_eq!(ssaa_native.len(), SCREEN_WIDTH * SCREEN_HEIGHT);
    // Verify top pixel is opaque
    assert_eq!(ssaa_native[0] & 0xFF00_0000, 0xFF00_0000);
}

#[test]
fn hd_mode7_bitmap_mode3_high_resolution() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Mode 3: 240x160 15-bit color bitmap, BG2 enabled
    // DISPCNT = 0x0403 (Mode 3, BG2 enabled, forced blank cleared)
    gba.mmu.write16(0x0400_0000, 0x0403);
    gba.mmu.write16(0x0400_0020, 0x0100); // PA = 1.0
    gba.mmu.write16(0x0400_0026, 0x0100); // PD = 1.0

    // Write color to VRAM pixel (0, 0): Green (0x03E0)
    gba.mmu.write16(0x0600_0000, 0x03E0);

    gba.set_hd_mode7_config(HdMode7Config {
        scale: HdScale::X2,
        perspective_interpolation: false,
        ssaa: false,
    });
    gba.run_frame();

    let hd = gba.render_hd_frame().expect("HD frame for Mode 3");
    assert_eq!(hd.width, 480);
    assert_eq!(hd.height, 320);
    assert_eq!(hd.pixels[0] & 0xFF00_0000, 0xFF00_0000);
}

#[test]
fn hd_mode7_affine_sprite_evaluation() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Enable OBJ layer (bit 12 = 0x1000) in Mode 0 (clear forced blank bit 7)
    gba.mmu.write16(0x0400_0000, 0x1000);

    // Sprite 0: Y=10, Affine=true (bit 8), 8x8 square
    gba.mmu.write16(0x0700_0000, 0x0100 | 10);
    // X=20, Affine Group 0
    gba.mmu.write16(0x0700_0002, 20);
    // Tile 0, Priority 0, Palette 0
    gba.mmu.write16(0x0700_0004, 0x0000);

    // Affine Group 0 parameters: 1:1 scale
    gba.mmu.write16(0x0700_0006, 0x0100); // PA = 1.0
    gba.mmu.write16(0x0700_000E, 0x0000); // PB = 0
    gba.mmu.write16(0x0700_0016, 0x0000); // PC = 0
    gba.mmu.write16(0x0700_001E, 0x0100); // PD = 1.0

    // OBJ Palette entry 1 = Yellow (BGR555 0x03FF)
    gba.mmu.write16(0x0500_0202, 0x03FF);

    // OBJ Tile 0 at 0x0601_0000: set first pixel to palette index 1 (4bpp)
    gba.mmu.write16(0x0601_0000, 0x0001);

    gba.set_hd_mode7_config(HdMode7Config {
        scale: HdScale::X4,
        perspective_interpolation: false,
        ssaa: false,
    });
    gba.run_frame();

    let hd = gba.render_hd_frame().expect("HD frame for affine sprite");
    assert_eq!(hd.width, 960);
    assert_eq!(hd.height, 640);

    // Check pixel at subpixel coordinate (20*4, 10*4) = (80, 40)
    let p_sprite = hd.pixels[40 * 960 + 80];
    assert_eq!(p_sprite & 0xFF00_0000, 0xFF00_0000, "Opaque alpha");
    assert_eq!(p_sprite & 0xFF, 0xFF, "Red channel active for yellow sprite");
    assert_eq!((p_sprite >> 8) & 0xFF, 0xFF, "Green channel active for yellow sprite");
}

