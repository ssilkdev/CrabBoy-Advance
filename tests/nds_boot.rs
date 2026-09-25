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
    // 2. Wait until the receive FIFO is non-empty (IPCFIFOCNT bit 8 clear)
    // 3. Read IPCFIFORECV (0x04100000) — GBATEK: 0x04000188 is send-only
    // 4. Write response 0x4E445337 to IPCFIFOSEND (0x04000188)
    // 5. B .
    let arm7_words: [u32; 15] = [
        0xE59F0024, // 00 LDR R0, [PC, #36] -> 0x04000184 (0x2C)
        0xE59F1024, // 04 LDR R1, [PC, #36] -> 0x8000     (0x30)
        0xE1C010B0, // 08 STRH R1, [R0]
        0xE1D040B0, // 0C wait: LDRH R4, [R0]
        0xE3140C01, // 10 TST R4, #0x100 (receive empty)
        0x1AFFFFFC, // 14 BNE wait
        0xE59F5018, // 18 LDR R5, [PC, #24] -> 0x04100000 (0x38)
        0xE5952000, // 1C LDR R2, [R5]
        0xE59F300C, // 20 LDR R3, [PC, #12] -> "NDS7"     (0x34)
        0xE5803004, // 24 STR R3, [R0, #4]  -> IPCFIFOSEND
        0xEAFFFFFE, // 28 B .
        0x04000184, // 2C Literal: IPCFIFOCNT
        0x00008000, // 30 Literal: Enable flag
        0x4E445337, // 34 Literal: "NDS7" token
        0x04100000, // 38 Literal: IPCFIFORECV
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
