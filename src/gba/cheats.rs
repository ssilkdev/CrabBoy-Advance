//! GameShark / Action Replay / CodeBreaker Cheat Engine & RAM Searcher

use super::mmu::Mmu;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheatOp {
    Write8(u32, u8),
    Write16(u32, u16),
    Write32(u32, u32),
}

#[derive(Debug, Clone)]
pub struct CheatEntry {
    pub name: String,
    pub code: String,
    pub enabled: bool,
    pub ops: Vec<CheatOp>,
}

impl CheatEntry {
    pub fn new(name: impl Into<String>, code: impl Into<String>) -> Self {
        let code_str = code.into();
        let ops = parse_cheat_code(&code_str);
        Self {
            name: name.into(),
            code: code_str,
            enabled: true,
            ops,
        }
    }
}

/// Parses multi-line cheat strings supporting:
/// 1. Action Replay / GameShark GBA: `XXXXXXXX YYYYYYYY`
/// 2. CodeBreaker: `8XXXXXXX YYYY` (16-bit write), `3XXXXXXX 00YY` (8-bit write)
/// 3. Raw Hex writes: `0202402C:270F` or `0x0202402C = 9999`
pub fn parse_cheat_code(text: &str) -> Vec<CheatOp> {
    let mut ops = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        // Format 1: Raw colon `0202402C:270F` or `0202402C:FF`
        if let Some((addr_str, val_str)) = line.split_once(':') {
            let addr_str = addr_str.trim().trim_start_matches("0x");
            let val_str = val_str.trim().trim_start_matches("0x");
            if let (Ok(addr), Ok(val)) = (u32::from_str_radix(addr_str, 16), u32::from_str_radix(val_str, 16)) {
                if val_str.len() <= 2 {
                    ops.push(CheatOp::Write8(addr, val as u8));
                } else if val_str.len() <= 4 {
                    ops.push(CheatOp::Write16(addr, val as u16));
                } else {
                    ops.push(CheatOp::Write32(addr, val));
                }
                continue;
            }
        }

        // Format 2: `XXXXXXXX YYYYYYYY` or `XXXXXXXX YYYY`
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() == 2 {
            let p0 = parts[0].trim_start_matches("0x");
            let p1 = parts[1].trim_start_matches("0x");
            if let (Ok(w0), Ok(w1)) = (u32::from_str_radix(p0, 16), u32::from_str_radix(p1, 16)) {
                // CodeBreaker 16-bit: `8XXXXXXX YYYY`
                if (w0 >> 28) == 0x8 {
                    let addr = 0x0200_0000 | (w0 & 0x01FF_FFFF);
                    ops.push(CheatOp::Write16(addr, w1 as u16));
                }
                // CodeBreaker 8-bit: `3XXXXXXX 00YY`
                else if (w0 >> 28) == 0x3 {
                    let addr = 0x0200_0000 | (w0 & 0x01FF_FFFF);
                    ops.push(CheatOp::Write8(addr, (w1 & 0xFF) as u8));
                }
                // GameShark / Action Replay standard EWRAM / IWRAM writes
                else if (w0 >> 24) == 0x02 || (w0 >> 24) == 0x03 {
                    if p1.len() <= 2 {
                        ops.push(CheatOp::Write8(w0, w1 as u8));
                    } else if p1.len() <= 4 {
                        ops.push(CheatOp::Write16(w0, w1 as u16));
                    } else {
                        ops.push(CheatOp::Write32(w0, w1));
                    }
                } else {
                    // Default to 16-bit EWRAM write if within 24-bit range
                    let addr = 0x0200_0000 | (w0 & 0x00FF_FFFF);
                    ops.push(CheatOp::Write16(addr, w1 as u16));
                }
            }
        }
    }

    ops
}

#[derive(Default)]
pub struct CheatManager {
    pub cheats: Vec<CheatEntry>,
    pub ram_searcher: RamSearcher,
}

