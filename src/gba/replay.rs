//! Deterministic movie replay (ROADMAP M2).
//!
//! A tiny self-contained GBA test program (hand-assembled ARM, built in
//! memory so no ROM file is needed) plus a scripted input movie in the TAS
//! engine's text format. Replaying it produces a hash per frame and one for
//! the audio. `tests/replay.rs` compares those against a second run, a
//! save/load split run and golden hashes; `replay_report` prints the same
//! summary so any platform (Windows CI, an Android device via adb) can be
//! checked against the same golden file.
//!
//! The program waits for VBlank through the BIOS (IRQ + IntrWait), reads
//! KEYINPUT, moves a pixel around a mode-3 screen leaving a trail coloured by
//! timer 0, plays a PSG square wave whose pitch follows the position, and
//! clears the screen with a DMA3 fill while B is held -- so the hashes cover
//! CPU, BIOS HLE, IRQs, timers, DMA, PPU and APU.

use crate::gba::keypad::Key;
use crate::gba::Gba;

// ---- A tiny ARM assembler for the handful of encodings used ------------

/// cond=AL data-processing immediate: op Rd, Rn, #imm8 ror (2*rot).
fn dp_imm(op: u32, s: bool, rn: u32, rd: u32, rot: u32, imm8: u32) -> u32 {
    0xE200_0000 | (op << 21) | ((s as u32) << 20) | (rn << 16) | (rd << 12) | (rot << 8) | imm8
}
const AND: u32 = 0x0;
const SUB: u32 = 0x2;
const ADD: u32 = 0x4;
const TST: u32 = 0x8;
const ORR: u32 = 0xC;
const MOV: u32 = 0xD;
/// strh Rd, [Rn, #off]  (off < 256)
fn strh(rd: u32, rn: u32, off: u32) -> u32 {
    0xE1C0_00B0 | (rn << 16) | (rd << 12) | ((off >> 4) << 8) | (off & 0xF)
}
/// ldrh Rd, [Rn, #off]
fn ldrh(rd: u32, rn: u32, off: u32) -> u32 {
    0xE1D0_00B0 | (rn << 16) | (rd << 12) | ((off >> 4) << 8) | (off & 0xF)
}
/// str Rd, [Rn, #off] (off < 4096)
fn str_(rd: u32, rn: u32, off: u32) -> u32 {
    0xE580_0000 | (rn << 16) | (rd << 12) | off
}
/// Rd = Rn + Rm << sh
fn add_lsl(rd: u32, rn: u32, rm: u32, sh: u32) -> u32 {
    0xE080_0000 | (rn << 16) | (rd << 12) | (sh << 7) | rm
}
/// Conditional branch to `target` from the instruction at `at` (word
/// indices into the program).
fn b(cond: u32, at: usize, target: usize) -> u32 {
    let off = (target as i64 - at as i64 - 2) as i32;
    (cond << 28) | 0x0A00_0000 | (off as u32 & 0x00FF_FFFF)
}
const EQ: u32 = 0x0;
const NE: u32 = 0x1;
const AL: u32 = 0xE;

/// Rotated immediate: `imm8 ror (2*rot)`. Panics if `value` isn't
/// encodable, so a typo can't silently produce a different constant.
fn imm(value: u32) -> (u32, u32) {
    for rot in 0..16 {
        let v = value.rotate_left(2 * rot);
        if v <= 0xFF {
            return (rot, v);
        }
    }
    panic!("{value:#x} is not an ARM immediate");
}
fn op_imm(op: u32, rd: u32, rn: u32, value: u32) -> u32 {
    let (rot, i8) = imm(value);
    dp_imm(op, false, rn, rd, rot, i8)
}
fn tst_imm(rn: u32, value: u32) -> u32 {
    let (rot, i8) = imm(value);
    dp_imm(TST, true, rn, 0, rot, i8)
}
/// Same instruction, executed only if Z is set (key pressed: KEYINPUT bits
/// are 0 when pressed, so `tst` leaves Z=1).
fn if_eq(w: u32) -> u32 {
    (w & 0x0FFF_FFFF) | (EQ << 28)
}

