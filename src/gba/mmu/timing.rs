//! GBA bus timing: memory wait states (ROADMAP M1).
//!
//! Every CPU-visible memory access costs 1 cycle plus the wait states of the
//! region it touches. The CPU instruction handlers already count one cycle
//! per access (their S/N cycles), so the MMU only accumulates the *extra*
//! wait cycles, and `step_arm` / `step_thumb` add the per-instruction delta.
//!
//! Region costs (16-bit / 32-bit access, in total cycles):
//!
//! | Region            | 8/16-bit        | 32-bit              |
//! |-------------------|-----------------|---------------------|
//! | BIOS, IWRAM, IO   | 1               | 1                   |
//! | OAM               | 1               | 1                   |
//! | Palette, VRAM     | 1               | 2                   |
//! | EWRAM             | 3               | 6                   |
//! | ROM WS0/WS1/WS2   | 1+N or 1+S      | (1+N or 1+S) + 1+S  |
//! | SRAM              | 1+SRAM wait     | same (8-bit bus)    |
//!
//! ROM N/S wait states come from WAITCNT. An access is sequential (S) when
//! it directly follows the previous access's address; the first access after
//! a jump or a data access elsewhere is non-sequential (N).
//!
//! **Prefetch buffer** (WAITCNT bit 14): the cartridge keeps fetching the
//! following halfwords while the CPU is busy elsewhere. This is modelled as a
//! small buffer that fills by one halfword per S-wait period of cycles spent
//! off the ROM bus; a sequential code fetch served from the buffer costs 1
//! cycle. See `Prefetch`.

use std::cell::Cell;

/// ROM first-access (N) wait states for WAITCNT settings 0-3.
const N_WAITS: [u32; 4] = [4, 3, 2, 8];

#[derive(Clone, Copy, Debug)]
pub struct WaitTables {
    /// Total cycles for a non-sequential / sequential 16-bit access, by
    /// region (address bits 24-27).
    pub n16: [u32; 16],
    pub s16: [u32; 16],
    /// Total cycles for a 32-bit access.
    pub n32: [u32; 16],
    pub s32: [u32; 16],
    pub prefetch: bool,
}

impl WaitTables {
    /// Build the tables for a WAITCNT value.
    pub fn from_waitcnt(waitcnt: u16) -> Self {
        let mut t = WaitTables {
            n16: [1; 16],
            s16: [1; 16],
            n32: [1; 16],
            s32: [1; 16],
            prefetch: waitcnt & (1 << 14) != 0,
        };
        // EWRAM: 2 wait states on a 16-bit bus.
        t.n16[0x2] = 3;
        t.s16[0x2] = 3;
        t.n32[0x2] = 6;
        t.s32[0x2] = 6;
        // Palette and VRAM: 16-bit bus.
        for r in [0x5, 0x6] {
            t.n32[r] = 2;
            t.s32[r] = 2;
        }
        // Cartridge ROM wait-state regions: (N field, S field, S options).
        let rom = [
            (0x8, (waitcnt >> 2) & 3, (waitcnt >> 4) & 1, [2, 1]),
            (0xA, (waitcnt >> 5) & 3, (waitcnt >> 7) & 1, [4, 1]),
            (0xC, (waitcnt >> 8) & 3, (waitcnt >> 10) & 1, [8, 1]),
        ];
        for (base, n_sel, s_sel, s_opts) in rom {
            let n = 1 + N_WAITS[n_sel as usize];
            let s = 1 + s_opts[s_sel as usize];
            for r in [base, base + 1] {
                t.n16[r] = n;
                t.s16[r] = s;
                // A 32-bit access is two 16-bit accesses, the second
                // sequential.
                t.n32[r] = n + s;
                t.s32[r] = s + s;
            }
        }
        // SRAM: 8-bit bus, same cost for every width.
        let sram = 1 + N_WAITS[(waitcnt & 3) as usize];
        for r in [0xE, 0xF] {
            t.n16[r] = sram;
            t.s16[r] = sram;
            t.n32[r] = sram;
            t.s32[r] = sram;
        }
        t
    }
}

impl Default for WaitTables {
    fn default() -> Self {
        Self::from_waitcnt(0)
    }
}

