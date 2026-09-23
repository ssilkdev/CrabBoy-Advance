//! Per-game memory maps and automatic variable discovery (ROADMAP M11).
//!
//! A *memory map* names the places in RAM where a game keeps things a player
//! cares about: HP, position, money, whether a text box is open, where the
//! text-box string lives. The rest of the emulator (dialogue reader, AI hint
//! mode, the companion) reads variables through the map instead of
//! hard-coding addresses per game.
//!
//! Maps are *discovered*, not typed in (see `discover`). Two things make that
//! harder than a classic RAM search, and both are handled here:
//!
//! - **Relocation.** Many games (Pokémon Gen 3 among them) move their save
//!   blocks around in EWRAM, so an address that's right today is wrong after
//!   the next reset. A variable's [`Location`] can therefore be relative to a
//!   pointer (`*(base) + offset`), and discovery picks between absolute and
//!   pointer forms by checking them against more than one play session.
//! - **Encoding.** Some values are stored XORed with a per-save key (Gen 3
//!   money). [`Encoding::Xor`] names the key's location, which is itself
//!   relocatable.

pub mod discover;
pub mod search;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::mmu::Mmu;

/// Start and size of the two work-RAM regions discovery looks at.
pub const EWRAM_BASE: u32 = 0x0200_0000;
pub const EWRAM_SIZE: usize = 256 * 1024;
pub const IWRAM_BASE: u32 = 0x0300_0000;
pub const IWRAM_SIZE: usize = 32 * 1024;

/// A copy of EWRAM + IWRAM at one moment.
#[derive(Clone)]
pub struct RamSnapshot {
    pub ewram: Box<[u8]>,
    pub iwram: Box<[u8]>,
}

impl RamSnapshot {
    pub fn capture(mmu: &Mmu) -> Self {
        Self { ewram: mmu.ewram[..].into(), iwram: mmu.iwram[..].into() }
    }

    /// Byte slice holding `addr` and everything after it in its region.
    fn tail(&self, addr: u32) -> Option<&[u8]> {
        if (EWRAM_BASE..EWRAM_BASE + EWRAM_SIZE as u32).contains(&addr) {
            Some(&self.ewram[(addr - EWRAM_BASE) as usize..])
        } else if (IWRAM_BASE..IWRAM_BASE + IWRAM_SIZE as u32).contains(&addr) {
            Some(&self.iwram[(addr - IWRAM_BASE) as usize..])
        } else {
            None
        }
    }

    /// Little-endian read of `width` (1, 2 or 4) bytes.
    pub fn read(&self, addr: u32, width: u8) -> Option<u32> {
        let t = self.tail(addr)?;
        let w = width as usize;
        if t.len() < w {
            return None;
        }
        Some(t[..w].iter().rev().fold(0u32, |acc, &b| acc << 8 | b as u32))
    }

    pub fn bytes(&self, addr: u32, len: usize) -> Option<&[u8]> {
        let t = self.tail(addr)?;
        t.get(..len)
    }

    /// Every (address, byte) of both regions, EWRAM first.
    pub fn regions(&self) -> [(u32, &[u8]); 2] {
        [(EWRAM_BASE, &self.ewram[..]), (IWRAM_BASE, &self.iwram[..])]
    }
}

pub fn in_ram(addr: u32) -> bool {
    (EWRAM_BASE..EWRAM_BASE + EWRAM_SIZE as u32).contains(&addr)
        || (IWRAM_BASE..IWRAM_BASE + IWRAM_SIZE as u32).contains(&addr)
}

/// Where a variable lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Location {
    /// A fixed address.
    Absolute { addr: u32 },
    /// `offset` bytes past the address stored (as a u32) at `base`.
    Pointer { base: u32, offset: u32 },
}

impl Location {
    pub fn resolve(&self, snap: &RamSnapshot) -> Option<u32> {
        match *self {
            Location::Absolute { addr } => Some(addr),
            Location::Pointer { base, offset } => {
                let p = snap.read(base, 4)?.wrapping_add(offset);
                in_ram(p).then_some(p)
            }
        }
    }

    pub fn resolve_live(&self, mmu: &Mmu) -> Option<u32> {
        match *self {
            Location::Absolute { addr } => Some(addr),
            Location::Pointer { base, offset } => {
                let p = mmu.read32(base).wrapping_add(offset);
                in_ram(p).then_some(p)
            }
        }
    }

    pub fn describe(&self) -> String {
        match *self {
            Location::Absolute { addr } => format!("0x{addr:08X}"),
            Location::Pointer { base, offset } => format!("[0x{base:08X}]+0x{offset:X}"),
        }
    }
}

