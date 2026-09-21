//! Drive the real game into a wild grass battle and watch for the crash.
//!
//! Loads the ROM (and its adjacent .sav battery file), boots through the
//! title/continue screens, then walks back-and-forth in grass until the
//! encounter triggers, asserting every frame that the CPU stays in
//! executable memory.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;

fn region(addr: u32) -> &'static str {
    match (addr >> 24) & 0xFF {
        0x00 => "BIOS",
        0x02 => "EWRAM",
        0x03 => "IWRAM",
        0x04 => "IO-REGS(BAD)",
        0x05 => "PALETTE",
        0x06 => "VRAM",
        0x07 => "OAM",
        0x08..=0x0F => "ROM",
        _ => "UNMAPPED(BAD)",
    }
}

fn bad_pc(pc: u32) -> bool {
    let r = (pc >> 24) & 0xFF;
    // Executable: BIOS, EWRAM, IWRAM, VRAM(rare but legal), ROM.
    !matches!(r, 0x00 | 0x02 | 0x03 | 0x06 | 0x08..=0x0F)
}

struct Watch {
    worst: Option<(usize, u32, u32, u32)>, // frame, pc, cpsr, prev_pc
    und_frames: Vec<usize>,
    prev_pc: u32,
}

impl Watch {
    fn check(&mut self, gba: &Gba, frame: usize) -> bool {
        let pc = gba.cpu.regs[15];
        let mode = gba.cpu.cpsr & 0x1F;
        if mode == 0x1B || mode == 0x17 {
            self.und_frames.push(frame);
        }
        if bad_pc(pc) && self.worst.is_none() {
            self.worst = Some((frame, pc, gba.cpu.cpsr, self.prev_pc));
            return true;
        }
        self.prev_pc = pc;
        false
    }
}

fn press(gba: &mut Gba, keys: &[Key], frames: usize, w: &mut Watch, base: usize) -> Option<usize> {
    for f in 0..frames {
        for k in keys {
            gba.mmu.keypad.set_key_state(*k, true);
        }
        gba.run_frame();
        for k in keys {
            gba.mmu.keypad.set_key_state(*k, false);
        }
        if w.check(gba, base + f) {
            return Some(base + f);
        }
    }
    None
}

#[test]
#[ignore]
fn grass_battle_does_not_crash() {
    let Ok(rom) = std::env::var("GBA_TEST_ROM") else {
        eprintln!("skipping: set GBA_TEST_ROM");
        return;
    };

    let mut gba = Gba::new();
    gba.load_rom(&rom).expect("load rom + .sav");

    let mut w = Watch {
        worst: None,
        und_frames: Vec::new(),
        prev_pc: 0,
    };
    let mut frame = 0usize;

    // Boot, then mash START/A to get through title + "CONTINUE".
    if let Some(f) = press(&mut gba, &[], 240, &mut w, frame) {
        panic!("crashed during boot at frame {}", f);
    }
    frame += 240;

    for _ in 0..12 {
        if let Some(f) = press(&mut gba, &[Key::Start], 6, &mut w, frame) {
            panic!("crashed on START at frame {}", f);
        }
        frame += 6;
        if let Some(f) = press(&mut gba, &[], 24, &mut w, frame) {
            panic!("crashed after START at frame {}", f);
        }
        frame += 24;
    }
    for _ in 0..10 {
        if let Some(f) = press(&mut gba, &[Key::A], 6, &mut w, frame) {
            panic!("crashed on A at frame {}", f);
        }
        frame += 6;
        if let Some(f) = press(&mut gba, &[], 20, &mut w, frame) {
            panic!("crashed after A at frame {}", f);
        }
        frame += 20;
    }

    println!(
        "after boot: frame {} pc=0x{:08X} [{}]",
        frame,
        gba.cpu.regs[15],
        region(gba.cpu.regs[15])
    );

    // Walk in grass: alternate up/down so we stay on the same patch.
    let steps: usize = std::env::var("WALK_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);

    for i in 0..steps {
        let dir = if (i / 2) % 2 == 0 { Key::Up } else { Key::Down };
        if let Some(f) = press(&mut gba, &[dir], 16, &mut w, frame) {
            let (_fr, pc, cpsr, prev) = w.worst.unwrap();
            let _ = gba.dump_frame_png("/tmp/grass/crash.png");
            panic!(
                "CRASH walking in grass at frame {} (step {}): \
                 pc=0x{:08X} [{}] prev_pc=0x{:08X} cpsr=0x{:08X} mode=0x{:02X}",
                f,
                i,
                pc,
                region(pc),
                prev,
                cpsr,
                cpsr & 0x1F
            );
        }
        frame += 16;

        {
            let _ = std::fs::create_dir_all("/tmp/grass");
            let _ = gba.dump_frame_png(&format!("/tmp/grass/step_{:03}.png", i));
        }
    }

    println!(
        "survived {} frames of grass walking; pc=0x{:08X} [{}]",
        frame,
        gba.cpu.regs[15],
        region(gba.cpu.regs[15])
    );
    println!("exception-mode frames seen: {}", w.und_frames.len());
}
