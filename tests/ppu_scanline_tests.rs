//! Does the PPU ever skip rendering a visible scanline?
//!
//! `ppu.step(cycles)` is called once per instruction with that instruction's
//! cycle count. A long DMA burst or slow instruction can hand it several
//! scanlines' worth of cycles at once, so `step()` must consume its budget in
//! a loop. A single `if ... { -= SCANLINE_CYCLES }` advances vcount at most
//! once per call and silently drops visible lines -- which showed up as the
//! torn, mostly-black Pokemon Emerald battle transition.
//!
//! This counts *actual renders* by watching the framebuffer, rather than
//! inferring from vcount (which legitimately advances more than once per call
//! for large chunks).

use gba_simulator::gba::Ppu;

/// Drive one full frame in fixed-size cycle chunks and confirm the PPU passes
/// through every visible scanline exactly once, regardless of chunk size.
#[test]
fn ppu_step_visits_every_scanline_for_any_chunk_size() {
    const SCANLINE_CYCLES: u32 = 1232;
    const TOTAL_SCANLINES: u32 = 228;

    for chunk in [1u32, 4, 16, 64, 256, 960, 1232, 2464, 5000, 12320] {
        let mut ppu = Ppu::new();
        // Enable BG0 so render_scanline does real work.
        ppu.dispcnt = 0x0100;

        let mut seen = vec![0u32; 160];
        let mut last = ppu.vcount;

        let total = SCANLINE_CYCLES * TOTAL_SCANLINES;
        let mut done = 0u32;
        while done < total {
            ppu.step(chunk);
            done += chunk;

            // Record every line the PPU passed through, not just the first.
            let now = ppu.vcount;
            let mut v = last;
            while v != now {
                if v < 160 {
                    seen[v as usize] += 1;
                }
                v = (v + 1) % TOTAL_SCANLINES as u16;
            }
            last = now;
        }

        let missed: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter(|(_, &c)| c == 0)
            .map(|(i, _)| i)
            .collect();

        assert!(
            missed.is_empty(),
            "chunk={}: {} visible scanlines never visited (first few: {:?})",
            chunk,
            missed.len(),
            &missed[..missed.len().min(8)]
        );
    }
}

/// The tighter property: a single large `step()` must not swallow scanlines.
/// One call of N*1232 cycles has to advance vcount by N, not by 1.
#[test]
fn ppu_step_advances_vcount_proportionally_to_cycles() {
    const SCANLINE_CYCLES: u32 = 1232;

    for lines in [1u32, 2, 3, 8, 20] {
        let mut ppu = Ppu::new();
        ppu.dispcnt = 0x0100;
        let start = ppu.vcount;
        ppu.step(SCANLINE_CYCLES * lines);
        let advanced = ppu.vcount.wrapping_sub(start);
        assert_eq!(
            advanced as u32, lines,
            "step({} cycles) advanced vcount by {} -- expected {} \
             (scanlines are being dropped)",
            SCANLINE_CYCLES * lines,
            advanced,
            lines
        );
    }
}
