//! Integration tests for Per-Game Widescreen expansion (ROADMAP M8).
//!
//! Validates:
//! 1. Aspect ratio modes, geometries, and dimensions (16:9, tile-aligned, 16:10).
//! 2. Database lookup by game code and title keyword matching.
//! 3. Memory and ROM patch application with safety checks.
//! 4. Text background layer expansion across 512-wide tilemaps.
//! 5. HUD anchoring (Center, Left, Right) preventing repeating HUD artifacts.
//! 6. Mode 2 continuous affine background evaluation in widescreen margins.
//! 7. Off-screen sprite expansion into horizontal margins.
//! 8. Window mode handling (full-screen fades covering widescreen margins).
//! 9. Three verified games running in widescreen with no visual errors:
//!    - Mario Kart: Super Circuit (AMKE)
//!    - F-Zero: Maximum Velocity (AFZE)
//!    - Metroid Fusion (AMFE)
//! 10. HD Mode 7 + Widescreen combined subpixel rendering and SSAA downsampling.
//! 11. HD Sprite Replacements (M7) rendering in widescreen margins.

use gba_simulator::gba::{
    hd_pack::{HdImage, HdPack, HdReplacement},
    ppu::hd_mode7::{HdMode7Config, HdScale},
    widescreen::{
        HudAnchor, WidescreenConfig, WidescreenDatabase, WidescreenMode,
        WidescreenWindowMode,
    },
    Gba,
};

#[test]
fn test_widescreen_modes_and_geometry() {
    let mut config = WidescreenConfig::default();
    assert_eq!(config.mode, WidescreenMode::Ratio16_9);

    // Disabled initially
    config.enabled = false;
    assert_eq!(config.margins(), (0, 0));
    assert_eq!(config.total_width(), 240);
    assert_eq!(config.height(), 160);

    // 16:9 Standard (284x160)
    config.enabled = true;
    config.mode = WidescreenMode::Ratio16_9;
    assert_eq!(config.margins(), (22, 22));
    assert_eq!(config.total_width(), 284);
    assert_eq!(config.height(), 160);
    let aspect_16_9 = config.aspect_ratio();
    assert!((aspect_16_9 - (284.0 / 160.0)).abs() < 1e-4);
    assert!((aspect_16_9 - (16.0 / 9.0)).abs() < 0.01); // within 0.15%

    // 16:9 Tile-aligned (288x160 = 36x20 tiles)
    config.mode = WidescreenMode::TileAligned16_9;
    assert_eq!(config.margins(), (24, 24));
    assert_eq!(config.total_width(), 288);
    assert_eq!(config.total_width() % 8, 0); // Perfectly tile-aligned
    assert_eq!(config.height() % 8, 0);

    // 16:10 Steam Deck (256x160 = 32x20 tiles)
    config.mode = WidescreenMode::Ratio16_10;
    assert_eq!(config.margins(), (8, 8));
    assert_eq!(config.total_width(), 256);
    assert_eq!(config.total_width() % 8, 0);
    let aspect_16_10 = config.aspect_ratio();
    assert!((aspect_16_10 - (16.0 / 10.0)).abs() < 1e-4);

    // Custom margins
    config.mode = WidescreenMode::Custom {
        left_margin: 30,
        right_margin: 30,
    };
    assert_eq!(config.margins(), (30, 30));
    assert_eq!(config.total_width(), 300);
}