/// Access width in bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Byte = 1,
    Half = 2,
    Word = 4,
}

/// Cartridge prefetch buffer model.
///
/// Holds up to 8 halfwords following the last ROM code fetch. It fills
/// while the ROM bus is idle (the CPU is running from or accessing other
/// memory, or doing internal cycles), one halfword per S-access period.
#[derive(Clone, Copy, Default)]
struct Prefetch {
    /// Address of the next halfword the buffer will hand to the CPU.
    head: u32,
    /// Halfwords ready in the buffer.
    count: u32,
    /// Cycles accumulated towards the next halfword.
    progress: u32,
}

const PREFETCH_CAPACITY: u32 = 8;

/// Mutable bus-timing state, kept in `Cell`s so the `&self` read paths can
/// update it.
#[derive(Default)]
pub struct BusTiming {
    pub tables: WaitTables,
    /// Extra (beyond 1 per access) wait cycles accumulated so far. The CPU
    /// reads the delta across each instruction.
    pub waits: Cell<u32>,
    /// Total cycles spent on bus accesses so far. The MMU uses it to know
    /// the current cycle in the middle of an instruction (see `Mmu::now`).
    pub clock: Cell<u64>,
    /// Address that would make the next access sequential.
    next_seq: Cell<u32>,
    prefetch: Cell<Prefetch>,
}

impl BusTiming {
    pub fn set_waitcnt(&mut self, waitcnt: u16) {
        self.tables = WaitTables::from_waitcnt(waitcnt);
        self.prefetch.set(Prefetch::default());
    }

    /// Account for one access and return its total cycle cost.
    #[inline(always)]
    pub fn access(&self, addr: u32, width: Width, code: bool) -> u32 {
        let region = if addr >= 0x1000_0000 { 0x1 } else { ((addr >> 24) & 0xF) as usize };
        let seq = addr == self.next_seq.get();
        self.next_seq.set(addr.wrapping_add(width as u32));

        let t = &self.tables;
        let is_rom = (0x8..=0xD).contains(&region);
        let cost = if is_rom && t.prefetch {
            self.rom_with_prefetch(addr, width, seq, code, region)
        } else {
            let cost = match (width, seq) {
                (Width::Word, false) => t.n32[region],
                (Width::Word, true) => t.s32[region],
                (_, false) => t.n16[region],
                (_, true) => t.s16[region],
            };
            if t.prefetch && !is_rom {
                // Off the ROM bus: the prefetcher keeps working.
                self.prefetch_idle(cost);
            }
            cost
        };
        self.waits.set(self.waits.get() + cost - 1);
        self.clock.set(self.clock.get() + cost as u64);
        cost
    }

    /// Cycles a DMA transfer of `count` units keeps the CPU off the bus:
    /// 2N + 2(n-1)S + 2I (GBATEK "DMA Transfer Timing"). Source and
    /// destination are separate sequential streams.
    pub fn dma_cost(&self, src: u32, dst: u32, word: bool, count: u32) -> u32 {
        let t = &self.tables;
        let region = |a: u32| if a >= 0x1000_0000 { 0x1 } else { ((a >> 24) & 0xF) as usize };
        let (rs, rd) = (region(src), region(dst));
        let (n, s) = if word { (&t.n32, &t.s32) } else { (&t.n16, &t.s16) };
        let first = n[rs] + n[rd];
        let rest = (s[rs] + s[rd]) * count.saturating_sub(1);
        first + rest + 2
    }

    /// Internal (I) cycles also let the prefetcher run.
    #[inline(always)]
    pub fn idle(&self, cycles: u32) {
        if self.tables.prefetch {
            self.prefetch_idle(cycles);
        }
    }

    fn prefetch_idle(&self, cycles: u32) {
        let mut pf = self.prefetch.get();
        if pf.count >= PREFETCH_CAPACITY || pf.head == 0 {
            return;
        }
        let region = ((pf.head >> 24) & 0xF) as usize;
        let s = self.tables.s16[region].max(1);
        pf.progress += cycles;
        let fetched = (pf.progress / s).min(PREFETCH_CAPACITY - pf.count);
        pf.count += fetched;
        pf.progress -= fetched * s;
        if pf.count >= PREFETCH_CAPACITY {
            pf.progress = 0;
        }
        self.prefetch.set(pf);
    }

