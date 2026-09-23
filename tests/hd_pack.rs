use gba_simulator::gba::{
    hd_pack::{
        extract_palette, extract_sprite, extract_tile_pixels,
        fnv1a_64, hash_palette, hash_tile, hash_to_hex, hex_to_hash,
        HdImage, HdPack, HdPackManifest, HdReplacement, HdReplacementEntry,
    },
    ppu::SCREEN_WIDTH,
    Gba,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_hd_pack_fnv1a_deterministic_hashing() {
    let data1 = b"CrabBoyAdvance_TestSprite1";
    let data2 = b"CrabBoyAdvance_TestSprite2";

    let hash1 = fnv1a_64(data1);
    let hash2 = fnv1a_64(data2);
    assert_ne!(hash1, hash2);

    // Determinism
    assert_eq!(hash1, fnv1a_64(data1));
    assert_eq!(hash2, fnv1a_64(data2));

    // Hex formatting and parsing
    let hex1 = hash_to_hex(hash1);
    assert_eq!(hex1.len(), 16);
    assert_eq!(hex_to_hash(&hex1), Some(hash1));
    assert_eq!(hex_to_hash(&format!("0x{}", hex1)), Some(hash1));
}

#[test]
fn test_hd_pack_tile_and_palette_extraction() {
    let mut vram = vec![0u8; 96 * 1024];
    let mut pal_ram = vec![0u8; 1024];

    // Configure 4bpp tile at address 0x1000
    let tile_addr = 0x1000;
    for i in 0..32 {
        vram[tile_addr + i] = ((i % 15) as u8) | (((15 - (i % 15)) as u8) << 4);
    }

    let pixels = extract_tile_pixels(&vram, false, tile_addr);
    assert_eq!(pixels.len(), 64);
    assert_eq!(pixels[0], 0); // 0 % 15
    assert_eq!(pixels[1], 15); // (15 - 0)
    let tile_h = hash_tile(&vram, false, tile_addr);
    assert_ne!(tile_h, 0);

    // Configure palette 1 for OBJ (offset 0x200 + 32)
    let pal_offset = 0x200 + 32;
    for i in 0..16 {
        let color = (i as u16) * 100;
        pal_ram[pal_offset + i * 2] = (color & 0xFF) as u8;
        pal_ram[pal_offset + i * 2 + 1] = ((color >> 8) & 0xFF) as u8;
    }

    let pal = extract_palette(&pal_ram, 1, false, true);
    assert_eq!(pal.len(), 16);
    assert_eq!(pal[1], 100);
    assert_eq!(pal[2], 200);

    let pal_h = hash_palette(&pal);
    assert_ne!(pal_h, 0);
}

#[test]
fn test_hd_pack_sprite_extraction_and_flip_invariance() {
    let mut oam = vec![0u8; 1024];
    let mut vram = vec![0u8; 96 * 1024];
    let pal_ram = vec![0u8; 1024];

    // Sprite 0: 16x16 (shape 0, size 1), 4bpp, 1D mapping, tile base 4, pal 2
    // attr0: y=40, shape=0 (square), 4bpp -> 40
    let raw_y = 40u16;
    oam[0] = (raw_y & 0xFF) as u8;
    oam[1] = 0;

    // attr1: x=60, size=1 (16x16) -> 0x4000 | 60
    let raw_x = 60u16;
    let attr1 = 0x4000 | raw_x;
    oam[2] = (attr1 & 0xFF) as u8;
    oam[3] = (attr1 >> 8) as u8;

    // attr2: tile_base=4, priority=1, pal=2
    let attr2 = (2 << 12) | (1 << 10) | 4;
    oam[4] = (attr2 & 0xFF) as u8;
    oam[5] = (attr2 >> 8) as u8;

    // Put distinct data into sprite tiles at 0x10000 + 4 * 32 = 0x10080
    for i in 0..128 {
        vram[0x10080 + i] = (i % 250) as u8;
    }

    // 1D mapping in dispcnt (bit 6 = 1)
    let dispcnt = 1 << 6;
    let spr = extract_sprite(&oam, 0, &vram, &pal_ram, dispcnt).expect("Sprite extraction failed");
    assert_eq!(spr.width, 16);
    assert_eq!(spr.height, 16);
    assert_eq!(spr.sprite_x, 60);
    assert_eq!(spr.raw_y, 40);
    assert_eq!(spr.pal_num, 2);
    assert_eq!(spr.priority, 1);
    assert!(!spr.hflip);
    assert!(!spr.vflip);

    let original_hash = spr.sprite_hash;

    // Now flip sprite horizontally (attr1 bit 12)
    let attr1_flipped = attr1 | (1 << 12);
    oam[2] = (attr1_flipped & 0xFF) as u8;
    oam[3] = (attr1_flipped >> 8) as u8;

    let spr_flipped = extract_sprite(&oam, 0, &vram, &pal_ram, dispcnt).expect("Sprite extraction failed");
    assert!(spr_flipped.hflip);
    // Un-flipped pixel extraction guarantees canonical hash invariance
    assert_eq!(spr_flipped.sprite_hash, original_hash);
}

#[test]
fn test_hd_pack_manifest_json_roundtrip() {
    let manifest = HdPackManifest {
        name: "Test Mario Advance HD".to_string(),
        version: "2.1.0".to_string(),
        author: "CrabBoy Artist".to_string(),
        game_title: Some("Super Mario Advance 2".to_string()),
        game_code: Some("AGBE".to_string()),
        scale: 4,
        replacements: vec![
            HdReplacementEntry {
                r#type: "sprite".to_string(),
                hash: "0123456789abcdef".to_string(),
                palette_hash: Some("fedcba9876543210".to_string()),
                file: "sprites/mario_walk.png".to_string(),
                x: Some(0),
                y: Some(0),
                width: Some(64),
                height: Some(64),
                hflip: None,
                vflip: None,
                recolor: true,
                base_palette: Some(vec![0x0000, 0x001F, 0x03E0]),
            },
            HdReplacementEntry {
                r#type: "tile".to_string(),
                hash: "aabbccddeeff0011".to_string(),
                palette_hash: None,
                file: "tiles/brick.png".to_string(),
                x: None,
                y: None,
                width: Some(32),
                height: Some(32),
                hflip: None,
                vflip: None,
                recolor: false,
                base_palette: None,
            },
        ],
    };

    let json_str = serde_json::to_string_pretty(&manifest).expect("Serialization failed");
    let deserialized: HdPackManifest = serde_json::from_str(&json_str).expect("Deserialization failed");

    assert_eq!(deserialized.name, manifest.name);
    assert_eq!(deserialized.scale, 4);
    assert_eq!(deserialized.replacements.len(), 2);
    assert_eq!(deserialized.replacements[0].hash, "0123456789abcdef");
    assert_eq!(deserialized.replacements[0].recolor, true);
    assert_eq!(deserialized.replacements[1].r#type, "tile");
}

#[test]
fn test_hd_pack_load_from_dir_and_png_assets() {
    let temp_dir = std::env::temp_dir().join(format!("crabboy_hd_pack_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(temp_dir.join("sprites")).expect("Create temp dir");

    // Write a sample 32x32 PNG (e.g. 4x replacement for an 8x8 sprite)
    let png_path = temp_dir.join("sprites/test_sprite.png");
    let mut raw_rgba = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32 {
        for x in 0..32 {
            raw_rgba.push((x * 7) as u8); // R
            raw_rgba.push((y * 7) as u8); // G
            raw_rgba.push(200);           // B
            raw_rgba.push(255);           // A
        }
    }
    image::save_buffer(&png_path, &raw_rgba, 32, 32, image::ExtendedColorType::Rgba8)
        .expect("Save test PNG");

    let manifest = HdPackManifest {
        name: "Mock HD Pack".to_string(),
        version: "1.0".to_string(),
        author: "Tester".to_string(),
        game_title: None,
        game_code: None,
        scale: 4,
        replacements: vec![HdReplacementEntry {
            r#type: "sprite".to_string(),
            hash: "1122334455667788".to_string(),
            palette_hash: Some("99aabbccddeeff00".to_string()),
            file: "sprites/test_sprite.png".to_string(),
            x: None,
            y: None,
            width: Some(32),
            height: Some(32),
            hflip: None,
            vflip: None,
            recolor: false,
            base_palette: None,
        }],
    };

    HdPack::save_manifest(&manifest, temp_dir.join("manifest.json")).expect("Save manifest");

    // Load pack from directory
    let pack = HdPack::load_from_dir(&temp_dir).expect("Load pack from dir");
    assert_eq!(pack.name, "Mock HD Pack");
    assert_eq!(pack.scale, 4);
    assert_eq!(pack.sprite_count(), 1);
    assert_eq!(pack.len(), 1);

    let hash = 0x1122334455667788u64;
    let pal_hash = 0x99aabbccddeeff00u64;

    let rep = pack.find_sprite_replacement(hash, pal_hash).expect("Found replacement");
    assert_eq!(rep.image.width, 32);
    assert_eq!(rep.image.height, 32);
    assert_eq!(rep.image.pixels.len(), 32 * 32);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_hd_pack_high_resolution_sprite_replacement_rendering() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    // Setup an 8x8 sprite in OAM at (x=20, y=30)
    // attr0: y=30, shape=0
    gba.mmu.ppu.oam[0] = 30;
    gba.mmu.ppu.oam[1] = 0;
    // attr1: x=20, size=0 (8x8)
    gba.mmu.ppu.oam[2] = 20;
    gba.mmu.ppu.oam[3] = 0;
    // attr2: tile_base=0, pal=0
    gba.mmu.ppu.oam[4] = 0;
    gba.mmu.ppu.oam[5] = 0;

    // Fill sprite tile 0 with non-zero color index
    for i in 0..32 {
        gba.mmu.ppu.vram[0x10000 + i] = 0x11; // index 1 in both nibbles
    }

    // Set palette 0 entry 1 to red (BGR555: 0x001F)
    gba.mmu.ppu.palette_ram[0x200 + 2] = 0x1F;
    gba.mmu.ppu.palette_ram[0x200 + 3] = 0x00;

    // Enable OBJ layer and Screen in DISPCNT
    gba.mmu.ppu.dispcnt = 0x1000; // bit 12: OBJ enable

    // Extract the sprite so we get its exact hash
    let raw = extract_sprite(&gba.mmu.ppu.oam[..], 0, &gba.mmu.ppu.vram[..], &gba.mmu.ppu.palette_ram[..], gba.mmu.ppu.dispcnt)
        .expect("Sprite extraction");

    // Create an HD pack with a 4x HD replacement (32x32 pixels, cyan: R=0, G=255, B=255)
    let mut pack = HdPack::new("Test Sprite HD Pack", 4);
    let cyan = 0xFFFF_FF00; // little-endian: RGBA cyan (R=0, G=255, B=255, A=255)
    let hd_img = HdImage {
        width: 32,
        height: 32,
        pixels: vec![cyan; 32 * 32],
    };

    pack.add_replacement(HdReplacement {
        is_sprite: true,
        hash: raw.sprite_hash,
        palette_hash: Some(raw.palette_hash),
        image: hd_img,
        hflip: None,
        vflip: None,
        recolor: false,
        base_palette: None,
    });

    gba.load_hd_pack(pack);
    gba.set_hd_pack_enabled(true);

    // Run 1 frame to render scanlines and capture layer buffers
    gba.run_frame();

    // Render HD frame
    let hd_frame = gba.render_hd_frame().expect("HD frame rendered");
    assert_eq!(hd_frame.width, 960);
    assert_eq!(hd_frame.height, 640);

    // Verify subpixel sampling at sprite location: (x=20*4=80, y=30*4=120)
    let sample_x = 20 * 4 + 4;
    let sample_y = 30 * 4 + 4;
    let pix = hd_frame.pixels[sample_y * hd_frame.width + sample_x];

    let r = (pix & 0xFF) as u8;
    let g = ((pix >> 8) & 0xFF) as u8;
    let b = ((pix >> 16) & 0xFF) as u8;

    // Cyan: R=0, G=255, B=255
    assert_eq!(r, 0);
    assert_eq!(g, 255);
    assert_eq!(b, 255);
}

#[test]
fn test_hd_pack_sprite_animation_frame_swapping() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);
    gba.mmu.ppu.dispcnt = 0x1000; // OBJ enable

    // Sprite 0 placed at (x=40, y=40), size 8x8
    gba.mmu.ppu.oam[0] = 40;
    gba.mmu.ppu.oam[1] = 0;
    gba.mmu.ppu.oam[2] = 40;
    gba.mmu.ppu.oam[3] = 0;

    // Set palette 0 entry 1 to White
    gba.mmu.ppu.palette_ram[0x200 + 2] = 0xFF;
    gba.mmu.ppu.palette_ram[0x200 + 3] = 0x7F;

    // Animation Frame 0: Tile 0 (filled with 0x11)
    for i in 0..32 {
        gba.mmu.ppu.vram[0x10000 + i] = 0x11;
    }
    // Animation Frame 1: Tile 1 (filled with 0x22)
    for i in 0..32 {
        gba.mmu.ppu.vram[0x10020 + i] = 0x22;
    }

    // Set sprite to Frame 0 (tile 0)
    gba.mmu.ppu.oam[4] = 0;
    gba.mmu.ppu.oam[5] = 0;
    let raw_frame0 = extract_sprite(&gba.mmu.ppu.oam[..], 0, &gba.mmu.ppu.vram[..], &gba.mmu.ppu.palette_ram[..], gba.mmu.ppu.dispcnt)
        .expect("Extract frame 0");

    // Set sprite to Frame 1 (tile 1)
    gba.mmu.ppu.oam[4] = 1;
    gba.mmu.ppu.oam[5] = 0;
    let raw_frame1 = extract_sprite(&gba.mmu.ppu.oam[..], 0, &gba.mmu.ppu.vram[..], &gba.mmu.ppu.palette_ram[..], gba.mmu.ppu.dispcnt)
        .expect("Extract frame 1");

    assert_ne!(raw_frame0.sprite_hash, raw_frame1.sprite_hash);

    // Create HD pack with distinct HD replacements for the two animation frames
    let mut pack = HdPack::new("Animation Test Pack", 4);

    // Frame 0 replacement: Pure Red (0xFF0000FF)
    let red_pixel = 0xFF00_00FF;
    pack.add_replacement(HdReplacement {
        is_sprite: true,
        hash: raw_frame0.sprite_hash,
        palette_hash: None, // Wildcard
        image: HdImage {
            width: 32,
            height: 32,
            pixels: vec![red_pixel; 32 * 32],
        },
        hflip: None,
        vflip: None,
        recolor: false,
        base_palette: None,
    });

    // Frame 1 replacement: Pure Green (0xFF00FF00)
    let green_pixel = 0xFF00_FF00;
    pack.add_replacement(HdReplacement {
        is_sprite: true,
        hash: raw_frame1.sprite_hash,
        palette_hash: None, // Wildcard
        image: HdImage {
            width: 32,
            height: 32,
            pixels: vec![green_pixel; 32 * 32],
        },
        hflip: None,
        vflip: None,
        recolor: false,
        base_palette: None,
    });

    gba.load_hd_pack(pack);
    gba.set_hd_pack_enabled(true);

    // Step A: Game displays Animation Frame 0 (tile 0)
    gba.mmu.ppu.oam[4] = 0;
    gba.run_frame();
    let hd0 = gba.render_hd_frame().expect("Render HD frame 0");
    let sample_x = 40 * 4 + 4;
    let sample_y = 40 * 4 + 4;
    let pix0 = hd0.pixels[sample_y * hd0.width + sample_x];
    let (r0, g0, b0) = ((pix0 & 0xFF) as u8, ((pix0 >> 8) & 0xFF) as u8, ((pix0 >> 16) & 0xFF) as u8);
    assert_eq!((r0, g0, b0), (255, 0, 0)); // Red

    // Step B: Game advances animation to Frame 1 (tile 1)
    gba.mmu.ppu.oam[4] = 1;
    gba.run_frame();
    let hd1 = gba.render_hd_frame().expect("Render HD frame 1");
    let pix1 = hd1.pixels[sample_y * hd1.width + sample_x];
    let (r1, g1, b1) = ((pix1 & 0xFF) as u8, ((pix1 >> 8) & 0xFF) as u8, ((pix1 >> 16) & 0xFF) as u8);
    assert_eq!((r1, g1, b1), (0, 255, 0)); // Green
}

#[test]
fn test_hd_pack_palette_variation_and_dynamic_recoloring() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);
    gba.mmu.ppu.dispcnt = 0x1000;

    // Sprite 0 at (x=50, y=50), size 8x8, tile 0, pal 0
    gba.mmu.ppu.oam[0] = 50;
    gba.mmu.ppu.oam[1] = 0;
    gba.mmu.ppu.oam[2] = 50;
    gba.mmu.ppu.oam[3] = 0;
    gba.mmu.ppu.oam[4] = 0;
    gba.mmu.ppu.oam[5] = 0;

    for i in 0..32 {
        gba.mmu.ppu.vram[0x10000 + i] = 0x11;
    }

    // Normal palette: entry 1 is Blue (0x7C00)
    let normal_blue = 0x7C00u16;
    gba.mmu.ppu.palette_ram[0x200 + 2] = (normal_blue & 0xFF) as u8;
    gba.mmu.ppu.palette_ram[0x200 + 3] = ((normal_blue >> 8) & 0xFF) as u8;

    let normal_pal = extract_palette(&gba.mmu.ppu.palette_ram[..], 0, false, true);
    let normal_pal_hash = hash_palette(&normal_pal);

    // Fire palette: entry 1 is Red (0x001F)
    let fire_red = 0x001Fu16;
    let mut fire_pal = normal_pal.clone();
    fire_pal[1] = fire_red;
    let fire_pal_hash = hash_palette(&fire_pal);

    assert_ne!(normal_pal_hash, fire_pal_hash);

    let raw = extract_sprite(&gba.mmu.ppu.oam[..], 0, &gba.mmu.ppu.vram[..], &gba.mmu.ppu.palette_ram[..], gba.mmu.ppu.dispcnt)
        .expect("Extract sprite");

    // Pack with exact palette match for Normal Mario (Blue HD texture)
    // and exact palette match for Fire Mario (Red HD texture)
    let mut pack = HdPack::new("Palette Test Pack", 4);

    let blue_hd = 0xFFFF_0000; // Blue (RGBA little-endian: R=0, G=0, B=255)
    let red_hd = 0xFF00_00FF;  // Red

    pack.add_replacement(HdReplacement {
        is_sprite: true,
        hash: raw.sprite_hash,
        palette_hash: Some(normal_pal_hash),
        image: HdImage {
            width: 32,
            height: 32,
            pixels: vec![blue_hd; 32 * 32],
        },
        hflip: None,
        vflip: None,
        recolor: false,
        base_palette: None,
    });

    pack.add_replacement(HdReplacement {
        is_sprite: true,
        hash: raw.sprite_hash,
        palette_hash: Some(fire_pal_hash),
        image: HdImage {
            width: 32,
            height: 32,
            pixels: vec![red_hd; 32 * 32],
        },
        hflip: None,
        vflip: None,
        recolor: false,
        base_palette: None,
    });

    gba.load_hd_pack(pack);
    gba.set_hd_pack_enabled(true);

    // 1. Emulate with Normal Palette -> renders blue HD texture
    gba.run_frame();
    let hd_norm = gba.render_hd_frame().expect("Render normal");
    let sample_x = 50 * 4 + 4;
    let sample_y = 50 * 4 + 4;
    let p_norm = hd_norm.pixels[sample_y * hd_norm.width + sample_x];
    assert_eq!(((p_norm & 0xFF) as u8, ((p_norm >> 8) & 0xFF) as u8, ((p_norm >> 16) & 0xFF) as u8), (0, 0, 255));

    // 2. Game picks up Fire Flower: update palette in Palette RAM
    gba.mmu.ppu.palette_ram[0x200 + 2] = (fire_red & 0xFF) as u8;
    gba.mmu.ppu.palette_ram[0x200 + 3] = ((fire_red >> 8) & 0xFF) as u8;

    gba.run_frame();
    let hd_fire = gba.render_hd_frame().expect("Render fire");
    let p_fire = hd_fire.pixels[sample_y * hd_fire.width + sample_x];
    assert_eq!(((p_fire & 0xFF) as u8, ((p_fire >> 8) & 0xFF) as u8, ((p_fire >> 16) & 0xFF) as u8), (255, 0, 0));
}