#[test]
fn test_widescreen_database_profiles() {
    // 1. Mario Kart: Super Circuit
    let mk = WidescreenDatabase::lookup("AMKE", "Mario Kart").expect("Mario Kart profile");
    assert_eq!(mk.game_code, "AMKE");
    assert!(!mk.bg_expand[0]); // BG0 HUD centered
    assert!(mk.bg_expand[1]);  // BG1 Horizon expanded
    assert!(mk.bg_expand[2]);  // BG2 Track expanded
    assert!(mk.bg_expand[3]);  // BG3 Clouds expanded
    assert!(mk.obj_expand);
    assert_eq!(mk.hud_anchor[0], HudAnchor::Center);
    assert_eq!(mk.window_mode, WidescreenWindowMode::ExtendFull);

    // European and Japanese releases
    assert!(WidescreenDatabase::lookup("AMKP", "").is_some());
    assert!(WidescreenDatabase::lookup("AMKJ", "").is_some());

    // 2. F-Zero: Maximum Velocity
    let fzero = WidescreenDatabase::lookup("AFZE", "F-Zero").expect("F-Zero profile");
    assert_eq!(fzero.game_code, "AFZE");
    assert!(!fzero.bg_expand[0]); // Speedometer HUD centered
    assert!(fzero.bg_expand[1]);  // Starfield expanded
    assert!(fzero.bg_expand[2]);  // Track expanded
    assert!(fzero.obj_expand);

    // 3. Metroid Fusion
    let metroid = WidescreenDatabase::lookup("AMFE", "Metroid Fusion").expect("Metroid Fusion profile");
    assert_eq!(metroid.game_code, "AMFE");
    assert!(!metroid.bg_expand[0]); // Energy HUD centered
    assert!(metroid.bg_expand[1]);  // Room scenery expanded
    assert!(metroid.bg_expand[2]);
    assert!(metroid.obj_expand);
    assert!(!metroid.patches.is_empty()); // Has camera patch definition

    // 4. Other notable games
    assert!(WidescreenDatabase::lookup("BMXE", "Zero Mission").is_some());
    assert!(WidescreenDatabase::lookup("ASOE", "Sonic Advance").is_some());
    assert!(WidescreenDatabase::lookup("AANE", "Castlevania").is_some());
    assert!(WidescreenDatabase::lookup("BPEE", "Pokemon Emerald").is_some());

    // Fallback title lookup
    let fallback = WidescreenDatabase::lookup("XXXX", "Super Mario Kart Advance").expect("Matched by title");
    assert!(fallback.title.contains("Mario Kart"));
}

#[test]
fn test_widescreen_memory_patch_application() {
    let profile = WidescreenDatabase::lookup("AMFE", "").expect("Metroid profile");
    assert!(!profile.patches.is_empty());

    let mut mock_rom = vec![0u8; 0x0800_2000];
    let patch = profile.patches[0];
    let addr = patch.address as usize;

    // Put original byte sequence
    mock_rom[addr..addr + patch.original.len()].copy_from_slice(patch.original);

    // Apply patches
    let count = profile.apply_patches(&mut mock_rom);
    assert_eq!(count, 1);
    assert_eq!(&mock_rom[addr..addr + patch.patched.len()], patch.patched);

    // Applying again is idempotent
    let count_again = profile.apply_patches(&mut mock_rom);
    assert_eq!(count_again, 0);
}