    fn rom_with_prefetch(&self, addr: u32, width: Width, seq: bool, code: bool, region: usize) -> u32 {
        let t = &self.tables;
        let halves = if width == Width::Word { 2 } else { 1 };
        let mut pf = self.prefetch.get();
        if code && addr == pf.head && pf.count >= halves {
            // Served from the buffer.
            pf.count -= halves;
            pf.head = addr.wrapping_add(width as u32);
            self.prefetch.set(pf);
            return 1;
        }
        // Buffer miss (or data access): a real bus access, after which the
        // prefetcher restarts behind it (code) or is flushed (data).
        let cost = match (width, seq) {
            (Width::Word, false) => t.n32[region],
            (Width::Word, true) => t.s32[region],
            (_, false) => t.n16[region],
            (_, true) => t.s16[region],
        };
        pf = if code {
            Prefetch { head: addr.wrapping_add(width as u32), count: 0, progress: 0 }
        } else {
            Prefetch::default()
        };
        self.prefetch.set(pf);
        cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emerald_waitcnt_tables() {
        // Pokemon Emerald writes 0x4317: SRAM 8 waits, WS0 3/1, WS2 8/1?
        let t = WaitTables::from_waitcnt(0x4317);
        assert!(t.prefetch);
        assert_eq!(t.n16[0x8], 1 + 3);
        assert_eq!(t.s16[0x8], 1 + 1);
        assert_eq!(t.n32[0x8], 4 + 2);
    }

    #[test]
    fn reset_tables_match_gbatek() {
        let t = WaitTables::from_waitcnt(0);
        assert_eq!((t.n16[0x8], t.s16[0x8]), (5, 3));
        assert_eq!((t.n32[0x2], t.n16[0x3], t.n32[0x6]), (6, 1, 2));
    }

    #[test]
    fn sequential_rom_access_is_cheaper() {
        let b = BusTiming::default();
        assert_eq!(b.access(0x0800_0000, Width::Half, true), 5);
        assert_eq!(b.access(0x0800_0002, Width::Half, true), 3);
        assert_eq!(b.access(0x0300_0000, Width::Half, false), 1);
        // Back to ROM after a data access: non-sequential again.
        assert_eq!(b.access(0x0800_0004, Width::Half, true), 5);
    }

    #[test]
    fn dma_cost_matches_gbatek() {
        let b = BusTiming::default();
        // IWRAM -> IWRAM, 4 halfwords: 2N + 6S + 2I, all 1-cycle accesses.
        assert_eq!(b.dma_cost(0x0300_0000, 0x0300_0100, false, 4), 2 + 6 + 2);
        // EWRAM words cost 6 each.
        assert_eq!(b.dma_cost(0x0200_0000, 0x0300_0000, true, 1), 6 + 1 + 2);
    }

    #[test]
    fn prefetch_hides_rom_latency_after_idle() {
        let mut b = BusTiming::default();
        b.set_waitcnt(0x4000 | (1 << 4)); // prefetch on, WS0 S = 1 wait
        assert_eq!(b.access(0x0800_0000, Width::Half, true), 5); // miss
        b.idle(8); // 4 halfwords at 2 cycles each
        assert_eq!(b.access(0x0800_0002, Width::Half, true), 1); // hit
        assert_eq!(b.access(0x0800_0004, Width::Half, true), 1);
    }
}

/// Bus timing state (ROADMAP M2): sequential-access tracking and the
/// cartridge prefetch buffer. Wait tables are rebuilt from WAITCNT.
impl crate::gba::state::Snapshot for BusTiming {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        w.u32(self.waits.get()); w.u64(self.clock.get()); w.u32(self.next_seq.get());
        let pf = self.prefetch.get();
        w.u32(pf.head); w.u32(pf.count); w.u32(pf.progress);
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        self.waits.set(r.u32()?); self.clock.set(r.u64()?); self.next_seq.set(r.u32()?);
        self.prefetch.set(Prefetch { head: r.u32()?, count: r.u32()?.min(PREFETCH_CAPACITY), progress: r.u32()? });
        Some(())
    }
}