/// How the stored bytes map to the value the player sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Encoding {
    Plain,
    /// value = stored ^ key, with the key read at `key` using the same width.
    Xor { key: Location },
}

/// Rule turning a raw value into on/off for flag variables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "values")]
pub enum FlagRule {
    /// On whenever the value is not this.
    NotEqual(u32),
    /// On exactly when the value is this.
    Equal(u32),
    /// On when the value is one of these.
    OneOf(Vec<u32>),
}

impl FlagRule {
    pub fn eval(&self, v: u32) -> bool {
        match self {
            FlagRule::NotEqual(x) => v != *x,
            FlagRule::Equal(x) => v == *x,
            FlagRule::OneOf(xs) => xs.contains(&v),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            FlagRule::NotEqual(x) => format!("on when != {x}"),
            FlagRule::Equal(x) => format!("on when == {x}"),
            FlagRule::OneOf(xs) => format!("on when in {xs:?}"),
        }
    }
}

/// What sort of thing a variable is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum VarKind {
    /// A number shown to the player (HP, money...).
    Number,
    /// A yes/no state (text box open, menu open...).
    Flag { rule: FlagRule },
    /// Player X or Y.
    Position { axis: Axis },
    /// A text buffer of up to `max_len` bytes, in the game's own encoding.
    Text { max_len: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
}

/// One named variable in a memory map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub kind: VarKind,
    pub location: Location,
    /// Bytes: 1, 2 or 4.
    pub width: u8,
    #[serde(default)]
    pub signed: bool,
    #[serde(default = "plain")]
    pub encoding: Encoding,
    /// Play sessions (resets with a fresh memory layout) this was confirmed in.
    #[serde(default)]
    pub sessions_confirmed: u32,
}

fn plain() -> Encoding {
    Encoding::Plain
}

/// A variable's current value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(i64),
    Flag(bool),
    Text(Vec<u8>),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Number(n) => write!(f, "{n}"),
            Value::Flag(b) => write!(f, "{}", if *b { "yes" } else { "no" }),
            Value::Text(t) => {
                let hex: Vec<String> = t.iter().take(16).map(|b| format!("{b:02X}")).collect();
                write!(f, "{}{}", hex.join(" "), if t.len() > 16 { " …" } else { "" })
            }
        }
    }
}

impl Variable {
    fn raw(&self, addr: u32, key: Option<u32>, read: impl Fn(u32, u8) -> Option<u32>) -> Option<i64> {
        let mut v = read(addr, self.width)?;
        if let Some(k) = key {
            v ^= k;
        }
        let mask = if self.width >= 4 { u32::MAX } else { (1u32 << (self.width * 8)) - 1 };
        v &= mask;
        Some(if self.signed {
            let shift = 32 - self.width as u32 * 8;
            ((v << shift) as i32 >> shift) as i64
        } else {
            v as i64
        })
    }

    fn finish(&self, n: i64, text: impl Fn(u32) -> Vec<u8>, addr: u32) -> Value {
        match &self.kind {
            VarKind::Flag { rule } => Value::Flag(rule.eval(n as u32)),
            VarKind::Text { .. } => Value::Text(text(addr)),
            _ => Value::Number(n),
        }
    }

    /// Read the variable from a snapshot.
    pub fn read(&self, snap: &RamSnapshot) -> Option<Value> {
        let addr = self.location.resolve(snap)?;
        let key = match self.encoding {
            Encoding::Plain => None,
            Encoding::Xor { key } => Some(snap.read(key.resolve(snap)?, self.width)?),
        };
        let n = self.raw(addr, key, |a, w| snap.read(a, w))?;
        let max = match self.kind {
            VarKind::Text { max_len } => max_len as usize,
            _ => 0,
        };
        Some(self.finish(n, |a| snap.bytes(a, max).map(|b| b.to_vec()).unwrap_or_default(), addr))
    }

    /// Read the variable from the running emulator.
    pub fn read_live(&self, mmu: &Mmu) -> Option<Value> {
        let addr = self.location.resolve_live(mmu)?;
        let rd = |a: u32, w: u8| -> Option<u32> {
            in_ram(a).then(|| match w {
                1 => mmu.read8(a) as u32,
                2 => mmu.read16(a) as u32,
                _ => mmu.read32(a),
            })
        };
        let key = match self.encoding {
            Encoding::Plain => None,
            Encoding::Xor { key } => Some(rd(key.resolve_live(mmu)?, self.width)?),
        };
        let n = self.raw(addr, key, rd)?;
        let max = match self.kind {
            VarKind::Text { max_len } => max_len,
            _ => 0,
        };
        Some(self.finish(n, |a| (0..max).map(|i| mmu.read8(a + i)).collect(), addr))
    }