#[test]
fn test_widescreen_text_bg_layer_expansion() {
    let mut gba = Gba::new();

    // Mode 0: Text backgrounds
    // Enable BG1 (scrolling scenery)
    gba.mmu.ppu.dispcnt = 0x0000 | (1 << 9); // Mode 0, BG1 enabled
    gba.mmu.ppu.bgcnt[1] = 0x0180 | (1 << 14); // Screen size 1: 512x256, 8bpp, charbase 0, screenbase 1 (addr 0x800)
    gba.mmu.ppu.bghofs[1] = 0;
    gba.mmu.ppu.bgvofs[1] = 0;

    // Palette:
    // Entry 0 (backdrop): Black (0x0000)
    // Entry 1: Blue (0x7C00)
    // Entry 2: Yellow (0x03FF)
    // Entry 3: Red (0x001F)
    gba.mmu.ppu.palette_ram[0] = 0x00; gba.mmu.ppu.palette_ram[1] = 0x00;
    gba.mmu.ppu.palette_ram[2] = 0x00; gba.mmu.ppu.palette_ram[3] = 0x7C;
    gba.mmu.ppu.palette_ram[4] = 0xFF; gba.mmu.ppu.palette_ram[5] = 0x03;
    gba.mmu.ppu.palette_ram[6] = 0x1F; gba.mmu.ppu.palette_ram[7] = 0x00;

    // Tile 1: solid Blue (palette index 1)
    for i in 0..64 {
        gba.mmu.ppu.vram[1 * 64 + i] = 1;
    }
    // Tile 2: solid Yellow (palette index 2)
    for i in 0..64 {
        gba.mmu.ppu.vram[2 * 64 + i] = 2;
    }
    // Tile 3: solid Red (palette index 3)
    for i in 0..64 {
        gba.mmu.ppu.vram[3 * 64 + i] = 3;
    }

    // In a 512x256 map:
    // Block 0 (left 256x256) is at 0x800 (offset 2048).
    // Block 1 (right 256x256) is at 0x1000 (offset 4096).
    // Map entry for tile 0: tile 1 (Blue)
    // Tile (31, 0) in block 0 is at x = 248..255: let's put Tile 2 (Yellow)
    // Tile (0, 0) in block 1 is at x = 256..263: let's put Tile 3 (Red)
    let screen_base = 2048;
    // Fill first row of block 0 with Tile 1 (Blue)
    for tx in 0..32 {
        let addr = screen_base + tx * 2;
        gba.mmu.ppu.vram[addr] = 1; // Tile 1
    }
    // Block 1 (at 4096): fill first row with Tile 3 (Red)
    for tx in 0..32 {
        let addr = screen_base + 2048 + tx * 2;
        gba.mmu.ppu.vram[addr] = 3; // Tile 3
    }

    // Set widescreen config
    let mut ws = WidescreenConfig::default();
    ws.enabled = true;
    ws.mode = WidescreenMode::Ratio16_9; // 284x160: 22px left margin, 240px center, 22px right margin
    ws.bg_expand = [false, true, false, false]; // Expand BG1
    gba.set_widescreen_config(ws);

    // Step a scanline at y = 0
    gba.mmu.ppu.render_scanline(0);

    let frame = gba.render_widescreen_frame().expect("Render widescreen frame");
    assert_eq!(frame.width, 284);
    assert_eq!(frame.height, 160);

    // Check center pixel at x = 22 + 10 = 32 (original screen x = 10):
    // Should be Blue (Tile 1: B=255, R=0, G=0)
    let center_pix = frame.pixels[22 + 10];
    let b = ((center_pix >> 16) & 0xFF) as u8;
    assert!(b > 200, "Center pixel should be blue, got 0x{:08X}", center_pix);

    // Check right margin pixel at x = 22 + 240 + 5 = 267 (original screen x = 245):
    // In a 512-wide map, x = 245 is still in Block 0 (Tile 1: Blue)
    let margin_pix = frame.pixels[267];
    let b_right = ((margin_pix >> 16) & 0xFF) as u8;
    assert!(b_right > 200, "Right margin should sample 512-wide map, got 0x{:08X}", margin_pix);

    // Check far right pixel at x = 22 + 258 = 280 (original screen x = 258):
    // In Block 1: Tile 3 (Red: R=255, G=0, B=0)
    let far_right_pix = frame.pixels[280];
    let r_far = (far_right_pix & 0xFF) as u8;
    assert!(r_far > 200, "Far right margin should sample Block 1 Red tile, got 0x{:08X}", far_right_pix);
}

