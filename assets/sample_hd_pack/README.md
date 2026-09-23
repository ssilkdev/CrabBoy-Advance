# CrabBoy Advance HD Asset Replacement Pack

This folder contains a sample high-definition asset replacement pack for CrabBoy Advance, following the Mesen-style hash-keyed replacement architecture (ROADMAP M7).

## Pack Structure

```
sample_hd_pack/
├── manifest.json       # Asset manifest defining mappings and parameters
├── README.md           # Documentation for creators
├── sprites/            # High-resolution sprite assets (PNG)
│   ├── hero_idle.png
│   ├── hero_walk1.png
│   ├── hero_walk2.png
│   └── hero_fire.png
└── tiles/              # High-resolution background tile assets (PNG)
```

## How It Works

1. **Deterministic Hashing**:
   - Each GBA 8x8 tile or composite sprite is identified by a 64-bit FNV-1a hash of its raw pixel data in canonical (un-flipped) orientation.
   - Each palette is identified by a 64-bit FNV-1a hash of its 16 or 256 BGR555 entries.

2. **Animation Support**:
   - As characters walk, jump, or idle, games swap tile indices or DMA new graphics to VRAM.
   - Because each animation frame has a distinct hash, the replacement engine maps each frame to its corresponding high-definition asset (`hero_idle.png`, `hero_walk1.png`, etc.).

3. **Palette Changes & Recoloring**:
   - **Variant matching**: Provide specific high-res art for different palettes (e.g. Normal Mario vs Fire Mario, Normal vs Shiny Pokémon) by specifying `palette_hash`.
   - **Dynamic recoloring**: Set `"recolor": true` to have the emulator dynamically remap the high-res texture's colors to match game palette shifts (damage flashing, day/night cycles).
   - **Wildcard matching**: Omit `palette_hash` or set it to `"*"` to apply the replacement across all palettes.

## Creating Your Own HD Packs

### 1. Dump Tiles and Sprites from Gameplay
Run CrabBoy Advance with the `--dump-tiles` CLI option:
```bash
crabboy-advance --dump-tiles <ROM_PATH> --frame 120 --output my_hd_pack/
```
Or click **Video -> HD Sprite & Tile Packs -> Dump Tiles & Sprites to Folder...** in the emulator GUI.

This creates:
- `sprites/sprite_{hash}_{palette_hash}_{w}x{h}.png`: All active sprites in the scene.
- `tiles/tile_{hash}_{palette_hash}.png`: All unique background tiles in VRAM.
- `manifest.json`: Pre-populated configuration ready for editing.

### 2. Draw High-Resolution Art
Open the dumped PNGs in your image editor (Aseprite, Photoshop, GIMP, Krita) and draw higher resolution art at 2x, 4x, or 8x scale (e.g. 64x64 for a 16x16 sprite).

### 3. Load the Pack in CrabBoy Advance
Launch via CLI:
```bash
crabboy-advance <ROM_PATH> --hd-pack my_hd_pack/
```
Or open the GUI menu: **Video -> HD Sprite & Tile Packs -> Load HD Pack Folder...**.
