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

// Default TEA seeds for the two GBA cheat-device encryption variants. These
// match the values real GameShark v1/v2 and Action Replay v3 devices use
// when no DEADFACE seed-change code has been applied (the vast majority of
// distributed codes never change the seed, so this covers them).
const GSA_V1_SEEDS: [u32; 4] = [0x09F4_FBBD, 0x9681_884A, 0x3520_27E9, 0xF3DE_E5A7];
const GSA_V3_SEEDS: [u32; 4] = [0x7AA9_648F, 0x7FAE_6994, 0xC0EF_AAD5, 0x4271_2C57];

/// Decrypts a raw GameShark/Action Replay (TEA-based) address:value pair.
/// This is the standard 32-round TEA decryption used by real GBA GameShark
/// v1/v2 and Action Replay v3 devices to turn the ciphertext codes found in
/// public cheat databases into the actual (address, value) pair to poke.
fn decrypt_gsa(mut address: u32, mut value: u32, seeds: &[u32; 4]) -> (u32, u32) {
    let mut rolling_seed: u32 = 0xC6EF_3720; // 32 * 0x9E3779B9 (mod 2^32)
    for _ in 0..32 {
        value = value.wrapping_sub(
            (((address << 4).wrapping_add(seeds[2])) ^ address.wrapping_add(rolling_seed))
                ^ ((address >> 5).wrapping_add(seeds[3])),
        );
        address = address.wrapping_sub(
            (((value << 4).wrapping_add(seeds[0])) ^ value.wrapping_add(rolling_seed))
                ^ ((value >> 5).wrapping_add(seeds[1])),
        );
        rolling_seed = rolling_seed.wrapping_sub(0x9E37_79B9);
    }
    (address, value)
}

/// True if `addr` lands in EWRAM (0x02xxxxxx) or IWRAM (0x03xxxxxx), the two
/// regions the overwhelming majority of real cheat codes target. Used as a
/// plausibility check to pick between the v1 and v3 seed tables, since the
/// raw ciphertext alone doesn't identify which device encrypted it.
fn looks_like_valid_target(addr: u32) -> bool {
    matches!((addr >> 24) & 0xFF, 0x02 | 0x03)
}

/// Decodes an already-TEA-decrypted GameShark v1/v2 (address, value) pair
/// into a write op, for the common single-write code types (0/1/2 = 8/16/32
/// -bit write). Other v1 opcodes (group writes, ROM patches, conditional
/// writes, slides, etc.) are intentionally unsupported and produce no op
/// rather than guessing -- silently doing nothing is safer than poking the
/// wrong address.
fn decode_gsa_v1(address: u32, value: u32) -> Option<CheatOp> {
    let write_type = (address >> 28) & 0xF;
    let target = address & 0x0FFF_FFFF;
    match write_type {
        0 => Some(CheatOp::Write8(target, value as u8)),
        1 => Some(CheatOp::Write16(target, value as u16)),
        2 => Some(CheatOp::Write32(target, value)),
        _ => None,
    }
}

/// Decodes an already-TEA-decrypted Action Replay v3 (address, value) pair
/// into a write op, for the common single-write code types. Other v3 opcodes
/// (conditionals, slides, fills, ROM patches, IO writes, master codes, etc.)
/// are intentionally unsupported; see `decode_gsa_v1` for why.
fn decode_gsa_v3(address: u32, value: u32) -> Option<CheatOp> {
    let code_type = ((address >> 25) & 0x7F) | ((address >> 17) & 0x80);
    let target = ((address & 0x00F0_0000) << 4) | (address & 0x0003_FFFF);
    match code_type {
        0x00 if address != 0 => Some(CheatOp::Write8(target, value as u8)),
        0x01 => Some(CheatOp::Write16(target, value as u16)),
        0x02 => Some(CheatOp::Write32(target, value)),
        _ => None,
    }
}

/// Attempts to decrypt a raw two-word hex code as a GameShark/Action Replay
/// cipher, trying the v1 seed table first and falling back to v3. Returns
/// `None` if neither decrypts to a plausible EWRAM/IWRAM target (unsupported
/// opcode, unrecognized format, or a DEADFACE-changed seed we don't model).
fn try_decrypt_gsa(w0: u32, w1: u32) -> Option<CheatOp> {
    let (addr_v1, val_v1) = decrypt_gsa(w0, w1, &GSA_V1_SEEDS);
    if looks_like_valid_target(addr_v1 & 0x0FFF_FFFF) {
        if let Some(op) = decode_gsa_v1(addr_v1, val_v1) {
            return Some(op);
        }
    }

    let (addr_v3, val_v3) = decrypt_gsa(w0, w1, &GSA_V3_SEEDS);
    let target_v3 = ((addr_v3 & 0x00F0_0000) << 4) | (addr_v3 & 0x0003_FFFF);
    if looks_like_valid_target(target_v3) {
        if let Some(op) = decode_gsa_v3(addr_v3, val_v3) {
            return Some(op);
        }
    }

    None
}