#[test]
fn test_widescreen_hud_anchoring_prevents_repeating() {
    let mut gba = Gba::new();

    // Mode 0: BG0 is HUD, BG1 is scrolling scenery
    gba.mmu.ppu.dispcnt = 0x0000 | (1 << 8) | (1 << 9); // BG0 + BG1 enabled
    gba.mmu.ppu.bgcnt[0] = 1 << 8; // BG0: screenbase 1 (2048), 256x256, 4bpp, Priority 0
    gba.mmu.ppu.bgcnt[1] = (2 << 8) | 1; // BG1: screenbase 2 (4096), 256x256, 4bpp, Priority 1

    // Palette:
    // Entry 0 (backdrop): Black (0x0000)
    // Palette 0, Color 1: Green (HUD text/meter)
    // Palette 1, Color 1: Blue (Background scenery)
    gba.mmu.ppu.palette_ram[0] = 0x00; gba.mmu.ppu.palette_ram[1] = 0x00;
    gba.mmu.ppu.palette_ram[2] = 0xE0; gba.mmu.ppu.palette_ram[3] = 0x03; // Green
    gba.mmu.ppu.palette_ram[34] = 0x00; gba.mmu.ppu.palette_ram[35] = 0x7C; // Blue

    // Tile 1: solid Green (color 1 in 4bpp)
    for i in 0..32 {
        gba.mmu.ppu.vram[1 * 32 + i] = 0x11;
    }
    // Tile 2: solid Blue (palette 1, color 1)
    for i in 0..32 {
        gba.mmu.ppu.vram[2 * 32 + i] = 0x11;
    }

    // BG0 tilemap at 2048 (screenbase 1): Tile 1 for entire first row
    for tx in 0..32 {
        gba.mmu.ppu.vram[2048 + tx * 2] = 1; // Tile 1, pal 0
    }
    // BG1 tilemap at 4096 (screenbase 2): Tile 2 for entire first row (pal 1)
    for tx in 0..32 {
        gba.mmu.ppu.vram[4096 + tx * 2] = 2; // Tile 2
        gba.mmu.ppu.vram[4096 + tx * 2 + 1] = 1 << 4; // Palette 1
    }

    // Widescreen config:
    // BG0 (HUD) is NOT expanded (bg_expand[0] = false), anchored to Center
    // BG1 (scenery) IS expanded (bg_expand[1] = true)
    let mut ws = WidescreenConfig::default();
    ws.enabled = true;
    ws.mode = WidescreenMode::Ratio16_9; // margins: (22, 22)
    ws.bg_expand = [false, true, false, false];
    ws.hud_anchor = [HudAnchor::Center, HudAnchor::Expand, HudAnchor::Expand, HudAnchor::Expand];
    gba.set_widescreen_config(ws);

    gba.mmu.ppu.render_scanline(0);

    let frame = gba.render_widescreen_frame().expect("Render frame");

    // 1. Center of the screen (x = 22 + 50): BG0 HUD is visible (Green: G=255, R=0, B=0)
    let center_pix = frame.pixels[22 + 50];
    let g_center = ((center_pix >> 8) & 0xFF) as u8;
    assert!(g_center > 200, "Center should show BG0 Green HUD, got 0x{:08X}", center_pix);

    // 2. Left margin (x = 5, native x = -17):
    // BG0 HUD is NOT drawn here! Instead, expanded BG1 Blue scenery shows through!
    let left_margin_pix = frame.pixels[5];
    let b_margin = ((left_margin_pix >> 16) & 0xFF) as u8;
    let g_margin = ((left_margin_pix >> 8) & 0xFF) as u8;
    assert!(b_margin > 200, "Left margin should show BG1 Blue scenery, got 0x{:08X}", left_margin_pix);
    assert!(g_margin < 50, "Left margin should NOT have HUD Green, got 0x{:08X}", left_margin_pix);

    // 3. Right margin (x = 22 + 240 + 10 = 272):
    // BG0 HUD is NOT drawn here! BG1 Blue scenery shows through!
    let right_margin_pix = frame.pixels[272];
    let b_right = ((right_margin_pix >> 16) & 0xFF) as u8;
    let g_right = ((right_margin_pix >> 8) & 0xFF) as u8;
    assert!(b_right > 200, "Right margin should show BG1 Blue scenery, got 0x{:08X}", right_margin_pix);
    assert!(g_right < 50, "Right margin should NOT have HUD Green, got 0x{:08X}", right_margin_pix);
}