/// The test program. Registers: r0 IO base, r1 VRAM base, r2 x, r3 y,
/// r4/r7/r8 scratch, r5 keys, r6 colour, r9 IO+0x100 (timers/keys).
fn program() -> Vec<u32> {
    let mut p: Vec<u32> = Vec::new();

    // [0] jump over the IRQ handler.
    p.push(0); // patched below
    // [1..] IRQ handler, called by the BIOS dispatcher with lr set:
    // acknowledge every pending IRQ (IF = IF) and return. The HLE BIOS
    // mirrors IF into 0x03007FF8 itself, which is what IntrWait checks.
    let handler = p.len();
    p.push(op_imm(MOV, 3, 0, 0x0400_0000));       // mov r3, #0x04000000
    p.push(op_imm(ADD, 3, 3, 0x200));            // r3 = 0x04000200
    p.push(ldrh(2, 3, 0x02));                    // r2 = IF
    p.push(strh(2, 3, 0x02));                    // IF = r2 (ack)
    p.push(0xE12F_FF1E);                         // bx lr
    let setup = p.len();
    p[0] = b(AL, 0, setup);

    // --- setup ---
    // Install the handler: [0x03007FFC] = its ROM address. `pc` reads as
    // this instruction + 8.
    let here = p.len();
    p.push(op_imm(SUB, 4, 15, ((here - handler + 2) * 4) as u32)); // r4 = &handler
    p.push(op_imm(MOV, 7, 0, 0x0300_0000));
    p.push(op_imm(ADD, 7, 7, 0x7F00));
    p.push(str_(4, 7, 0xFC));                    // [0x03007FFC] = r4

    p.push(op_imm(MOV, 0, 0, 0x0400_0000));       // r0 = IO
    p.push(op_imm(ADD, 9, 0, 0x100));             // r9 = IO + 0x100
    p.push(op_imm(MOV, 4, 0, 0x400));             // DISPCNT = BG2 ...
    p.push(op_imm(ORR, 4, 4, 0x03));              //   | mode 3
    p.push(strh(4, 0, 0x00));
    p.push(op_imm(MOV, 4, 0, 0x08));              // DISPSTAT: VBlank IRQ
    p.push(strh(4, 0, 0x04));
    // Sound: master on, PSG ch2 on both sides at full volume.
    p.push(op_imm(MOV, 4, 0, 0x80));
    p.push(strh(4, 0, 0x84));                    // SOUNDCNT_X
    p.push(op_imm(MOV, 4, 0, 0x2277 & 0xFF00));
    p.push(op_imm(ORR, 4, 4, 0x77));
    p.push(strh(4, 0, 0x80));                    // SOUNDCNT_L = 0x2277
    p.push(op_imm(MOV, 4, 0, 0x02));
    p.push(strh(4, 0, 0x82));                    // SOUNDCNT_H: PSG 100%
    p.push(op_imm(MOV, 4, 0, 0xF000));
    p.push(op_imm(ORR, 4, 4, 0x80));             // vol 15, duty 50%
    p.push(strh(4, 0, 0x68));                    // SOUND2CNT_L
    // Timer 0 running at /64 (tints the trail).
    p.push(op_imm(MOV, 4, 0, 0x81));
    p.push(strh(4, 9, 0x02));                    // TM0CNT_H
    // IE = VBlank, IME = 1.
    p.push(op_imm(ADD, 7, 0, 0x200));
    p.push(op_imm(MOV, 4, 0, 1));
    p.push(strh(4, 7, 0x00));
    p.push(strh(4, 7, 0x08));
    p.push(op_imm(MOV, 1, 0, 0x0600_0000));       // r1 = VRAM
    p.push(op_imm(MOV, 2, 0, 60));                // x
    p.push(op_imm(MOV, 3, 0, 60));                // y

    // --- main loop ---
    let top = p.len();
    p.push(0xEF05_0000);                         // swi VBlankIntrWait
    p.push(ldrh(5, 9, 0x30));                    // r5 = KEYINPUT
    p.push(tst_imm(5, 0x10)); p.push(if_eq(op_imm(ADD, 2, 2, 1))); // Right
    p.push(tst_imm(5, 0x20)); p.push(if_eq(op_imm(SUB, 2, 2, 1))); // Left
    p.push(tst_imm(5, 0x80)); p.push(if_eq(op_imm(ADD, 3, 3, 1))); // Down
    p.push(tst_imm(5, 0x40)); p.push(if_eq(op_imm(SUB, 3, 3, 1))); // Up
    p.push(op_imm(AND, 2, 2, 0x7F));             // keep x, y in 0..127
    p.push(op_imm(AND, 3, 3, 0x7F));
    // colour = TM0 counter | (A held ? 0x7C00 : 0)
    p.push(ldrh(6, 9, 0x00));
    p.push(tst_imm(5, 0x01)); p.push(if_eq(op_imm(ORR, 6, 6, 0x7C00)));
    // VRAM address = r1 + y*480 + x*2 (y*480 = y*512 - y*32)
    p.push(add_lsl(7, 1, 3, 9));
    p.push(0xE047_7283);                         // sub r7, r7, r3, lsl #5
    p.push(add_lsl(7, 7, 2, 1));
    p.push(strh(6, 7, 0));
    // Tone follows x: SOUND2CNT_H = 0x8000 | (x << 3), restarted per frame.
    p.push(0xE1A0_8182);                         // mov r8, r2, lsl #3
    p.push(op_imm(ORR, 8, 8, 0x8000));
    p.push(strh(8, 0, 0x6C));
    // B held: clear the screen with a DMA3 32-bit fill from a zero word.
    p.push(tst_imm(5, 0x02));
    let skip = p.len();
    p.push(0); // bne after (patched)
    p.push(op_imm(MOV, 4, 0, 0));
    p.push(op_imm(MOV, 7, 0, 0x0300_0000));
    p.push(str_(4, 7, 0));                       // [0x03000000] = 0
    p.push(op_imm(ADD, 8, 0, 0xD4));             // r8 = DMA3 regs
    p.push(str_(7, 8, 0));                       // DMA3SAD = 0x03000000
    p.push(str_(1, 8, 4));                       // DMA3DAD = VRAM
    p.push(op_imm(MOV, 4, 0, 0x4B00));           // 19200 words
    p.push(op_imm(ORR, 4, 4, 0x8500_0000));      // enable, 32-bit, src fixed
    p.push(str_(4, 8, 8));                       // DMA3CNT
    let after = p.len();
    p[skip] = b(NE, skip, after);
    let here = p.len();
    p.push(b(AL, here, top));
    p
}