/// Parses multi-line cheat strings supporting:
/// 1. Action Replay / GameShark GBA (TEA-encrypted): `XXXXXXXX YYYYYYYY`
/// 2. CodeBreaker: `8XXXXXXX YYYY` (16-bit write), `3XXXXXXX 00YY` (8-bit write)
///    -- NOTE: real CodeBreaker codes are encrypted with a per-game seed
///    derived from a master/seed code; that scheme is not implemented here,
///    so only already-plaintext CodeBreaker-formatted address:value pairs work.
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
                // Already-plaintext EWRAM / IWRAM address:value (e.g. from the
                // built-in RAM Searcher) -- pass through unencrypted.
                else if (w0 >> 24) == 0x02 || (w0 >> 24) == 0x03 {
                    if p1.len() <= 2 {
                        ops.push(CheatOp::Write8(w0, w1 as u8));
                    } else if p1.len() <= 4 {
                        ops.push(CheatOp::Write16(w0, w1 as u16));
                    } else {
                        ops.push(CheatOp::Write32(w0, w1));
                    }
                } else {
                    // Otherwise this is a real encrypted GameShark/Action Replay
                    // code: decrypt it (see try_decrypt_gsa) instead of treating
                    // the ciphertext as a literal address.
                    if let Some(op) = try_decrypt_gsa(w0, w1) {
                        ops.push(op);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// TEA encrypt (the forward direction), used only by this test to verify
    /// `decrypt_gsa` is a correct inverse of the real device's algorithm shape.
    fn encrypt_gsa(mut address: u32, mut value: u32, seeds: &[u32; 4]) -> (u32, u32) {
        let mut rolling_seed: u32 = 0;
        for _ in 0..32 {
            rolling_seed = rolling_seed.wrapping_add(0x9E37_79B9);
            address = address.wrapping_add(
                (((value << 4).wrapping_add(seeds[0])) ^ value.wrapping_add(rolling_seed))
                    ^ ((value >> 5).wrapping_add(seeds[1])),
            );
            value = value.wrapping_add(
                (((address << 4).wrapping_add(seeds[2])) ^ address.wrapping_add(rolling_seed))
                    ^ ((address >> 5).wrapping_add(seeds[3])),
            );
        }
        (address, value)
    }

    #[test]
    fn gsa_decrypt_is_inverse_of_encrypt() {
        for &seeds in &[&GSA_V1_SEEDS, &GSA_V3_SEEDS] {
            let plain_addr = 0x0203_1234u32;
            let plain_val = 0x0000_0063u32;
            let (c_addr, c_val) = encrypt_gsa(plain_addr, plain_val, seeds);
            let (d_addr, d_val) = decrypt_gsa(c_addr, c_val, seeds);
            assert_eq!((d_addr, d_val), (plain_addr, plain_val));
        }
    }

    #[test]
    fn decode_gsa_v1_simple_write_types() {
        // type 0 = 8-bit write, target = address & 0x0FFFFFFF
        assert_eq!(decode_gsa_v1(0x0200_1000, 0xAB), Some(CheatOp::Write8(0x0200_1000, 0xAB)));
        // type 1 = 16-bit write
        assert_eq!(decode_gsa_v1(0x1200_1000, 0x1234), Some(CheatOp::Write16(0x0200_1000, 0x1234)));
        // type 2 = 32-bit write
        assert_eq!(decode_gsa_v1(0x2200_1000, 0xDEAD_BEEF), Some(CheatOp::Write32(0x0200_1000, 0xDEAD_BEEF)));
        // unsupported opcode type -> no-op rather than a wrong guess
        assert_eq!(decode_gsa_v1(0x3200_1000, 0x0000_0000), None);
    }

    #[test]
    fn try_decrypt_gsa_roundtrips_a_real_looking_v1_code() {
        // Build a plaintext 8-bit-write code (type 0) targeting EWRAM, encrypt
        // it with the v1 seeds, and confirm the public parser recovers it.
        let target = 0x0203_00AAu32; // type nibble 0 => 8-bit write
        let value = 0x0000_0009u32;
        let (cipher_addr, cipher_val) = encrypt_gsa(target, value, &GSA_V1_SEEDS);
        let op = try_decrypt_gsa(cipher_addr, cipher_val);
        assert_eq!(op, Some(CheatOp::Write8(target, 0x09)));
    }

    #[test]
    fn parse_cheat_code_decrypts_gsa_line() {
        let target = 0x0203_0050u32;
        // Real GSA ciphertext can coincidentally start with the nibble the
        // CodeBreaker heuristic reserves (0x3/0x8) -- that ambiguity is a
        // pre-existing limitation of format auto-detection without an
        // explicit user-selected cheat type, not something this test is
        // about. Search a small range of values for one whose ciphertext
        // doesn't collide with that heuristic, so this test exercises the
        // GSA decrypt path specifically.
        let (line, expected_value) = (0..64u32)
            .find_map(|salt| {
                let value = 0x0000_2A00u32 + salt;
                // OR the type-2 (32-bit write) nibble into the address's top bits before encrypting.
                let (cipher_addr, cipher_val) = encrypt_gsa(target | 0x2000_0000, value, &GSA_V1_SEEDS);
                let top_nibble = cipher_addr >> 28;
                if top_nibble != 0x8 && top_nibble != 0x3 {
                    Some((format!("{:08X} {:08X}", cipher_addr, cipher_val), value))
                } else {
                    None
                }
            })
            .expect("expected at least one non-colliding value in range");

        let ops = parse_cheat_code(&line);
        assert_eq!(ops, vec![CheatOp::Write32(target, expected_value)]);
    }

    #[test]
    fn parse_cheat_code_leaves_plaintext_ewram_pokes_untouched() {
        let ops = parse_cheat_code("0202402C 270F");
        assert_eq!(ops, vec![CheatOp::Write16(0x0202402C, 0x270F)]);
    }
}