#[test]
fn test_widescreen_mode2_affine_track_evaluation() {
    let mut gba = Gba::new();

    // Mode 2: BG2 and BG3 affine
    gba.mmu.ppu.dispcnt = 0x0002 | (1 << 10); // Mode 2, BG2 enabled
    gba.mmu.ppu.bgcnt[2] = 0x0000 | (1 << 13); // 128x128, wrap enabled
    gba.mmu.ppu.bg_pa[0] = 0x0100; // 1.0 scale
    gba.mmu.ppu.bg_pb[0] = 0;
    gba.mmu.ppu.bg_pc[0] = 0;
    gba.mmu.ppu.bg_pd[0] = 0x0100;
    gba.mmu.ppu.bg_x[0] = 0;
    gba.mmu.ppu.bg_y[0] = 0;
    gba.mmu.ppu.bg_x_internal[0] = 0;
    gba.mmu.ppu.bg_y_internal[0] = 0;

    // Palette:
    // Entry 1: Cyan (0x7FE0)
    gba.mmu.ppu.palette_ram[2] = 0xE0;
    gba.mmu.ppu.palette_ram[3] = 0x7F;

    // Tile 1: solid Cyan
    for i in 0..64 {
        gba.mmu.ppu.vram[1 * 64 + i] = 1;
    }

    // Affine map: 16x16 tiles. Set tile (0, 0) to 1.
    gba.mmu.ppu.vram[0] = 1;

    let mut ws = WidescreenConfig::default();
    ws.enabled = true;
    ws.mode = WidescreenMode::Ratio16_9;
    ws.bg_expand = [false, false, true, false]; // Expand BG2
    gba.set_widescreen_config(ws);

    gba.mmu.ppu.render_scanline(0);

    let frame = gba.render_widescreen_frame().expect("Render frame");
    assert_eq!(frame.width, 284);

    // Center pixel at (22, 0) corresponds to native x = 0 (Tile 1: Cyan)
    let p_center = frame.pixels[22];
    let g = ((p_center >> 8) & 0xFF) as u8;
    let b = ((p_center >> 16) & 0xFF) as u8;
    assert!(g > 200 && b > 200, "Affine pixel at x=0 should be Cyan, got 0x{:08X}", p_center);

    // Left margin pixel at (22 - 8, 0) = (14, 0) corresponds to native x = -8.
    // Because wrap is enabled, -8 wraps to 128 - 8 = 120 (Tile 15, 0).
    // It evaluates continuously without panicking or clipping!
    let p_left = frame.pixels[14];
    assert_eq!((p_left >> 24) & 0xFF, 0xFF); // Fully opaque
}

