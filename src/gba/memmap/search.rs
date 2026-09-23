//! Guided RAM search (ROADMAP M11).
//!
//! The classic cheat-finder loop: start from every candidate address, then
//! narrow the set with observations ("it's 56 now", "it went up by one",
//! "it didn't change"). Two additions over a plain search:
//!
//! - **XOR pairs.** A candidate can be `stored ^ key` where the key is
//!   another word in RAM. Seeding such a search indexes every aligned word
//!   once, so finding all pairs that decode to a value is linear, not
//!   quadratic.
//! - **Pointer generalization** (`pointer_forms`): turn an absolute hit into
//!   the `[base]+offset` forms that would still find it after the game
//!   relocates its data, so the hit can be checked in another session.

use std::collections::HashMap;

use super::{Encoding, Location, RamSnapshot, IWRAM_BASE, IWRAM_SIZE};

/// One candidate: where, how wide, how decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Candidate {
    pub addr: u32,
    pub width: u8,
    /// XOR key address (absolute, same width), if any.
    pub key: Option<u32>,
}

impl Candidate {
    pub fn read(&self, s: &RamSnapshot) -> Option<u32> {
        let v = s.read(self.addr, self.width)?;
        Some(match self.key {
            Some(k) => v ^ s.read(k, self.width)?,
            None => v,
        })
    }

    pub fn encoding(&self) -> Encoding {
        match self.key {
            Some(k) => Encoding::Xor { key: Location::Absolute { addr: k } },
            None => Encoding::Plain,
        }
    }
}

/// An observation about how a variable relates to the previous snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Relation {
    Equals(u32),
    Unchanged,
    Changed,
    Increased,
    Decreased,
    /// new - old == delta (wrapping at the candidate's width).
    ChangedBy(i64),
}

fn mask(width: u8) -> u32 {
    if width >= 4 {
        u32::MAX
    } else {
        (1u32 << (width * 8)) - 1
    }
}

impl Relation {
    fn holds(&self, old: Option<u32>, new: u32, width: u8, signed: bool) -> bool {
        let m = mask(width);
        let sx = |v: u32| -> i64 {
            if signed {
                let shift = 32 - width as u32 * 8;
                ((v << shift) as i32 >> shift) as i64
            } else {
                v as i64
            }
        };
        match *self {
            Relation::Equals(x) => new == x & m,
            Relation::Unchanged => old == Some(new),
            Relation::Changed => old.is_some_and(|o| o != new),
            Relation::Increased => old.is_some_and(|o| sx(new) > sx(o)),
            Relation::Decreased => old.is_some_and(|o| sx(new) < sx(o)),
            Relation::ChangedBy(d) => old.is_some_and(|o| (new.wrapping_sub(o) & m) == (d as u32 & m)),
        }
    }
}

/// A narrowing RAM search.
#[derive(Debug, Clone)]
pub struct Search {
    pub candidates: Vec<Candidate>,
    /// Last value of each candidate (same order).
    last: Vec<Option<u32>>,
    pub signed: bool,
    /// Observations applied so far.
    pub steps: usize,
}

impl Search {
    /// Every aligned address of the given width in EWRAM + IWRAM.
    pub fn all(snap: &RamSnapshot, width: u8) -> Self {
        let mut candidates = Vec::new();
        for (base, bytes) in snap.regions() {
            let mut off = 0;
            while off + width as usize <= bytes.len() {
                candidates.push(Candidate { addr: base + off as u32, width, key: None });
                off += width as usize;
            }
        }
        Self::from_candidates(snap, candidates)
    }

    /// Every aligned address whose value is `value` right now.
    pub fn equal_to(snap: &RamSnapshot, width: u8, value: u32) -> Self {
        let mut s = Self::all(snap, width);
        s.observe(snap, Relation::Equals(value));
        s
    }

