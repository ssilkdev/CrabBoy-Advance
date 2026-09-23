//! Automatic variable discovery (ROADMAP M11).
//!
//! The player (or a script) records *observations* while playing: a RAM
//! snapshot plus whatever they can see on screen at that moment ("HP is
//! 27", "I just stepped one tile right", "a text box is open", "this is the
//! second message"). Nobody says where anything is. From those, discovery
//! works out the location of each variable:
//!
//! 1. **Seed and narrow** in the first session with a RAM search (plain
//!    values and XOR-encoded pairs).
//! 2. **Generalize** each survivor to every pointer-relative form that
//!    reaches it (`search::portable_forms`).
//! 3. **Validate** each form against the other sessions. Sessions are
//!    separate boots, so data the game relocates lands elsewhere; only forms
//!    that follow it survive, which weeds out coincidences.
//! 4. **Rank** what's left (simplest form, sensible width) and keep the
//!    runner-ups as alternatives.
//!
//! Flags are learned rather than searched: every byte is a candidate
//! *decision stump* (`== v`, `!= v`, `in {..}`) trained on the first
//! session's labeled snapshots and kept only if it classifies every other
//! session's snapshots correctly (held-out sessions as a validation set).
//! Stumps using fewer distinct values generalize better and rank higher.
//!
//! Text buffers are found by what text *is*: a region that holds the same
//! bytes for as long as one message is showing, different bytes for a
//! different message, and whose opening bytes appear verbatim in the ROM
//! (the game copied the string from there). Window pixel/tile buffers pass
//! the first two tests but never the third.

use std::collections::{HashMap, HashSet};

use super::search::{confirm, portable_forms, Candidate, Portable, Relation, Search};
use super::{Axis, FlagRule, Location, MemoryMap, RamSnapshot, VarKind, Variable};

/// Something visible on screen when an observation was recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    /// `var` currently shows `value`.
    Value { var: String, value: u32 },
    /// Since the previous observation in this session the player moved this
    /// many tiles (x right, y down). `Moved { 0, 0 }` = stood still.
    Moved { dx: i32, dy: i32 },
    /// Flag `var` (e.g. "text box open") is currently on/off.
    Flag { var: String, on: bool },
    /// Text `var` is showing message number `message` (same number = same
    /// message; the number itself means nothing).
    Text { var: String, message: u32 },
}

/// A snapshot plus facts.
#[derive(Clone)]
pub struct Observation {
    /// Which boot this came from. Observations from different boots are what
    /// separates real variables from coincidences.
    pub session: u32,
    pub snap: RamSnapshot,
    pub facts: Vec<Fact>,
}

/// What discovery concluded about one variable.
#[derive(Debug, Clone)]
pub struct Finding {
    pub variable: Variable,
    /// Other forms that also fit every observation, best first. Narrow them
    /// down with `PokeRefiner`.
    pub alternatives: Vec<Variable>,
    /// Candidates alive after the first session's search, before
    /// cross-session validation (for the UI: "narrowed 70 000 → 12 → 1").
    pub after_first_session: usize,
}

impl Finding {
    pub fn is_unique(&self) -> bool {
        self.alternatives.is_empty()
    }
}

fn sessions(obs: &[Observation]) -> Vec<u32> {
    let mut seen = HashSet::new();
    obs.iter().map(|o| o.session).filter(|s| seen.insert(*s)).collect()
}

fn value_of(o: &Observation, var: &str) -> Option<u32> {
    o.facts.iter().find_map(|f| match f {
        Fact::Value { var: v, value } if v.eq_ignore_ascii_case(var) => Some(*value),
        _ => None,
    })
}

fn moved(o: &Observation) -> Option<(i32, i32)> {
    o.facts.iter().find_map(|f| match f {
        Fact::Moved { dx, dy } => Some((*dx, *dy)),
        _ => None,
    })
}

fn flag_of(o: &Observation, var: &str) -> Option<bool> {
    o.facts.iter().find_map(|f| match f {
        Fact::Flag { var: v, on } if v.eq_ignore_ascii_case(var) => Some(*on),
        _ => None,
    })
}

fn text_of(o: &Observation, var: &str) -> Option<u32> {
    o.facts.iter().find_map(|f| match f {
        Fact::Text { var: v, message } if v.eq_ignore_ascii_case(var) => Some(*message),
        _ => None,
    })
}