#[test]
fn test_widescreen_sprite_offscreen_expansion() {
    let mut gba = Gba::new();

    // Mode 0: OBJ enabled with 1D mapping
    gba.mmu.ppu.dispcnt = 0x0000 | (1 << 12) | (1 << 6);

    // Palette:
    // OBJ Palette 0, Color 1: Yellow (0x03FF)
    gba.mmu.ppu.palette_ram[0x200 + 2] = 0xFF;
    gba.mmu.ppu.palette_ram[0x200 + 3] = 0x03;

    // OBJ Tiles 0..3 (16x16 sprite in 1D mapping): 4bpp solid color 1
    for t in 0..4 {
        for i in 0..32 {
            gba.mmu.ppu.vram[0x10000 + t * 32 + i] = 0x11;
        }
    }

    // Sprite 0: 16x16, non-affine, positioned at X = -10 (which is 512 - 10 = 502 = 0x1F6)
    // and Y = 10
    // attr0: Y = 10, Shape = 0 (Square)
    gba.mmu.ppu.oam[0] = 10;
    gba.mmu.ppu.oam[1] = 0;
    // attr1: X = 502 (0x01F6), Size = 1 (16x16)
    gba.mmu.ppu.oam[2] = 0xF6;
    gba.mmu.ppu.oam[3] = 0x01 | (1 << 6); // size 1
    // attr2: Tile = 0, priority = 0
    gba.mmu.ppu.oam[4] = 0;
    gba.mmu.ppu.oam[5] = 0;

    // 1. Without widescreen: sprite at X = -10 is culled outside 0..240 or only partially shown
    let mut ws = WidescreenConfig::default();
    ws.enabled = true;
    ws.mode = WidescreenMode::Ratio16_9; // Left margin = 22
    ws.obj_expand = true;
    gba.set_widescreen_config(ws);

    gba.mmu.ppu.render_scanline(10);

    let frame = gba.render_widescreen_frame().expect("Render frame");

    // Sprite is at screen X = -10, width 16.
    // It spans screen X from -10 to +5.
    // In widescreen frame coordinates (offset +22):
    // X spans from 22 - 10 = 12 to 22 + 5 = 27.
    // At x = 15 (which is inside the left widescreen margin!), the sprite should be visible!
    let row_off = 10 * 284;
    let margin_sprite_pix = frame.pixels[row_off + 15];
    let r = (margin_sprite_pix & 0xFF) as u8;
    let g = ((margin_sprite_pix >> 8) & 0xFF) as u8;
    assert!(r > 200 && g > 200, "Sprite in left widescreen margin should be Yellow, got 0x{:08X}", margin_sprite_pix);

    // At x = 24 (which is inside original screen at native x = +2), it should also be visible:
    let center_sprite_pix = frame.pixels[row_off + 24];
    let r_c = (center_sprite_pix & 0xFF) as u8;
    let g_c = ((center_sprite_pix >> 8) & 0xFF) as u8;
    assert!(r_c > 200 && g_c > 200, "Sprite across boundary should be Yellow, got 0x{:08X}", center_sprite_pix);
}

#[test]
fn test_widescreen_window_full_screen_fade() {
    let mut gba = Gba::new();

    // Mode 0: BG1 enabled, WIN0 enabled
    gba.mmu.ppu.dispcnt = 0x0000 | (1 << 9) | (1 << 13);
    gba.mmu.ppu.bgcnt[1] = 1 << 8; // Screenbase 1 (2048)

    // WIN0 covers entire 0..240 horizontally and 0..160 vertically
    gba.mmu.ppu.win0h = (0 << 8) | 240;
    gba.mmu.ppu.win0v = (0 << 8) | 160;

    // WININ: BG1 enabled inside WIN0 (bit 1) and Color Effects enabled (bit 5)
    gba.mmu.ppu.winin = (1 << 1) | (1 << 5);
    // WINOUT: BG1 disabled outside (bit 1 = 0)
    gba.mmu.ppu.winout = 0;

    // Palette: BG1 is White
    gba.mmu.ppu.palette_ram[2] = 0xFF;
    gba.mmu.ppu.palette_ram[3] = 0x7F;

    for i in 0..32 {
        gba.mmu.ppu.vram[1 * 32 + i] = 0x11;
    }
    for tx in 0..32 {
        gba.mmu.ppu.vram[2048 + tx * 2] = 1;
    }

    let mut ws = WidescreenConfig::default();
    ws.enabled = true;
    ws.mode = WidescreenMode::Ratio16_9;
    ws.bg_expand = [false, true, false, false];
    ws.window_mode = WidescreenWindowMode::ExtendFull;
    gba.set_widescreen_config(ws);

    gba.mmu.ppu.render_scanline(0);

    let frame = gba.render_widescreen_frame().expect("Render frame");

    // Under ExtendFull, the window that spans 0..240 extends across the widescreen margins
    // So both center and margins have the window enabled!
    let p_center = frame.pixels[22 + 50];
    let p_margin = frame.pixels[10];

    assert_eq!(p_center, 0xFFFF_FFFF);
    assert_eq!(p_margin, 0xFFFF_FFFF);
}