#[test]
fn test_hd_pack_tile_and_sprite_dump_tool() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);
    gba.mmu.ppu.dispcnt = 0x1000; // OBJ enable

    // Put a sprite in OAM
    gba.mmu.ppu.oam[0] = 20; // y
    gba.mmu.ppu.oam[2] = 30; // x
    gba.mmu.ppu.oam[4] = 0;  // tile 0

    // Fill sprite tile 0
    for i in 0..32 {
        gba.mmu.ppu.vram[0x10000 + i] = 0x33;
    }
    // Fill a BG tile in VRAM (tile 1)
    for i in 0..32 {
        gba.mmu.ppu.vram[32 + i] = 0x44;
    }

    gba.run_frame();

    let temp_dir = std::env::temp_dir().join(format!("crabboy_dump_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);

    let manifest = gba.dump_tiles_and_sprites(&temp_dir).expect("Dump tiles and sprites");
    assert!(!manifest.replacements.is_empty());
    assert!(temp_dir.join("manifest.json").exists());
    assert!(temp_dir.join("sprites").exists());
    assert!(temp_dir.join("tiles").exists());

    // Verify pack can be loaded directly from the dumped directory
    let loaded = HdPack::load_from_dir(&temp_dir).expect("Load dumped pack");
    assert_eq!(loaded.name, "Dumped Pack Template");
    assert!(loaded.len() > 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_hd_pack_ssaa_downsampling_preserves_subpixel_fidelity() {
    let mut gba = Gba::new_headless();
    let mut rom = vec![0u8; 1024];
    rom[0..4].copy_from_slice(&0xEAFFFFFEu32.to_le_bytes());
    gba.load_rom_bytes(rom);
    gba.mmu.ppu.dispcnt = 0x1000;

    gba.mmu.ppu.oam[0] = 10;
    gba.mmu.ppu.oam[2] = 10;
    for i in 0..32 {
        gba.mmu.ppu.vram[0x10000 + i] = 0x11;
    }

    let raw = extract_sprite(&gba.mmu.ppu.oam[..], 0, &gba.mmu.ppu.vram[..], &gba.mmu.ppu.palette_ram[..], gba.mmu.ppu.dispcnt)
        .expect("Extract sprite");

    let mut pack = HdPack::new("SSAA Test Pack", 4);
    let yellow = 0xFF00_FFFF; // Yellow
    pack.add_replacement(HdReplacement {
        is_sprite: true,
        hash: raw.sprite_hash,
        palette_hash: None,
        image: HdImage {
            width: 32,
            height: 32,
            pixels: vec![yellow; 32 * 32],
        },
        hflip: None,
        vflip: None,
        recolor: false,
        base_palette: None,
    });

    gba.load_hd_pack(pack);
    gba.set_hd_pack_enabled(true);
    gba.run_frame();

    let hd = gba.render_hd_frame().expect("Render HD frame");
    assert_eq!(hd.width, 960);
    assert_eq!(hd.height, 640);

    // Box-filter SSAA downsample to native 240x160
    let native_words = hd.downsample_ssaa();
    assert_eq!(native_words.len(), 240 * 160);

    // Pixel at (10, 10) in native framebuffer should contain the sprite's color
    let native_pix = native_words[10 * SCREEN_WIDTH + 10];
    let r = (native_pix & 0xFF) as u8;
    let g = ((native_pix >> 8) & 0xFF) as u8;

    assert!(r > 200);
    assert!(g > 200);
}

#[test]
fn test_hd_pack_sample_pack_verification() {
    let sample_dir = PathBuf::from("assets/sample_hd_pack");
    let sprites_dir = sample_dir.join("sprites");
    fs::create_dir_all(&sprites_dir).expect("Create sample sprites dir");

    let create_test_png = |path: &std::path::Path, r: u8, g: u8, b: u8| {
        let mut bytes = Vec::with_capacity(64 * 64 * 4);
        for _ in 0..(64 * 64) {
            bytes.push(r);
            bytes.push(g);
            bytes.push(b);
            bytes.push(255);
        }
        image::save_buffer(path, &bytes, 64, 64, image::ExtendedColorType::Rgba8)
            .expect("Save sample PNG");
    };

    create_test_png(&sprites_dir.join("hero_idle.png"), 0, 0, 255);   // Blue
    create_test_png(&sprites_dir.join("hero_walk1.png"), 0, 255, 255); // Cyan
    create_test_png(&sprites_dir.join("hero_walk2.png"), 0, 255, 0);   // Green
    create_test_png(&sprites_dir.join("hero_fire.png"), 255, 0, 0);    // Red

    // Load pack from assets/sample_hd_pack
    let pack = HdPack::load_from_dir(&sample_dir).expect("Load sample pack");
    assert_eq!(pack.name, "Sample Crab Hero HD Pack");
    assert_eq!(pack.scale, 4);
    assert_eq!(pack.len(), 4);
    assert_eq!(pack.sprite_count(), 4);

    let idle_hash = 0x1000000000000001u64;
    let walk1_hash = 0x1000000000000002u64;
    let walk2_hash = 0x1000000000000003u64;
    let fire_pal_hash = 0x2000000000000001u64;
    let any_pal_hash = 0x9999999999999999u64;

    // 1. Wildcard animation frame lookups
    let idle_rep = pack.find_sprite_replacement(idle_hash, any_pal_hash).expect("Found idle");
    assert_eq!(idle_rep.image.width, 64);
    assert_eq!(idle_rep.image.height, 64);
    // Blue: R=0, G=0, B=255
    let p_idle = idle_rep.image.pixels[0];
    assert_eq!(((p_idle & 0xFF) as u8, ((p_idle >> 8) & 0xFF) as u8, ((p_idle >> 16) & 0xFF) as u8), (0, 0, 255));

    let walk1_rep = pack.find_sprite_replacement(walk1_hash, any_pal_hash).expect("Found walk 1");
    let p_w1 = walk1_rep.image.pixels[0];
    assert_eq!(((p_w1 & 0xFF) as u8, ((p_w1 >> 8) & 0xFF) as u8, ((p_w1 >> 16) & 0xFF) as u8), (0, 255, 255));

    let walk2_rep = pack.find_sprite_replacement(walk2_hash, any_pal_hash).expect("Found walk 2");
    let p_w2 = walk2_rep.image.pixels[0];
    assert_eq!(((p_w2 & 0xFF) as u8, ((p_w2 >> 8) & 0xFF) as u8, ((p_w2 >> 16) & 0xFF) as u8), (0, 255, 0));

    // 2. Palette variant match: same idle hash + fire palette -> Red fire sprite
    let fire_rep = pack.find_sprite_replacement(idle_hash, fire_pal_hash).expect("Found fire variant");
    let p_fire = fire_rep.image.pixels[0];
    assert_eq!(((p_fire & 0xFF) as u8, ((p_fire >> 8) & 0xFF) as u8, ((p_fire >> 16) & 0xFF) as u8), (255, 0, 0));
}