fn variable(name: &str, kind: VarKind, p: &Portable, signed: bool, sessions: u32) -> Variable {
    Variable {
        name: name.to_string(),
        kind,
        location: p.location,
        width: p.width,
        signed,
        encoding: p.encoding(),
        sessions_confirmed: sessions,
    }
}

/// Order: simplest location, then width 2 > 4 > 1 (a u8 hit inside a u16
/// field is the less specific read), then address for stability.
fn rank(forms: &mut Vec<Portable>) {
    let width_pref = |w: u8| match w {
        2 => 0,
        4 => 1,
        _ => 2,
    };
    let addr = |p: &Portable| match p.location {
        Location::Absolute { addr } => (0, addr, 0),
        Location::Pointer { base, offset } => (1, base, offset),
    };
    forms.sort_by_key(|p| (p.complexity(), width_pref(p.width), addr(p)));
    forms.dedup();
    // Drop a u8 read when a wider read at the same place also fits.
    let wide: HashSet<(Location, Option<Location>)> =
        forms.iter().filter(|p| p.width > 1).map(|p| (p.location, p.key)).collect();
    forms.retain(|p| p.width > 1 || !wide.contains(&(p.location, p.key)));
}

/// Among forms that reach the *same* data, keep the best-ranked one: e.g.
/// an absolute address and a pointer form that both work mean the data
/// doesn't move, so the absolute one is enough.
fn collapse_same_target(forms: Vec<Portable>, snap: &RamSnapshot) -> Vec<Portable> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in forms {
        // Width isn't part of the target: a u16 and a u32 read at the same
        // address both fit when the high bytes are zero, and `rank` already
        // put the preferred width first.
        let target = (p.location.resolve(snap), p.key.and_then(|k| k.resolve(snap)));
        if seen.insert(target) {
            out.push(p);
        }
    }
    out
}