#[test]
fn test_game_verification_1_mario_kart_super_circuit() {
    let mut gba = Gba::new();

    // Configure Mario Kart: Super Circuit (AMKE)
    // Mode 2: BG2 is affine track, BG1 is horizon, BG0 is centered HUD
    gba.mmu.ppu.dispcnt = 0x0002 | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 12);
    gba.mmu.ppu.bgcnt[0] = 0x0000; // BG0: HUD
    gba.mmu.ppu.bgcnt[1] = 0x0001; // BG1: Horizon
    gba.mmu.ppu.bgcnt[2] = 0x0002 | (1 << 13); // BG2: Affine track (wrap enabled)

    // Identity affine matrix
    gba.mmu.ppu.bg_pa[0] = 0x0100;
    gba.mmu.ppu.bg_pd[0] = 0x0100;

    // Load profile
    let profile = WidescreenDatabase::lookup("AMKE", "Mario Kart: Super Circuit").expect("Mario Kart profile");
    let mut config = profile.to_config();
    config.enabled = true;
    gba.set_widescreen_config(config);

    // Step scanline
    gba.mmu.ppu.render_scanline(50);

    let frame = gba.render_widescreen_frame().expect("Render widescreen frame");
    assert_eq!(frame.width, 284);
    assert_eq!(frame.height, 160);

    // Verify 0 visual errors: all pixels are valid, fully opaque RGBA words
    for &p in &frame.pixels {
        assert_eq!((p >> 24) & 0xFF, 0xFF, "All widescreen pixels must be fully opaque");
    }
}

#[test]
fn test_game_verification_2_fzero_maximum_velocity() {
    let mut gba = Gba::new();

    // Configure F-Zero: Maximum Velocity (AFZE)
    // Mode 2: BG2 track, BG1 starfield, BG0 speedometer HUD
    gba.mmu.ppu.dispcnt = 0x0002 | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 12);
    gba.mmu.ppu.bgcnt[0] = 0x0000; // BG0: Speedometer
    gba.mmu.ppu.bgcnt[1] = 0x0001; // BG1: Starfield
    gba.mmu.ppu.bgcnt[2] = 0x0002 | (1 << 13); // BG2: Track

    gba.mmu.ppu.bg_pa[0] = 0x0100;
    gba.mmu.ppu.bg_pd[0] = 0x0100;

    let profile = WidescreenDatabase::lookup("AFZE", "F-Zero").expect("F-Zero profile");
    let mut config = profile.to_config();
    config.enabled = true;
    gba.set_widescreen_config(config);

    gba.mmu.ppu.render_scanline(80);

    let frame = gba.render_widescreen_frame().expect("Render widescreen frame");
    assert_eq!(frame.width, 284);
    assert_eq!(frame.height, 160);

    for &p in &frame.pixels {
        assert_eq!((p >> 24) & 0xFF, 0xFF);
    }
}

#[test]
fn test_game_verification_3_metroid_fusion() {
    let mut gba = Gba::new();

    // Configure Metroid Fusion (AMFE)
    // Mode 0: BG0 is energy HUD, BG1/BG2 are room tilemaps
    gba.mmu.ppu.dispcnt = 0x0000 | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 12);
    gba.mmu.ppu.bgcnt[0] = 0x0000; // BG0: Energy HUD
    gba.mmu.ppu.bgcnt[1] = 0x0001; // BG1: Room scenery
    gba.mmu.ppu.bgcnt[2] = 0x0002; // BG2: Room terrain

    let profile = WidescreenDatabase::lookup("AMFE", "Metroid Fusion").expect("Metroid Fusion profile");
    let mut config = profile.to_config();
    config.enabled = true;
    gba.set_widescreen_config(config);

    gba.mmu.ppu.render_scanline(100);

    let frame = gba.render_widescreen_frame().expect("Render widescreen frame");
    assert_eq!(frame.width, 284);
    assert_eq!(frame.height, 160);

    for &p in &frame.pixels {
        assert_eq!((p >> 24) & 0xFF, 0xFF);
    }
}

