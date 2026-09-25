//! Integration tests for Nintendo DS Boot, CPU execution, IPC FIFO, and frame stepping.

use gba_simulator::nds::Nds;
use std::io::Write;

fn build_nds_test_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];

    // Header (512 bytes)
    rom[0x00..0x08].copy_from_slice(b"NDS TEST");
    rom[0x0C..0x10].copy_from_slice(b"NTST");
    rom[0x10..0x12].copy_from_slice(b"01");

    // ARM9 ROM offset: 0x4000, entry: 0x0200_0000, ram: 0x0200_0000
    rom[0x20..0x24].copy_from_slice(&0x4000u32.to_le_bytes());
    rom[0x24..0x28].copy_from_slice(&0x0200_0000u32.to_le_bytes());
    rom[0x28..0x2C].copy_from_slice(&0x0200_0000u32.to_le_bytes());
    rom[0x2C..0x30].copy_from_slice(&0x100u32.to_le_bytes()); // ARM9 size

    // ARM7 ROM offset: 0x5000, entry: 0x0380_0000, ram: 0x0380_0000
    rom[0x30..0x34].copy_from_slice(&0x5000u32.to_le_bytes());
    rom[0x34..0x38].copy_from_slice(&0x0380_0000u32.to_le_bytes());
    rom[0x38..0x3C].copy_from_slice(&0x0380_0000u32.to_le_bytes());
    rom[0x3C..0x40].copy_from_slice(&0x100u32.to_le_bytes()); // ARM7 size

    // ARM9 Code at 0x4000:
    // 1. MOV R0, #0x04000000 (IO base)  -> 0xE3A00404
    // 2. MOV R1, #0x8000 (FIFO enable)  -> 0xE3A01802 (rotated: 2 ror 16 = 0x20000 -> 0x8000: imm 0x80, rot 16 -> 0xE3A01880)
    //    Or LDR R1, [PC, #offset]
    // Let's use simple instructions:
    // LDR R0, =0x04000184
    // LDR R1, =0x8000
    // STRH R1, [R0]
    // LDR R2, =0x4E445339
    // STR R2, [R0, #4] (store to 0x04000188 IPCFIFOSEND)
    // B .
    let arm9_words: [u32; 10] = [
        0xE59F0010, // LDR R0, [PC, #16] -> loads 0x04000184 (index 6 at 0x18; PC is 0x08)
        0xE59F1010, // LDR R1, [PC, #16] -> loads 0x8000 (index 7 at 0x1C; PC is 0x0C)
        0xE1C010B0, // STRH R1, [R0]
        0xE59F200C, // LDR R2, [PC, #12] -> loads 0x4E445339 (index 8 at 0x20; PC is 0x14)
        0xE5802004, // STR R2, [R0, #4]  -> writes to 0x04000188
        0xEAFFFFFE, // B .
        0x04000184, // Literal: IPCFIFOCNT
        0x00008000, // Literal: Enable flag
        0x4E445339, // Literal: "NDS9" token
        0x00000000,
    ];

    for (i, &w) in arm9_words.iter().enumerate() {
        let off = 0x4000 + i * 4;
        rom[off..off + 4].copy_from_slice(&w.to_le_bytes());
    }

    // ARM7 Code at 0x5000:
    // 1. Enable FIFO
    // 2. Read from IPCFIFODATA (0x04000188)
    // 3. Write response 0x4E445337 to IPCFIFODATA (0x04000188)
    // 4. B .
    let arm7_words: [u32; 12] = [
        0xE59F0018, // LDR R0, [PC, #24] -> loads 0x04000184 (index 8 at 0x20; PC is 0x08)
        0xE59F1018, // LDR R1, [PC, #24] -> loads 0x8000 (index 9 at 0x24; PC is 0x0C)
        0xE1C010B0, // STRH R1, [R0]
        0xE5902004, // LDR R2, [R0, #4]  -> reads from 0x04000188
        0xE59F3010, // LDR R3, [PC, #16] -> loads 0x4E445337 ("NDS7", index 10 at 0x28; PC is 0x18)
        0xE5803004, // STR R3, [R0, #4]  -> writes response to 0x04000188
        0xEAFFFFFE, // B .
        0x00000000,
        0x04000184, // Literal: IPCFIFOCNT
        0x00008000, // Literal: Enable flag
        0x4E445337, // Literal: "NDS7" token
        0x00000000,
    ];

    for (i, &w) in arm7_words.iter().enumerate() {
        let off = 0x5000 + i * 4;
        rom[off..off + 4].copy_from_slice(&w.to_le_bytes());
    }

    rom
}

#[test]
fn test_nds_boot_and_ipc_communication() {
    let rom_data = build_nds_test_rom();

    // Write temp file to load via load_rom
    let temp_dir = std::env::temp_dir();
    let temp_rom = temp_dir.join("test_boot.nds");
    {
        let mut file = std::fs::File::create(&temp_rom).expect("create temp rom");
        file.write_all(&rom_data).expect("write temp rom");
    }

    let mut nds = Nds::new();
    nds.load_rom(&temp_rom).expect("load rom failed");
    let _ = std::fs::remove_file(&temp_rom);

    assert_eq!(nds.title(), "NDS TEST");
    assert_eq!(nds.game_code(), "NTST");
    assert_eq!(nds.arm9.regs[15], 0x0200_0000);
    assert_eq!(nds.arm7.regs[15], 0x0380_0000);

    // Run 1 frame (~263 scanlines, ~1.12M ARM9 cycles)
    nds.run_frame();
    assert_eq!(nds.frame_counter, 1);

    // ARM7 should have read the token 0x4E445339 sent by ARM9 into R2
    assert_eq!(nds.arm7.regs[2], 0x4E445339);

    // ARM7 sent back 0x4E445337 to ARM9 via FIFO; verify it is in FIFO queue
    assert_eq!(nds.bus.ipc.fifo.read_data_arm9(), 0x4E445337);
}