fn finish(
    name: &str,
    kind: VarKind,
    mut forms: Vec<Portable>,
    n_sessions: u32,
    first: usize,
    snap: &RamSnapshot,
) -> Result<Finding, String> {
    rank(&mut forms);
    let forms = collapse_same_target(forms, snap);
    let mut vars = forms.iter().map(|p| variable(name, kind.clone(), p, false, n_sessions));
    let variable = vars.next().ok_or_else(|| format!("{name}: no location fits every observation"))?;
    Ok(Finding { variable, alternatives: vars.collect(), after_first_session: first })
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

/// Find a number the player can read off the screen (HP, money...).
pub fn discover_number(obs: &[Observation], name: &str) -> Result<Finding, String> {
    let sess = sessions(obs);
    let first_s = *sess.first().ok_or("no observations")?;
    let first: Vec<&Observation> =
        obs.iter().filter(|o| o.session == first_s && value_of(o, name).is_some()).collect();
    let Some((seed, rest)) = first.split_first() else {
        return Err(format!("{name}: no values recorded in the first session"));
    };
    let v0 = value_of(seed, name).unwrap();

    let mut cands: Vec<Candidate> = Vec::new();
    for width in [1u8, 2, 4] {
        if width < 4 && v0 >= 1 << (width * 8) {
            continue;
        }
        let mut s = Search::equal_to(&seed.snap, width, v0);
        for o in rest {
            if s.is_empty() {
                break;
            }
            s.observe(&o.snap, Relation::Equals(value_of(o, name).unwrap()));
        }
        cands.extend(s.candidates);
    }
    let mut xs = Search::xor_pairs(&seed.snap, v0);
    for o in rest {
        if xs.is_empty() {
            break;
        }
        xs.observe(&o.snap, Relation::Equals(value_of(o, name).unwrap()));
    }
    cands.extend(xs.candidates);
    let after_first = cands.len();

    let mut forms = portable_forms(&seed.snap, &cands);
    for o in obs.iter().filter(|o| o.session != first_s) {
        if let Some(v) = value_of(o, name) {
            forms = confirm(&forms, &o.snap, v);
        }
    }
    finish(name, VarKind::Number, forms, sess.len() as u32, after_first, &seed.snap)
}

// ---------------------------------------------------------------------------
// Position
// ---------------------------------------------------------------------------

/// Whether a reader tracks `axis` over every recorded step of one session.
fn tracks(
    read: impl Fn(&RamSnapshot) -> Option<u32>,
    obs: &[&Observation],
    axis: Axis,
    width: u8,
    min_moves: usize,
) -> bool {
    let m = if width >= 4 { u32::MAX } else { (1u32 << (width * 8)) - 1 };
    let mut moves = 0;
    let mut prev: Option<u32> = None;
    for o in obs {
        let Some(v) = read(&o.snap) else { return false };
        if let (Some(p), Some((dx, dy))) = (prev, moved(o)) {
            let d = if axis == Axis::X { dx } else { dy };
            if v.wrapping_sub(p) & m != d as u32 & m {
                return false;
            }
            if d != 0 {
                moves += 1;
            }
        }
        prev = Some(v);
    }
    moves >= min_moves
}

/// Find the player's X and Y tile coordinates from recorded steps.
pub fn discover_position(obs: &[Observation]) -> Result<(Finding, Finding), String> {
    let sess = sessions(obs);
    let first_s = *sess.first().ok_or("no observations")?;
    let first: Vec<&Observation> = obs.iter().filter(|o| o.session == first_s).collect();
    let snap0 = &first.first().ok_or("no observations")?.snap;
    let mut out = Vec::new();
    for (axis, name) in [(Axis::X, "Player X"), (Axis::Y, "Player Y")] {
        let mut cands = Vec::new();
        for width in [1u8, 2] {
            let s = Search::all(snap0, width);
            cands.extend(s.candidates.into_iter().filter(|c| tracks(|sn| c.read(sn), &first, axis, width, 2)));
        }
        let after_first = cands.len();
        let mut forms = portable_forms(snap0, &cands);
        for s in sess.iter().skip(1) {
            let so: Vec<&Observation> = obs.iter().filter(|o| o.session == *s).collect();
            forms.retain(|p| tracks(|sn| p.read(sn), &so, axis, p.width, 1));
        }
        out.push(finish(name, VarKind::Position { axis }, forms, sess.len() as u32, after_first, snap0)?);
    }
    let y = out.pop().unwrap();
    let x = out.pop().unwrap();
    Ok((x, y))
}

// ---------------------------------------------------------------------------
// Flags
// ---------------------------------------------------------------------------

/// Learn the simplest stump separating `on` values from `off` values.
fn learn_stump(on: &[u32], off: &[u32]) -> Option<FlagRule> {
    if on.is_empty() || off.is_empty() {
        return None;
    }
    let on_set: HashSet<u32> = on.iter().copied().collect();
    let off_set: HashSet<u32> = off.iter().copied().collect();
    if !on_set.is_disjoint(&off_set) {
        return None;
    }
    if off_set.len() == 1 {
        return Some(FlagRule::NotEqual(*off_set.iter().next().unwrap()));
    }
    if on_set.len() == 1 {
        return Some(FlagRule::Equal(*on_set.iter().next().unwrap()));
    }
    if on_set.len() <= 4 {
        let mut v: Vec<u32> = on_set.into_iter().collect();
        v.sort_unstable();
        return Some(FlagRule::OneOf(v));
    }
    None
}

fn rule_size(r: &FlagRule) -> usize {
    match r {
        FlagRule::NotEqual(_) | FlagRule::Equal(_) => 1,
        FlagRule::OneOf(v) => v.len(),
    }
}

/// Learn a yes/no variable (e.g. "text box open") from labeled snapshots.
pub fn discover_flag(obs: &[Observation], name: &str) -> Result<Finding, String> {
    let sess = sessions(obs);
    let first_s = *sess.first().ok_or("no observations")?;
    let train: Vec<(&Observation, bool)> =
        obs.iter().filter(|o| o.session == first_s).filter_map(|o| flag_of(o, name).map(|f| (o, f))).collect();
    if !train.iter().any(|t| t.1) || !train.iter().any(|t| !t.1) {
        return Err(format!("{name}: record it both on and off in the first session"));
    }
    let validated = sess.iter().skip(1).any(|s| {
        let labels: HashSet<bool> = obs.iter().filter(|o| o.session == *s).filter_map(|o| flag_of(o, name)).collect();
        labels.len() == 2
    });
    if !validated {
        return Err(format!("{name}: record it on and off in a second session too, to validate"));
    }
    let snap0 = &train[0].0.snap;

    // Train a stump per byte.
    let mut stumps: Vec<(Candidate, FlagRule)> = Vec::new();
    for c in Search::all(snap0, 1).candidates {
        let (mut on, mut off) = (Vec::new(), Vec::new());
        for (o, f) in &train {
            let v = c.read(&o.snap).unwrap_or(0);
            if *f {
                on.push(v)
            } else {
                off.push(v)
            }
        }
        if let Some(rule) = learn_stump(&on, &off) {
            stumps.push((c, rule));
        }
    }
    let after_first = stumps.len();

    // Generalize, then validate on the held-out sessions.
    let mut kept: Vec<(Portable, FlagRule)> = Vec::new();
    for (c, rule) in stumps {
        for p in portable_forms(snap0, &[c]) {
            let ok = obs.iter().filter(|o| o.session != first_s).all(|o| match flag_of(o, name) {
                Some(f) => p.read(&o.snap).is_some_and(|v| rule.eval(v) == f),
                None => true,
            });
            if ok {
                kept.push((p, rule.clone()));
            }
        }
    }
    // Simplest rule first, then the usual location ranking.
    kept.sort_by_key(|(p, r)| (rule_size(r), p.complexity(), matches!(r, FlagRule::Equal(_)) as u8));
    let Some(best_size) = kept.first().map(|(_, r)| rule_size(r)) else {
        return Err(format!("{name}: no byte predicts it in every session"));
    };
    let mut seen = HashSet::new();
    let mut vars = kept
        .iter()
        .filter(|(_, r)| rule_size(r) == best_size)
        .filter(|(p, _)| seen.insert(p.location.resolve(snap0)))
        .map(|(p, r)| variable(name, VarKind::Flag { rule: r.clone() }, p, false, sess.len() as u32));
    let variable = vars.next().unwrap();
    Ok(Finding { variable, alternatives: vars.collect(), after_first_session: after_first })
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Length of the prefix that must match the ROM.
const ROM_PROBE: usize = 8;
/// Bytes of a text buffer kept in the map.
pub const TEXT_MAX_LEN: u32 = 256;

fn probe_key(b: &[u8]) -> u64 {
    let mut k = [0u8; 8];
    k.copy_from_slice(&b[..ROM_PROBE]);
    u64::from_le_bytes(k)
}

/// Text has variety: at least 5 different byte values in the probe and no
/// short repeating pattern (tile maps and fills like `00 E0 00 E0` also
/// occur in ROMs).
fn looks_like_text(b: &[u8]) -> bool {
    let distinct: HashSet<u8> = b.iter().copied().collect();
    let periodic = |p: usize| b.iter().zip(&b[p..]).all(|(x, y)| x == y);
    distinct.len() >= 5 && !(1..=4).any(periodic)
}

/// Same message -> same probe bytes, different messages -> different bytes.
fn consistent(probes: impl Iterator<Item = (u32, Option<Vec<u8>>)>) -> Option<HashMap<u32, Vec<u8>>> {
    let mut by_msg: HashMap<u32, Vec<u8>> = HashMap::new();
    for (m, b) in probes {
        let b = b?;
        match by_msg.get(&m) {
            Some(prev) if *prev != b => return None,
            Some(_) => {}
            None => {
                by_msg.insert(m, b);
            }
        }
    }
    let distinct: HashSet<&Vec<u8>> = by_msg.values().collect();
    (distinct.len() == by_msg.len()).then_some(by_msg)
}

/// Find the buffer holding the text of the message on screen.
pub fn discover_text(obs: &[Observation], name: &str, rom: &[u8]) -> Result<Finding, String> {
    let sess = sessions(obs);
    let labeled: Vec<(&Observation, u32)> = obs.iter().filter_map(|o| text_of(o, name).map(|m| (o, m))).collect();
    let first_s = *sess.first().ok_or("no observations")?;
    let train: Vec<(&Observation, u32)> = labeled.iter().copied().filter(|(o, _)| o.session == first_s).collect();
    let msgs: HashSet<u32> = train.iter().map(|t| t.1).collect();
    if msgs.len() < 2 {
        return Err(format!("{name}: record at least two different messages in the first session"));
    }
    let snap0 = &train[0].0.snap;
    let probe = |o: &Observation, addr: u32| o.snap.bytes(addr, ROM_PROBE).map(|b| b.to_vec());

    let mut cands = Vec::new();
    for (base, bytes) in snap0.regions() {
        for off in (0..bytes.len().saturating_sub(ROM_PROBE)).step_by(2) {
            let addr = base + off as u32;
            let Some(by_msg) = consistent(train.iter().map(|(o, m)| (*m, probe(o, addr)))) else { continue };
            if by_msg.values().all(|b| looks_like_text(b)) {
                cands.push(addr);
            }
        }
    }
    let after_first = cands.len();

    // Opening bytes must be a ROM string. One pass over the ROM checks all.
    let mut wanted: HashSet<u64> = HashSet::new();
    for &a in &cands {
        for (o, _) in &train {
            if let Some(b) = probe(o, a) {
                wanted.insert(probe_key(&b));
            }
        }
    }
    let mut in_rom: HashSet<u64> = HashSet::new();
    for w in rom.windows(ROM_PROBE) {
        let k = probe_key(w);
        if wanted.contains(&k) {
            in_rom.insert(k);
        }
    }
    cands.retain(|&a| train.iter().all(|(o, _)| probe(o, a).is_some_and(|b| in_rom.contains(&probe_key(&b)))));
    // A string that matches at `a` also matches at a+2, a+4...: keep starts.
    let set: HashSet<u32> = cands.iter().copied().collect();
    cands.retain(|a| !set.contains(&(a - 2)));

    // Generalize and require the same behavior in every other session.
    let as_cands: Vec<Candidate> = cands.iter().map(|&addr| Candidate { addr, width: 1, key: None }).collect();
    let mut forms = portable_forms(snap0, &as_cands);
    for s in sess.iter().skip(1) {
        let so: Vec<(&Observation, u32)> = labeled.iter().copied().filter(|(o, _)| o.session == *s).collect();
        forms.retain(|p| {
            consistent(so.iter().map(|(o, m)| {
                (*m, p.location.resolve(&o.snap).and_then(|a| o.snap.bytes(a, ROM_PROBE)).map(|b| b.to_vec()))
            }))
            .is_some()
        });
    }
    finish(name, VarKind::Text { max_len: TEXT_MAX_LEN }, forms, sess.len() as u32, after_first, snap0)
}

// ---------------------------------------------------------------------------
// Poke and ask
// ---------------------------------------------------------------------------

/// Narrows a number's candidates by experiment: write a test value into half
/// of them, ask the player what the game shows now, keep the half that
/// explains the answer. Each question halves what's left, so a few hundred
/// candidates take about eight questions.
///
/// The caller owns the experiment's safety: save a state before `poke`,
/// show the player the screen, restore after. Candidates that land on the
/// same bytes in this session can't be told apart by poking and are kept
/// together; the best-ranked one wins.
pub struct PokeRefiner {
    pub name: String,
    /// Groups of candidates that share a target, best-ranked first.
    groups: Vec<Vec<Variable>>,
    /// Groups poked by the pending question.
    poked: usize,
    baseline: u32,
    pub questions: u32,
}

impl PokeRefiner {
    /// `mmu` is the session the experiments will run in.
    pub fn new(finding: &Finding, mmu: &super::super::mmu::Mmu) -> Self {
        let mut groups: Vec<Vec<Variable>> = Vec::new();
        let mut index: HashMap<(u32, Option<u32>, u8), usize> = HashMap::new();
        for v in std::iter::once(&finding.variable).chain(&finding.alternatives) {
            let Some(addr) = v.location.resolve_live(mmu) else { continue };
            let key = match v.encoding {
                super::Encoding::Plain => None,
                super::Encoding::Xor { key } => match key.resolve_live(mmu) {
                    Some(k) => Some(k),
                    None => continue,
                },
            };
            let t = (addr, key, v.width);
            match index.get(&t) {
                Some(&i) => groups[i].push(v.clone()),
                None => {
                    index.insert(t, groups.len());
                    groups.push(vec![v.clone()]);
                }
            }
        }
        Self { name: finding.variable.name.clone(), groups, poked: 0, baseline: 0, questions: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.groups.len()
    }

    pub fn done(&self) -> bool {
        self.groups.len() <= 1
    }

    /// The answer, once `done`.
    pub fn result(&self) -> Option<&Variable> {
        self.groups.first().and_then(|g| g.first())
    }

    /// Value the pending experiment writes; the player should see this if
    /// the real variable was among the poked half.
    pub fn test_value(&self) -> u32 {
        if self.baseline & 0xFF != 123 {
            123
        } else {
            77
        }
    }

    /// Write the test value into the first half of the remaining groups.
    /// `shown` is what the game displays right now (before poking).
    pub fn poke(&mut self, mmu: &mut super::super::mmu::Mmu, shown: u32) {
        self.baseline = shown;
        self.poked = self.groups.len().div_ceil(2);
        let t = self.test_value();
        for g in &self.groups[..self.poked] {
            let v = &g[0];
            let (Some(addr), key) = (v.location.resolve_live(mmu), &v.encoding) else { continue };
            let stored = match key {
                super::Encoding::Plain => t,
                super::Encoding::Xor { key } => {
                    let Some(k) = key.resolve_live(mmu) else { continue };
                    let kv = match v.width {
                        1 => mmu.read8(k) as u32,
                        2 => mmu.read16(k) as u32,
                        _ => mmu.read32(k),
                    };
                    t ^ kv
                }
            };
            match v.width {
                1 => mmu.write8(addr, stored as u8),
                2 => mmu.write16(addr, stored as u16),
                _ => mmu.write32(addr, stored),
            }
        }
    }

    /// What the game shows after the poke. Unchanged = the variable is in
    /// the half that wasn't touched.
    pub fn answer(&mut self, shown: u32) {
        self.questions += 1;
        if shown == self.baseline {
            self.groups.drain(..self.poked);
        } else {
            self.groups.truncate(self.poked);
        }
        self.poked = 0;
    }
}

// ---------------------------------------------------------------------------
// Everything
// ---------------------------------------------------------------------------

/// Run every discovery the observations support and build a map. Returns
/// the map plus one report line per variable (or per failure).
pub fn discover_all(obs: &[Observation], rom: &[u8], game_code: &str, title: &str) -> (MemoryMap, Vec<String>) {
    let mut map = MemoryMap::new(game_code, title);
    let mut report = Vec::new();
    let mut add = |r: Result<Finding, String>, map: &mut MemoryMap| match r {
        Ok(f) => {
            report.push(format!(
                "{}: {} ({} candidate(s) after session 1, {} alternative(s))",
                f.variable.name,
                f.variable.describe(),
                f.after_first_session,
                f.alternatives.len()
            ));
            map.set(f.variable);
        }
        Err(e) => report.push(e),
    };

    let mut names = Vec::new();
    let mut flags = Vec::new();
    let mut texts = Vec::new();
    let mut has_moves = false;
    for f in obs.iter().flat_map(|o| &o.facts) {
        match f {
            Fact::Value { var, .. } if !names.contains(var) => names.push(var.clone()),
            Fact::Flag { var, .. } if !flags.contains(var) => flags.push(var.clone()),
            Fact::Text { var, .. } if !texts.contains(var) => texts.push(var.clone()),
            Fact::Moved { .. } => has_moves = true,
            _ => {}
        }
    }
    for n in &names {
        add(discover_number(obs, n), &mut map);
    }
    if has_moves {
        match discover_position(obs) {
            Ok((x, y)) => {
                add(Ok(x), &mut map);
                add(Ok(y), &mut map);
            }
            Err(e) => add(Err(e), &mut map),
        }
    }
    for n in &flags {
        add(discover_flag(obs, n), &mut map);
    }
    for n in &texts {
        add(discover_text(obs, n, rom), &mut map);
    }
    (map, report)
}

#[cfg(test)]
mod tests {
    use super::super::{EWRAM_BASE, EWRAM_SIZE, IWRAM_BASE, IWRAM_SIZE};
    use super::*;

    fn snap() -> RamSnapshot {
        // Busy background so accidental matches exist.
        let mut e = vec![0u8; EWRAM_SIZE];
        let mut x = 0x1234_5678u32;
        for b in e.iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *b = x as u8;
        }
        RamSnapshot { ewram: e.into(), iwram: vec![0; IWRAM_SIZE].into() }
    }

    fn put(s: &mut RamSnapshot, addr: u32, bytes: &[u8]) {
        let (buf, off) = if addr >= IWRAM_BASE {
            (&mut s.iwram, addr - IWRAM_BASE)
        } else {
            (&mut s.ewram, addr - EWRAM_BASE)
        };
        buf[off as usize..off as usize + bytes.len()].copy_from_slice(bytes);
    }

    /// A toy game: a relocating block (pointer at IWRAM 0x100) holds
    /// x/y/hp; a fixed flag byte; a text buffer.
    fn frame(session: u32, x: u16, y: u16, hp: u16, open: bool, msg: Option<&[u8]>, facts: Vec<Fact>) -> Observation {
        let mut s = snap();
        let block = EWRAM_BASE + 0x8000 + session * 0x48;
        put(&mut s, IWRAM_BASE + 0x100, &block.to_le_bytes());
        put(&mut s, block, &x.to_le_bytes());
        put(&mut s, block + 2, &y.to_le_bytes());
        put(&mut s, block + 0x40, &hp.to_le_bytes());
        put(&mut s, EWRAM_BASE + 0x100, &[open as u8]);
        if let Some(m) = msg {
            put(&mut s, EWRAM_BASE + 0x2000, m);
        }
        Observation { session, snap: s, facts }
    }

    #[test]
    fn toy_game_is_fully_discovered() {
        let rom: Vec<u8> = b"....HELLO THERE TRAVELLER\xFF....WELCOME TO TOWN\xFF".to_vec();
        let hello: &[u8] = b"HELLO THERE TRAVELLER\xFF";
        let welcome: &[u8] = b"WELCOME TO TOWN\xFF\0\0\0\0\0\0";
        let v = |n: u32| Fact::Value { var: "HP".into(), value: n };
        let mv = |dx, dy| Fact::Moved { dx, dy };
        let fl = |on| Fact::Flag { var: "Text box".into(), on };
        let tx = |m| Fact::Text { var: "Text".into(), message: m };
        let mut obs = Vec::new();
        for s in 0..2u32 {
            obs.push(frame(s, 10, 20, 29, false, None, vec![v(29), fl(false)]));
            obs.push(frame(s, 11, 20, 29, false, None, vec![mv(1, 0), fl(false)]));
            obs.push(frame(s, 12, 20, 25, false, None, vec![mv(1, 0), v(25)]));
            obs.push(frame(s, 12, 19, 25, true, Some(hello), vec![mv(0, -1), fl(true), tx(1)]));
            obs.push(frame(s, 12, 19, 25, true, Some(hello), vec![mv(0, 0), fl(true), tx(1)]));
            obs.push(frame(s, 12, 18, 25, true, Some(welcome), vec![mv(0, -1), fl(true), tx(2)]));
            obs.push(frame(s, 12, 18, 25, false, Some(welcome), vec![mv(0, 0), fl(false)]));
        }
        let (map, report) = discover_all(&obs, &rom, "TEST", "");
        let r = report.join("\n");
        let ptr = |off| Location::Pointer { base: IWRAM_BASE + 0x100, offset: off };
        assert_eq!(map.get("HP").map(|v| v.location), Some(ptr(0x40)), "{r}");
        assert_eq!(map.get("Player X").map(|v| v.location), Some(ptr(0)), "{r}");
        assert_eq!(map.get("Player Y").map(|v| v.location), Some(ptr(2)), "{r}");
        assert_eq!(map.get("Text box").map(|v| v.location), Some(Location::Absolute { addr: EWRAM_BASE + 0x100 }), "{r}");
        assert_eq!(map.get("Text").map(|v| v.location), Some(Location::Absolute { addr: EWRAM_BASE + 0x2000 }), "{r}");
    }

    #[test]
    fn stump_learner_prefers_simple_rules() {
        assert_eq!(learn_stump(&[1, 2], &[0, 0]), Some(FlagRule::NotEqual(0)));
        assert_eq!(learn_stump(&[5], &[0, 1]), Some(FlagRule::Equal(5)));
        assert_eq!(learn_stump(&[1, 0], &[0]), None);
    }
}