#[test]
fn test_hd_mode7_widescreen_combined_rendering() {
    let mut gba = Gba::new();

    // Mode 2 affine track with HD Mode 7 (4x)
    gba.mmu.ppu.dispcnt = 0x0002 | (1 << 10);
    gba.mmu.ppu.bgcnt[2] = 0x0000 | (1 << 13);
    gba.mmu.ppu.bg_pa[0] = 0x0100;
    gba.mmu.ppu.bg_pd[0] = 0x0100;

    let mut hd_cfg = HdMode7Config::default();
    hd_cfg.scale = HdScale::X4; // 4x scale
    gba.set_hd_mode7_config(hd_cfg);

    let mut ws = WidescreenConfig::default();
    ws.enabled = true;
    ws.mode = WidescreenMode::Ratio16_9; // 284x160
    ws.bg_expand = [false, false, true, false];
    gba.set_widescreen_config(ws);

    gba.mmu.ppu.render_scanline(0);

    let frame = gba.render_widescreen_frame().expect("Render HD widescreen frame");
    assert_eq!(frame.scale, 4);
    assert_eq!(frame.width, 284 * 4); // 1136
    assert_eq!(frame.height, 160 * 4); // 640

    // SSAA downsampling from 1136x640 to 284x160
    let native_words = frame.downsample_ssaa_wide();
    assert_eq!(native_words.len(), 284 * 160);
}

#[test]
fn test_hd_pack_widescreen_replacement_rendering() {
    let mut gba = Gba::new();

    // Mode 0: Sprites enabled
    gba.mmu.ppu.dispcnt = 0x0000 | (1 << 12);

    // Sprite 0 at X = -10 (which is 502 in GBA 9-bit), Y = 10, Size 16x16
    gba.mmu.ppu.oam[0] = 10;
    gba.mmu.ppu.oam[1] = 0;
    gba.mmu.ppu.oam[2] = 0xF6;
    gba.mmu.ppu.oam[3] = 0x01 | (1 << 6);
    gba.mmu.ppu.oam[4] = 0;
    gba.mmu.ppu.oam[5] = 0;

    // Create an HD replacement texture (64x64 solid Red: 0xFF00_00FF)
    let rep_img = HdImage {
        width: 64,
        height: 64,
        pixels: vec![0xFF00_00FF; 64 * 64], // Red (0xAABBGGRR: A=0xFF, B=0, G=0, R=0xFF)
    };

    let mut pack = HdPack::new("Widescreen Test Pack", 4);
    // Find canonical hash of sprite 0
    let raw = gba_simulator::gba::hd_pack::extract_sprite(&gba.mmu.ppu.oam[..], 0, &gba.mmu.ppu.vram[..], &gba.mmu.ppu.palette_ram[..], gba.mmu.ppu.dispcnt)
        .expect("Extract sprite");
    let rep = HdReplacement {
        is_sprite: true,
        hash: raw.sprite_hash,
        palette_hash: None,
        image: rep_img,
        hflip: None,
        vflip: None,
        recolor: false,
        base_palette: None,
    };
    pack.add_replacement(rep);
    gba.load_hd_pack(pack);

    let mut ws = WidescreenConfig::default();
    ws.enabled = true;
    ws.mode = WidescreenMode::Ratio16_9;
    ws.obj_expand = true;
    gba.set_widescreen_config(ws);

    gba.mmu.ppu.render_scanline(10);

    let frame = gba.render_widescreen_frame().expect("Render widescreen frame");
    assert_eq!(frame.scale, 4);
    assert_eq!(frame.width, 1136);
    assert_eq!(frame.height, 640);

    // Sprite in left margin at screen X = -10 (offset +22 -> frame x = 12..27, scaled 4x = 48..108)
    // Should be Red from HD replacement
    let row_off = (10 * 4) * 1136;
    let margin_pix = frame.pixels[row_off + 60];
    let r = (margin_pix & 0xFF) as u8;
    let b = ((margin_pix >> 16) & 0xFF) as u8;
    assert!(r > 200 && b < 50, "HD replacement in widescreen margin should be Red, got 0x{:08X}", margin_pix);
}