    /// Every (word, key word) pair of 32-bit words in RAM whose XOR is
    /// `value` right now. A zero key is skipped (that's a plain match).
    pub fn xor_pairs(snap: &RamSnapshot, value: u32) -> Self {
        let mut by_value: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut words = Vec::new();
        for (base, bytes) in snap.regions() {
            for (i, c) in bytes.chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let addr = base + i as u32 * 4;
                by_value.entry(v).or_default().push(addr);
                words.push((addr, v));
            }
        }
        let mut candidates = Vec::new();
        for (addr, v) in words {
            let key_val = v ^ value;
            // Skip degenerate pairs: a key of 0 (plain match), a key equal
            // to the stored word, and a stored word of 0 (then the "key" is
            // just the value itself sitting somewhere in RAM).
            if v == 0 || key_val == 0 || key_val == v {
                continue;
            }
            if let Some(keys) = by_value.get(&key_val) {
                // Keys used by thousands of words are fill patterns.
                if keys.len() > 8 {
                    continue;
                }
                for &k in keys {
                    candidates.push(Candidate { addr, width: 4, key: Some(k) });
                }
            }
        }
        Self::from_candidates(snap, candidates)
    }

    pub fn from_candidates(snap: &RamSnapshot, candidates: Vec<Candidate>) -> Self {
        let last = candidates.iter().map(|c| c.read(snap)).collect();
        Self { candidates, last, signed: false, steps: 0 }
    }

    /// Keep the candidates for which `rel` holds between the previous
    /// snapshot and `snap`. Returns how many are left.
    pub fn observe(&mut self, snap: &RamSnapshot, rel: Relation) -> usize {
        let signed = self.signed;
        let mut keep_c = Vec::with_capacity(self.candidates.len());
        let mut keep_l = Vec::with_capacity(self.candidates.len());
        for (c, old) in self.candidates.iter().zip(&self.last) {
            if let Some(new) = c.read(snap) {
                if rel.holds(*old, new, c.width, signed) {
                    keep_c.push(*c);
                    keep_l.push(Some(new));
                }
            }
        }
        self.candidates = keep_c;
        self.last = keep_l;
        self.steps += 1;
        self.candidates.len()
    }

    /// Update remembered values without filtering (e.g. after a step the
    /// player doesn't want to describe).
    pub fn rebase(&mut self, snap: &RamSnapshot) {
        self.last = self.candidates.iter().map(|c| c.read(snap)).collect();
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

/// How far past a pointer a field may sit. Gen 3's SaveBlock1 is ~16 KB.
pub const MAX_POINTER_OFFSET: u32 = 0x4000;

/// All the ways to reach `addr` in `snap`: the absolute address, plus
/// `[base]+offset` for every aligned IWRAM word `base` holding an EWRAM or
/// IWRAM pointer at most `MAX_POINTER_OFFSET` bytes below `addr`. Games keep
/// their long-lived pointers in IWRAM; EWRAM is not scanned for bases.
pub fn pointer_forms(snap: &RamSnapshot, addr: u32) -> Vec<Location> {
    let mut out = vec![Location::Absolute { addr }];
    for (i, c) in snap.iwram.chunks_exact(4).enumerate() {
        let p = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        if p & 3 != 0 || !super::in_ram(p) || p > addr || addr - p >= MAX_POINTER_OFFSET {
            continue;
        }
        let base = IWRAM_BASE + i as u32 * 4;
        if base as usize >= IWRAM_BASE as usize + IWRAM_SIZE {
            break;
        }
        out.push(Location::Pointer { base, offset: addr - p });
    }
    out
}

/// A candidate generalized over pointer forms (value and key both).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Portable {
    pub location: Location,
    pub width: u8,
    pub key: Option<Location>,
}

impl Portable {
    pub fn read(&self, snap: &RamSnapshot) -> Option<u32> {
        let v = snap.read(self.location.resolve(snap)?, self.width)?;
        Some(match self.key {
            Some(k) => v ^ snap.read(k.resolve(snap)?, self.width)?,
            None => v,
        })
    }

    pub fn encoding(&self) -> Encoding {
        match self.key {
            Some(key) => Encoding::Xor { key },
            None => Encoding::Plain,
        }
    }

