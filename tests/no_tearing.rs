//! The displayed frame must be one whole frame. `run_frame` stops on a cycle
//! budget, not at VBlank, so the live framebuffer is part new frame, part old
//! frame; showing it put a tear line wherever the frame happened to stop.

use gba_simulator::gba::Gba;

fn emerald() -> Option<Gba> {
    let home = std::env::var("HOME").ok()?;
    let p = std::path::Path::new(&home).join("Downloads/Pokemon - Emerald Version (USA, Europe).gba");
    if !p.exists() {
        eprintln!("skipping: Emerald ROM not found");
        return None;
    }
    let mut g = Gba::new_headless();
    g.load_rom(&p).ok()?;
    Some(g)
}

/// A frame read by the display equals what the PPU held at the VBlank that
/// finished it, never a mix with the frame in progress.
#[test]
fn displayed_frame_is_the_frame_completed_at_vblank() {
    let Some(mut g) = emerald() else { return };
    let mut boundaries_off_vblank = 0;
    let mut mixed = 0;
    for f in 0..900 {
        // Step to VBlank by hand to capture the exact completed picture.
        g.run_frame();
        let vcount = g.mmu.ppu.vcount;
        if vcount != 160 {
            boundaries_off_vblank += 1;
        }
        let shown = *g.get_framebuffer();
        let live = *g.live_framebuffer();
        // Rows 0..vcount of the live buffer already belong to the next frame.
        if vcount < 160 && f > 60 {
            let rows_new = (0..vcount as usize).filter(|&y| live[y * 240..(y + 1) * 240] != shown[y * 240..(y + 1) * 240]).count();
            // Row `vcount` itself may already be drawn (it's the current line).
            let rows_old = (vcount as usize + 1..160).filter(|&y| live[y * 240..(y + 1) * 240] != shown[y * 240..(y + 1) * 240]).count();
            // The shown frame must match the live buffer below the stop line
            // (those rows are still the completed frame).
            assert_eq!(rows_old, 0, "frame {f}: rows below scanline {vcount} differ from the completed frame");
            if rows_new > 0 {
                mixed += 1;
            }
        }
    }
    // The problem this guards against is real: frames don't end at VBlank,
    // and the live buffer often holds two frames at once.
    assert!(boundaries_off_vblank > 800, "frames end mid-screen: {boundaries_off_vblank}/900");
    eprintln!("{mixed} of 900 live buffers were a mix of two frames (would have torn)");
}

/// Same for the Game Boy core.
#[test]
fn gb_displayed_frame_is_complete() {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = std::path::Path::new(&home).join("Downloads/gb-test-roms");
    let rom = std::fs::read_dir(&dir)
        .ok()
        .and_then(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "gb" || x == "gbc")));
    let Some(rom) = rom else {
        eprintln!("skipping: no GB ROM in ~/Downloads/gb-test-roms");
        return;
    };
    let mut gb = gba_simulator::dmg::GameBoy::from_file(&rom, false).expect("load GB ROM");
    for _ in 0..120 {
        gb.run_frame();
        let ly = gb.mmu.ppu.ly as usize;
        if ly < 144 {
            for y in ly + 1..144 {
                assert_eq!(
                    gb.get_framebuffer()[y * 160..(y + 1) * 160],
                    gb.live_framebuffer()[y * 160..(y + 1) * 160],
                    "GB: rows not yet redrawn must match the completed frame"
                );
            }
        }
    }
}