    /// Address (and key address) right now, for jumping to it in the hex
    /// editor.
    pub fn current_address(&self, mmu: &Mmu) -> Option<u32> {
        self.location.resolve_live(mmu)
    }

    pub fn describe(&self) -> String {
        let enc = match self.encoding {
            Encoding::Plain => String::new(),
            Encoding::Xor { key } => format!(" xor {}", key.describe()),
        };
        format!("{} u{}{}", self.location.describe(), self.width as u32 * 8, enc)
    }
}

/// A game's memory map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryMap {
    pub game_code: String,
    pub title: String,
    pub variables: Vec<Variable>,
}

impl MemoryMap {
    pub fn new(game_code: &str, title: &str) -> Self {
        Self { game_code: game_code.to_string(), title: title.to_string(), variables: Vec::new() }
    }

    pub fn get(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// Add or replace (by name).
    pub fn set(&mut self, var: Variable) {
        match self.variables.iter_mut().find(|v| v.name.eq_ignore_ascii_case(&var.name)) {
            Some(slot) => *slot = var,
            None => self.variables.push(var),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.variables.retain(|v| !v.name.eq_ignore_ascii_case(name));
    }

    pub fn read_live(&self, name: &str, mmu: &Mmu) -> Option<Value> {
        self.get(name)?.read_live(mmu)
    }

    /// File name for a game: its code, or a sanitized title.
    pub fn file_name(game_code: &str, title: &str) -> String {
        let id = super::accessibility::game_key(game_code, title);
        let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        format!("{safe}.json")
    }

    pub fn path_in(dir: &Path, game_code: &str, title: &str) -> PathBuf {
        dir.join(Self::file_name(game_code, title))
    }

    pub fn load(dir: &Path, game_code: &str, title: &str) -> Option<Self> {
        let s = std::fs::read_to_string(Self::path_in(dir, game_code, title)).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = Self::path_in(dir, &self.game_code, &self.title);
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> RamSnapshot {
        RamSnapshot { ewram: vec![0; EWRAM_SIZE].into(), iwram: vec![0; IWRAM_SIZE].into() }
    }

    #[test]
    fn pointer_locations_and_xor_decode() {
        let mut s = snap();
        // pointer at IWRAM+0x10 -> EWRAM+0x1000; value at +0x490 = 56 ^ key
        s.iwram[0x10..0x14].copy_from_slice(&0x0200_1000u32.to_le_bytes());
        let key = 0x8E25_989Au32;
        s.ewram[0x1490..0x1494].copy_from_slice(&(56 ^ key).to_le_bytes());
        s.ewram[0x2000..0x2004].copy_from_slice(&key.to_le_bytes());
        let v = Variable {
            name: "money".into(),
            kind: VarKind::Number,
            location: Location::Pointer { base: 0x0300_0010, offset: 0x490 },
            width: 4,
            signed: false,
            encoding: Encoding::Xor { key: Location::Absolute { addr: 0x0200_2000 } },
            sessions_confirmed: 0,
        };
        assert_eq!(v.read(&s), Some(Value::Number(56)));
        assert!(v.describe().contains("xor"));
    }

    #[test]
    fn signed_and_flag_values() {
        let mut s = snap();
        s.ewram[4] = 0xFE;
        let mut v = Variable {
            name: "x".into(),
            kind: VarKind::Number,
            location: Location::Absolute { addr: EWRAM_BASE + 4 },
            width: 1,
            signed: true,
            encoding: Encoding::Plain,
            sessions_confirmed: 0,
        };
        assert_eq!(v.read(&s), Some(Value::Number(-2)));
        v.kind = VarKind::Flag { rule: FlagRule::NotEqual(0) };
        assert_eq!(v.read(&s), Some(Value::Flag(true)));
    }

    #[test]
    fn map_roundtrips_through_json_files() {
        let dir = std::env::temp_dir().join(format!("crabboy-memmap-{}", std::process::id()));
        let mut m = MemoryMap::new("BPEE", "POKEMON EMER");
        m.set(Variable {
            name: "HP".into(),
            kind: VarKind::Number,
            location: Location::Absolute { addr: 0x0202_4542 },
            width: 2,
            signed: false,
            encoding: Encoding::Plain,
            sessions_confirmed: 2,
        });
        m.save(&dir).unwrap();
        let back = MemoryMap::load(&dir, "bpee", "").unwrap();
        assert_eq!(back, m);
        let _ = std::fs::remove_dir_all(dir);
    }
}