    /// Fewer indirections and absolute-over-pointer are simpler; lower is
    /// preferred when several forms agree.
    pub fn complexity(&self) -> u32 {
        let c = |l: &Location| match l {
            Location::Absolute { .. } => 0,
            Location::Pointer { .. } => 1,
        };
        c(&self.location) + self.key.as_ref().map_or(0, |k| 1 + c(k))
    }
}

/// Expand surviving candidates into every portable form.
pub fn portable_forms(snap: &RamSnapshot, cands: &[Candidate]) -> Vec<Portable> {
    let mut out = Vec::new();
    let mut cache: HashMap<u32, Vec<Location>> = HashMap::new();
    let mut forms = |a: u32| cache.entry(a).or_insert_with(|| pointer_forms(snap, a)).clone();
    for c in cands {
        let locs = forms(c.addr);
        match c.key {
            None => out.extend(locs.into_iter().map(|location| Portable { location, width: c.width, key: None })),
            Some(k) => {
                let keys = forms(k);
                for &location in &locs {
                    for &key in &keys {
                        out.push(Portable { location, width: c.width, key: Some(key) });
                    }
                }
            }
        }
    }
    out
}

/// Keep the forms that still read `value` in another session's snapshot.
pub fn confirm(forms: &[Portable], snap: &RamSnapshot, value: u32) -> Vec<Portable> {
    forms.iter().copied().filter(|f| f.read(snap) == Some(value & mask(f.width))).collect()
}

#[cfg(test)]
mod tests {
    use super::super::{EWRAM_BASE, EWRAM_SIZE, IWRAM_SIZE};
    use super::*;

    fn snap() -> RamSnapshot {
        RamSnapshot { ewram: vec![0; EWRAM_SIZE].into(), iwram: vec![0; IWRAM_SIZE].into() }
    }

    fn put(s: &mut RamSnapshot, addr: u32, v: u32) {
        let (buf, off) = if addr >= IWRAM_BASE { (&mut s.iwram, addr - IWRAM_BASE) } else { (&mut s.ewram, addr - EWRAM_BASE) };
        buf[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn narrowing_by_relations() {
        let mut a = snap();
        put(&mut a, 0x0200_0100, 29);
        put(&mut a, 0x0200_0200, 29);
        let mut s = Search::equal_to(&a, 2, 29);
        assert_eq!(s.len(), 2);
        let mut b = a.clone();
        put(&mut b, 0x0200_0100, 25);
        assert_eq!(s.observe(&b, Relation::Decreased), 1);
        assert_eq!(s.candidates[0].addr, 0x0200_0100);
        let mut c = b.clone();
        put(&mut c, 0x0200_0100, 29);
        assert_eq!(s.observe(&c, Relation::ChangedBy(4)), 1);
    }

    #[test]
    fn xor_pairs_find_encoded_money_and_survive_relocation() {
        let key = 0x8E25_989Au32;
        // Session 1: blocks at 0x1000 / 0x2000, pointers in IWRAM.
        let mut a = snap();
        put(&mut a, 0x0300_5D8C, 0x0200_1000);
        put(&mut a, 0x0300_5D90, 0x0200_2000);
        put(&mut a, 0x0200_1490, 56 ^ key);
        put(&mut a, 0x0200_20AC, key);
        let s = Search::xor_pairs(&a, 56);
        assert!(s.candidates.iter().any(|c| c.addr == 0x0200_1490 && c.key == Some(0x0200_20AC)));
        // Session 2: same data relocated.
        let mut b = snap();
        put(&mut b, 0x0300_5D8C, 0x0200_1044);
        put(&mut b, 0x0300_5D90, 0x0200_2044);
        put(&mut b, 0x0200_14D4, 56 ^ key);
        put(&mut b, 0x0200_20F0, key);
        let forms = portable_forms(&a, &s.candidates);
        let ok = confirm(&forms, &b, 56);
        assert!(ok.contains(&Portable {
            location: Location::Pointer { base: 0x0300_5D8C, offset: 0x490 },
            width: 4,
            key: Some(Location::Pointer { base: 0x0300_5D90, offset: 0xAC }),
        }));
        // The absolute form moved and is gone.
        assert!(!ok.iter().any(|p| p.location == Location::Absolute { addr: 0x0200_1490 }));
    }
}