pub fn build_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x200];
    // Entry: b 0xC0
    rom[0..4].copy_from_slice(&0xEA00_002Eu32.to_le_bytes());
    rom[0xA0..0xAC].copy_from_slice(b"CRABREPLAY\0\0");
    rom[0xB2] = 0x96;
    let chk = rom[0xA0..0xBD].iter().fold(0u8, |c, &b| c.wrapping_sub(b)).wrapping_sub(0x19);
    rom[0xBD] = chk;
    let code = program();
    let mut out = rom;
    out.truncate(0xC0);
    for w in code {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.resize(0x1000, 0);
    out
}

// ---- Movie -----------------------------------------------------------------

/// Movie in the TAS engine's text format: `NNNNNN: ABsSRLUDrl` flags.
pub fn movie() -> String {
    let mut out = String::from("# CrabBoy replay test movie\n");
    for f in 0..900u32 {
        let mut k = [b'0'; 10];
        let phase = (f / 60) % 6;
        match phase {
            0 => k[4] = b'1',             // Right
            1 => { k[7] = b'1'; k[0] = b'1' } // Down + A
            2 => k[5] = b'1',             // Left
            3 => k[6] = b'1',             // Up
            4 => { k[4] = b'1'; k[7] = b'1' } // diagonal
            _ => if f % 7 == 0 { k[1] = b'1' } // B taps
        }
        out.push_str(&format!("{:06}: {}\n", f + 1, std::str::from_utf8(&k).unwrap()));
    }
    out
}

pub fn parse_movie(text: &str) -> Vec<[bool; 10]> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#') && l.contains(':'))
        .map(|l| {
            let keys = l.split_once(':').unwrap().1.trim();
            let mut f = [false; 10];
            for (i, c) in keys.chars().take(10).enumerate() {
                f[i] = c == '1';
            }
            f
        })
        .collect()
}

const KEYS: [Key; 10] = [
    Key::A, Key::B, Key::Select, Key::Start, Key::Right,
    Key::Left, Key::Up, Key::Down, Key::R, Key::L,
];

pub fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub fn boot() -> Gba {
    let mut gba = Gba::new_headless();
    gba.load_rom_bytes(build_rom());
    gba.set_deterministic_clock(Some(1_788_000_000));
    gba.mmu.apu.capture = Some(Vec::new());
    gba
}

/// Play `frames` of the movie starting at movie frame `start`.
pub fn play(gba: &mut Gba, movie: &[[bool; 10]], start: usize, frames: usize) -> (Vec<u64>, u64) {
    let mut hashes = Vec::new();
    let mut audio = Vec::new();
    for f in start..start + frames {
        for (i, k) in KEYS.iter().enumerate() {
            gba.mmu.keypad.set_key_state(*k, movie[f][i]);
        }
        gba.run_frame();
        gba.mmu.apu.flush_samples();
        audio.append(gba.mmu.apu.capture.get_or_insert_with(Vec::new));
        hashes.push(fnv(gba.get_framebuffer().iter().flat_map(|p| p.to_le_bytes())));
    }
    (hashes, fnv(audio.iter().flat_map(|s| s.to_bits().to_le_bytes())))
}


/// The golden-file text for a full replay: one frame hash every 60 frames
/// plus the audio hash. `tests/data/replay_golden.txt` holds the expected
/// output.
pub fn replay_report() -> String {
    let mv = parse_movie(&movie());
    let (v, a) = play(&mut boot(), &mv, 0, mv.len());
    report_text(&v, a)
}

/// Format per-frame hashes and the audio hash as the golden-file text.
pub fn report_text(video: &[u64], audio: u64) -> String {
    let mut text = String::new();
    for (i, h) in video.iter().enumerate().filter(|(i, _)| i % 60 == 59) {
        text.push_str(&format!("frame {:04} {:016x}\n", i + 1, h));
    }
    text.push_str(&format!("audio {:016x}\n", audio));
    text
}