impl CheatManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_cheat(&mut self, name: impl Into<String>, code: impl Into<String>) {
        self.cheats.push(CheatEntry::new(name, code));
    }

    pub fn remove_cheat(&mut self, index: usize) {
        if index < self.cheats.len() {
            self.cheats.remove(index);
        }
    }

    /// Evaluates and applies all active cheats to MMU memory
    pub fn apply(&self, mmu: &mut Mmu) {
        for cheat in &self.cheats {
            if !cheat.enabled {
                continue;
            }
            for op in &cheat.ops {
                match *op {
                    CheatOp::Write8(addr, val) => mmu.write8(addr, val),
                    CheatOp::Write16(addr, val) => mmu.write16(addr, val),
                    CheatOp::Write32(addr, val) => mmu.write32(addr, val),
                }
            }
        }
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let mut out = String::new();
        for cheat in &self.cheats {
            out.push_str(&format!("[{}]\n", cheat.name));
            out.push_str(&format!("enabled = {}\n", cheat.enabled));
            out.push_str(&format!("code = {}\n\n", cheat.code));
        }
        fs::write(path, out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSize {
    U8,
    U16,
    U32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareType {
    Exact(u32),
    EqualPrevious,
    GreaterThanPrevious,
    LessThanPrevious,
    Changed,
    Unchanged,
}

pub struct SearchCandidate {
    pub address: u32,
    pub previous_value: u32,
    pub current_value: u32,
}

pub struct RamSearcher {
    pub search_size: SearchSize,
    pub candidates: Vec<SearchCandidate>,
    pub previous_ewram: Box<[u8; 256 * 1024]>,
    pub previous_iwram: Box<[u8; 32 * 1024]>,
    pub has_searched: bool,
}

impl Default for RamSearcher {
    fn default() -> Self {
        Self {
            search_size: SearchSize::U16,
            candidates: Vec::new(),
            previous_ewram: Box::new([0; 256 * 1024]),
            previous_iwram: Box::new([0; 32 * 1024]),
            has_searched: false,
        }
    }
}

impl RamSearcher {
    pub fn new() -> Self {
        Self::default()
    }

    fn read_val_at(mmu: &Mmu, addr: u32, size: SearchSize) -> u32 {
        match size {
            SearchSize::U8 => mmu.read8(addr) as u32,
            SearchSize::U16 => mmu.read16(addr & !1) as u32,
            SearchSize::U32 => mmu.read32(addr & !3),
        }
    }

    /// Performs the initial scan over EWRAM (`0x02000000`) and IWRAM (`0x03000000`)
    pub fn initial_search(&mut self, mmu: &Mmu, target: Option<u32>) {
        self.candidates.clear();
        self.previous_ewram.copy_from_slice(&mmu.ewram[..]);
        self.previous_iwram.copy_from_slice(&mmu.iwram[..]);
        self.has_searched = true;

        let step = match self.search_size {
            SearchSize::U8 => 1,
            SearchSize::U16 => 2,
            SearchSize::U32 => 4,
        };

        // Scan EWRAM (256 KB)
        let mut offset = 0;
        while offset + step <= 256 * 1024 {
            let addr = 0x0200_0000 + offset as u32;
            let val = Self::read_val_at(mmu, addr, self.search_size);
            if let Some(t) = target {
                if val == t {
                    self.candidates.push(SearchCandidate {
                        address: addr,
                        previous_value: val,
                        current_value: val,
                    });
                }
            } else {
                self.candidates.push(SearchCandidate {
                    address: addr,
                    previous_value: val,
                    current_value: val,
                });
            }
            offset += step;
        }

        // Scan IWRAM (32 KB)
        let mut offset = 0;
        while offset + step <= 32 * 1024 {
            let addr = 0x0300_0000 + offset as u32;
            let val = Self::read_val_at(mmu, addr, self.search_size);
            if let Some(t) = target {
                if val == t {
                    self.candidates.push(SearchCandidate {
                        address: addr,
                        previous_value: val,
                        current_value: val,
                    });
                }
            } else {
                self.candidates.push(SearchCandidate {
                    address: addr,
                    previous_value: val,
                    current_value: val,
                });
            }
            offset += step;
        }
    }

    /// Refines existing candidates with a comparison rule
    pub fn filter_search(&mut self, mmu: &Mmu, cmp: CompareType) {
        if !self.has_searched {
            if let CompareType::Exact(val) = cmp {
                self.initial_search(mmu, Some(val));
            } else {
                self.initial_search(mmu, None);
            }
            return;
        }

        let mut next_candidates = Vec::with_capacity(self.candidates.len());
        for mut cand in self.candidates.drain(..) {
            let current = Self::read_val_at(mmu, cand.address, self.search_size);
            let prev = cand.current_value;

            let matches = match cmp {
                CompareType::Exact(val) => current == val,
                CompareType::EqualPrevious => current == prev,
                CompareType::GreaterThanPrevious => current > prev,
                CompareType::LessThanPrevious => current < prev,
                CompareType::Changed => current != prev,
                CompareType::Unchanged => current == prev,
            };

            if matches {
                cand.previous_value = prev;
                cand.current_value = current;
                next_candidates.push(cand);
            }
        }

        self.candidates = next_candidates;
        self.previous_ewram.copy_from_slice(&mmu.ewram[..]);
        self.previous_iwram.copy_from_slice(&mmu.iwram[..]);
    }
}
