//! Does save_state/load_state actually round-trip the whole machine?

use gba_simulator::gba::Gba;

#[test]
#[ignore]
fn save_state_roundtrip_preserves_hardware_state() {
    let Ok(rom) = std::env::var("GBA_TEST_ROM") else {
        eprintln!("skipping: set GBA_TEST_ROM");
        return;
    };
    let mut gba = Gba::new();
    gba.load_rom_bytes(std::fs::read(&rom).unwrap());

    // Run far enough that the game has set up IRQs, DMA and timers.
    for _ in 0..600 {
        gba.run_frame();
    }

    let before_ime = gba.mmu.ime;
    let before_ie = gba.mmu.ie;
    let before_if = gba.mmu.if_reg;
    let before_handler = gba.mmu.read32(0x0300_7FFC);
    let before_dma_sad: Vec<u32> = gba.mmu.dma.channels.iter().map(|c| c.sad).collect();
    let before_dma_cnt: Vec<u16> = gba.mmu.dma.channels.iter().map(|c| c.cnt_h).collect();
    let before_tm: Vec<u16> = gba.mmu.timers.timers.iter().map(|t| t.counter).collect();

    let blob = gba.save_state();
    let mut restored = Gba::new();
    restored.load_rom_bytes(std::fs::read(&rom).unwrap());
    assert!(restored.load_state(&blob));

    println!("            before -> after");
    println!("IME       : {:5} -> {:5}", before_ime, restored.mmu.ime);
    println!("IE        : 0x{:04X} -> 0x{:04X}", before_ie, restored.mmu.ie);
    println!("IF        : 0x{:04X} -> 0x{:04X}", before_if, restored.mmu.if_reg);
    println!(
        "handler   : 0x{:08X} -> 0x{:08X}",
        before_handler,
        restored.mmu.read32(0x0300_7FFC)
    );
    for i in 0..4 {
        println!(
            "DMA{} sad  : 0x{:08X} -> 0x{:08X}   cnt_h 0x{:04X} -> 0x{:04X}",
            i,
            before_dma_sad[i],
            restored.mmu.dma.channels[i].sad,
            before_dma_cnt[i],
            restored.mmu.dma.channels[i].cnt_h
        );
    }
    for i in 0..4 {
        println!(
            "TM{} count : {:5} -> {:5}",
            i, before_tm[i], restored.mmu.timers.timers[i].counter
        );
    }

    let mut lost = Vec::new();
    if restored.mmu.ime != before_ime {
        lost.push("IME");
    }
    if restored.mmu.ie != before_ie {
        lost.push("IE");
    }
    if (0..4).any(|i| restored.mmu.dma.channels[i].sad != before_dma_sad[i]) {
        lost.push("DMA source addresses");
    }
    if (0..4).any(|i| restored.mmu.dma.channels[i].cnt_h != before_dma_cnt[i]) {
        lost.push("DMA control");
    }

    assert!(
        lost.is_empty(),
        "save_state() silently drops: {}",
        lost.join(", ")
    );
}

/// The actual user-visible bug: save mid-game, reload, keep playing.
/// With v1 states the restored machine had IME=false and no DMA, so the
/// next interrupt sent PC into IO space and the game locked up.
#[test]
#[ignore]
fn restored_state_keeps_executing_normally() {
    let Ok(rom) = std::env::var("GBA_TEST_ROM") else {
        eprintln!("skipping: set GBA_TEST_ROM");
        return;
    };
    let rom_bytes = std::fs::read(&rom).unwrap();

    let mut gba = Gba::new();
    gba.load_rom_bytes(rom_bytes.clone());
    for _ in 0..600 {
        gba.run_frame();
    }

    let blob = gba.save_state();

    // Baseline: the un-saved machine keeps running fine.
    for _ in 0..300 {
        gba.run_frame();
    }
    let live_pc = gba.cpu.regs[15];

    let mut restored = Gba::new();
    restored.load_rom_bytes(rom_bytes);
    assert!(restored.load_state(&blob));

    for i in 0..300 {
        restored.run_frame();
        let pc = restored.cpu.regs[15];
        assert!(
            !(0x0400_0000..0x0500_0000).contains(&pc),
            "frame {} after restore: PC ran into IO space (0x{:08X}) -- \
             this is the grass-battle crash",
            i,
            pc
        );
    }

    println!("live      PC after 300 more frames: 0x{:08X}", live_pc);
    println!("restored  PC after 300 more frames: 0x{:08X}", restored.cpu.regs[15]);
    println!("IME={} IE=0x{:04X}", restored.mmu.ime, restored.mmu.ie);
}
