//! Differential test: the NDS instruction executor (ARM7 side, ARMv4T) must
//! match the GBA core's ARM7TDMI implementation, which is validated against
//! jsmolka's gba-tests. Random register-only instructions (ALU, shifts,
//! multiplies, flag ops) are run on both and the resulting registers and
//! CPSR compared.

use gba_simulator::gba::cpu::{arm::step_arm, thumb::step_thumb, Arm7Tdmi as GbaCpu};
use gba_simulator::gba::mmu::Mmu;
use gba_simulator::nds::cpu::step_arm7;
use gba_simulator::nds::Nds;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

const GBA_BASE: u32 = 0x0300_0000;
const NDS_BASE: u32 = 0x0200_0000;

/// Run one instruction on both cores from identical state. Returns a
/// mismatch description, or None.
fn compare(instr: u32, thumb: bool, regs: &[u32; 16], cpsr: u32) -> Option<String> {
    // GBA reference
    let mut mmu = Mmu::new();
    let mut g = GbaCpu::new();
    g.set_cpsr((cpsr & !0x3F) | 0x1F | if thumb { 0x20 } else { 0 });
    if thumb {
        mmu.iwram[..2].copy_from_slice(&(instr as u16).to_le_bytes());
    } else {
        mmu.iwram[..4].copy_from_slice(&instr.to_le_bytes());
    }
    g.regs[..15].copy_from_slice(&regs[..15]);
    g.regs[15] = GBA_BASE;
    g.pipe_valid = false;
    if thumb {
        step_thumb(&mut g, &mut mmu);
    } else {
        step_arm(&mut g, &mut mmu);
    }

    // NDS ARM7
    let mut n = Nds::new();
    let c = &mut n.arm7;
    c.set_cpsr((cpsr & !0x3F) | 0x1F | if thumb { 0x20 } else { 0 });
    if thumb {
        n.bus.write_arm7_u16(NDS_BASE, instr as u16);
    } else {
        n.bus.write_arm7_u32(NDS_BASE, instr);
    }
    let c = &mut n.arm7;
    c.regs[..15].copy_from_slice(&regs[..15]);
    c.regs[15] = NDS_BASE;
    step_arm7(&mut n.arm7, &mut n.bus);

    let mut diffs = Vec::new();
    for r in 0..15 {
        if g.regs[r] != n.arm7.regs[r] {
            diffs.push(format!("r{r}: gba={:08X} nds={:08X}", g.regs[r], n.arm7.regs[r]));
        }
    }
    let gpc = g.regs[15].wrapping_sub(GBA_BASE);
    let npc = n.arm7.regs[15].wrapping_sub(NDS_BASE);
    if gpc != npc {
        diffs.push(format!("pc offset: gba={gpc:X} nds={npc:X}"));
    }
    // Compare NZCV + T + I; mode bits too.
    let mask = 0xF000_00FF;
    if g.cpsr & mask != n.arm7.cpsr & mask {
        diffs.push(format!("cpsr: gba={:08X} nds={:08X}", g.cpsr, n.arm7.cpsr));
    }
    (!diffs.is_empty()).then(|| diffs.join(", "))
}

fn random_regs(rng: &mut Rng) -> [u32; 16] {
    let mut r = [0u32; 16];
    for v in r.iter_mut() {
        *v = match rng.next() % 4 {
            0 => rng.next() & 0x1F,
            1 => rng.next(),
            2 => 0x8000_0000 | (rng.next() & 3),
            _ => rng.next() & 0xFF,
        };
    }
    r
}

/// Thumb formats that only touch registers: 1-5 (minus hi-reg BX/PC ops).
fn random_thumb(rng: &mut Rng) -> u32 {
    loop {
        let i = rng.next() & 0xFFFF;
        let ok = match i >> 13 {
            0 | 1 => true, // formats 1-3
            2 => {
                // 010000 = ALU ops (format 4); 010001 = hi-reg ops (format 5)
                if i & 0xFC00 == 0x4000 {
                    true
                } else if i & 0xFC00 == 0x4400 {
                    let op = (i >> 8) & 3;
                    let rd = (i & 7) | ((i >> 4) & 8);
                    let rs = (i >> 3) & 0xF;
                    op != 3 && rd != 15 && rs != 15 // no BX, no PC
                } else {
                    false
                }
            }
            _ => false,
        };
        if ok {
            return i;
        }
    }
}

/// ARM data processing / multiply with register operands only, cond=AL,
/// never writing r15 and never reading it.
fn random_arm(rng: &mut Rng) -> u32 {
    loop {
        let mut i = rng.next();
        i = (i & 0x0FFF_FFFF) | 0xE000_0000;
        let rd = (i >> 12) & 0xF;
        let rn = (i >> 16) & 0xF;
        let rm = i & 0xF;
        let rs = (i >> 8) & 0xF;
        if (i & 0x0FC0_00F0) == 0x0000_0090 || (i & 0x0F80_00F0) == 0x0080_0090 {
            // MUL/MLA/UMULL/...: avoid r15 anywhere
            if [rd, rn, rm, rs].contains(&15) {
                continue;
            }
            return i;
        }
        if (i & 0x0C00_0000) != 0 {
            continue; // not data processing
        }
        let imm = i & (1 << 25) != 0;
        let reg_shift = !imm && (i & 0x10) != 0;
        if reg_shift && (i & 0x80) != 0 {
            continue; // multiply/extra-load space
        }
        let op = (i >> 21) & 0xF;
        let s = i & (1 << 20) != 0;
        if (8..=11).contains(&op) && !s {
            continue; // MRS/MSR space
        }
        if rd == 15 || rn == 15 || (!imm && rm == 15) || (reg_shift && rs == 15) {
            continue;
        }
        return i;
    }
}

fn run(thumb: bool, count: usize, seed: u64) {
    let mut rng = Rng(seed);
    let mut failures = Vec::new();
    for _ in 0..count {
        let instr = if thumb { random_thumb(&mut rng) } else { random_arm(&mut rng) };
        let regs = random_regs(&mut rng);
        let cpsr = rng.next() & 0xF000_0000;
        if let Some(d) = compare(instr, thumb, &regs, cpsr) {
            failures.push(format!("{}{:08X}: {d}", if thumb { "T " } else { "A " }, instr));
        }
    }
    failures.sort();
    failures.dedup_by(|a, b| a[..6] == b[..6]);
    assert!(
        failures.is_empty(),
        "{} mismatches, first few:\n{}",
        failures.len(),
        failures.iter().take(25).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn thumb_register_ops_match_gba_core() {
    run(true, 3_000, 0x1234_5678_9ABC_DEF1);
}

#[test]
fn arm_register_ops_match_gba_core() {
    run(false, 3_000, 0x0F0E_0D0C_0B0A_0908);
}
