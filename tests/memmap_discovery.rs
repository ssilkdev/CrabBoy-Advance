//! ROADMAP M11: automatic game-variable discovery on Pokémon Emerald.
//!
//! Needs an Emerald ROM + its .sav (skips without): `$CRABBOY_EMERALD_DIR`
//! or `~/Downloads`, files `Pokemon - Emerald Version (USA, Europe).{gba,sav}`.
//! The save must be standing on Route 104 north of Petalburg (the one this
//! was written against); other saves skip with a message.
//!
//! Each *session* is a fresh boot with a different number of idle frames
//! before CONTINUE, which makes Emerald place its save blocks (and money
//! encryption key) differently, so answers must survive relocation.
//!
//! The scripted player labels each snapshot with what the screen shows: HP
//! and money from the HUD, how many tiles it walked, whether a text box is
//! open, which message it is. The script reads those numbers through
//! `Oracle` (known Gen 3 addresses) purely to stand in for a person reading
//! the screen; discovery never sees an address. The oracle is used again at
//! the end to grade what discovery found.

use std::path::PathBuf;

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::memmap::discover::{discover_all, discover_number, Fact, Observation, PokeRefiner};
use gba_simulator::gba::memmap::{MemoryMap, RamSnapshot, Value, VarKind};
use gba_simulator::gba::Gba;

fn emerald_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("CRABBOY_EMERALD_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")))?;
    let rom = dir.join("Pokemon - Emerald Version (USA, Europe).gba");
    let sav = dir.join("Pokemon - Emerald Version (USA, Europe).sav");
    (rom.exists() && sav.exists()).then_some(dir)
}

/// Known Emerald (BPEE) locations, used only to label and to grade.
struct Oracle;
impl Oracle {
    fn sb1(g: &Gba) -> u32 {
        g.mmu.read32(0x0300_5D8C)
    }
    fn pos(g: &Gba) -> (i32, i32) {
        let s = Self::sb1(g);
        (g.mmu.read16(s) as i16 as i32, g.mmu.read16(s + 2) as i16 as i32)
    }
    fn hp(g: &Gba) -> u32 {
        g.mmu.read16(0x0202_44EC + 0x56) as u32
    }
    fn money(g: &Gba) -> u32 {
        let key = g.mmu.read32(g.mmu.read32(0x0300_5D90) + 0xAC);
        g.mmu.read32(Self::sb1(g) + 0x490) ^ key
    }
    fn in_battle(g: &Gba) -> bool {
        g.mmu.read16(0x0202_4084 + 0x28) != 0
    }
}

struct Player {
    g: Gba,
    session: u32,
    obs: Vec<Observation>,
    last_pos: (i32, i32),
}

impl Player {
    fn boot(dir: &PathBuf, session: u32, idle: usize) -> Self {
        let mut g = Gba::new_headless();
        g.load_rom(dir.join("Pokemon - Emerald Version (USA, Europe).gba")).unwrap();
        let mut p = Player { g, session, obs: Vec::new(), last_pos: (0, 0) };
        p.idle(idle);
        for _ in 0..6 {
            p.press(Key::Start, 4);
            p.idle(40);
        }
        for _ in 0..6 {
            p.press(Key::A, 4);
            p.idle(40);
        }
        p.idle(200);
        p.last_pos = Oracle::pos(&p.g);
        p
    }

    fn idle(&mut self, n: usize) {
        for _ in 0..n {
            self.g.run_frame();
        }
    }

    fn press(&mut self, k: Key, n: usize) {
        self.g.mmu.keypad.set_key_state(k, true);
        self.idle(n);
        self.g.mmu.keypad.set_key_state(k, false);
    }

    fn walk(&mut self, k: Key) {
        self.press(k, 16);
        self.idle(8);
    }

    /// Record what the screen shows now.
    fn look(&mut self, mut facts: Vec<Fact>, walked: bool) {
        let pos = Oracle::pos(&self.g);
        if walked {
            facts.push(Fact::Moved { dx: pos.0 - self.last_pos.0, dy: pos.1 - self.last_pos.1 });
        }
        self.last_pos = pos;
        facts.push(Fact::Value { var: "HP".into(), value: Oracle::hp(&self.g) });
        if !Oracle::in_battle(&self.g) {
            facts.push(Fact::Value { var: "Money".into(), value: Oracle::money(&self.g) });
        }
        self.obs.push(Observation { session: self.session, snap: RamSnapshot::capture(&self.g.mmu), facts });
    }
}

fn text_box(on: bool) -> Fact {
    Fact::Flag { var: "Text box open".into(), on }
}
fn message(n: u32) -> Fact {
    Fact::Text { var: "Text box text".into(), message: n }
}

/// Walking around, a sign, the start menu and the save prompt.
fn overworld_tour(p: &mut Player) {
    p.look(vec![text_box(false)], true);
    for k in [Key::Right, Key::Left, Key::Left, Key::Up, Key::Up, Key::Down, Key::Right] {
        p.walk(k);
        p.look(vec![text_box(false)], true);
    }
    // Back at the start tile, facing down; the sign is one tile down-left.
    p.walk(Key::Left);
    p.look(vec![text_box(false)], true);
    p.press(Key::Down, 3);
    p.idle(10);
    p.press(Key::A, 4);
    p.idle(60);
    p.look(vec![text_box(true), message(1)], true);
    p.idle(40);
    p.look(vec![text_box(true), message(1)], true);
    p.press(Key::A, 4);
    p.idle(40);
    p.look(vec![text_box(false)], true);
    // Start menu (a menu, not a text box).
    p.press(Key::Start, 4);
    p.idle(30);
    p.look(vec![text_box(false)], true);
    for _ in 0..4 {
        p.press(Key::Down, 4);
        p.idle(8);
    }
    p.press(Key::A, 4);
    p.idle(60);
    p.look(vec![text_box(true), message(2)], true);
    p.press(Key::B, 4);
    p.idle(60);
    p.press(Key::B, 4);
    p.idle(40);
    p.look(vec![text_box(false)], true);
}

/// Walk into the grass north-west and fight whatever shows up until the
/// lead Pokémon has been hit.
fn battle(p: &mut Player) -> bool {
    // Up to the ledge row first, then left, then up into the grass (25,59).
    for (target, axis) in [(62, 1), (25, 0), (59, 1)] {
        for _ in 0..12 {
            let pos = Oracle::pos(&p.g);
            let cur = if axis == 0 { pos.0 } else { pos.1 };
            if cur == target {
                break;
            }
            let k = match (axis, cur < target) {
                (0, true) => Key::Right,
                (0, false) => Key::Left,
                (_, true) => Key::Down,
                (_, false) => Key::Up,
            };
            p.walk(k);
        }
    }
    assert_eq!(Oracle::pos(&p.g), (25, 59), "didn't reach the grass");
    // Encounter -> mash FIGHT/first move/through text until our HP drops.
    // A battle we win without being hit just leads to the next encounter.
    let hp0 = Oracle::hp(&p.g);
    for _ in 0..40 {
        let before = Oracle::hp(&p.g);
        for _ in 0..30 {
            p.walk(Key::Right);
            p.walk(Key::Left);
            if Oracle::hp(&p.g) != before || p.g.mmu.read8(0x0300_22C0 + 0x438) != 0 {
                break;
            }
        }
        for _ in 0..60 {
            p.press(Key::A, 4);
            p.idle(30);
            if Oracle::hp(&p.g) < hp0 {
                p.idle(120);
                p.look(vec![], false);
                return true;
            }
        }
    }
    let _ = p.g.dump_frame_png(std::env::temp_dir().join("m11_nobattle.png"));
    false
}

/// Grade a found variable against the oracle over every observation.
fn check(map: &MemoryMap, name: &str, sessions: &[Player], truth: impl Fn(&Gba) -> i64) -> Result<String, String> {
    let v = map.get(name).ok_or(format!("{name}: not found"))?;
    let mut offsets = Vec::new();
    for p in sessions {
        let got = match v.read_live(&p.g.mmu) {
            Some(Value::Number(n)) => n,
            other => return Err(format!("{name}: read {other:?}")),
        };
        offsets.push(got - truth(&p.g));
    }
    // Position may be found in a copy that's offset by a constant (object
    // event coordinates are +7): the difference must be the same everywhere.
    if offsets.windows(2).all(|w| w[0] == w[1]) {
        Ok(format!("{name} = {} (offset {})", v.describe(), offsets[0]))
    } else {
        Err(format!("{name} = {} disagrees with the game: offsets {offsets:?}", v.describe()))
    }
}

#[test]
fn emerald_variables_are_discovered_and_the_map_is_reused() {
    let Some(dir) = emerald_dir() else {
        eprintln!("skipping: no Emerald ROM + save");
        return;
    };
    let rom = std::fs::read(dir.join("Pokemon - Emerald Version (USA, Europe).gba")).unwrap();

    // Session 0 fights; sessions 1 and 2 only tour.
    let mut players = Vec::new();
    for (s, idle) in [(0u32, 300usize), (1, 317), (2, 343)] {
        let mut p = Player::boot(&dir, s, idle);
        if Oracle::pos(&p.g) != (28, 65) {
            eprintln!("skipping: save isn't at Route 104 (28,65): {:?}", Oracle::pos(&p.g));
            return;
        }
        overworld_tour(&mut p);
        if s == 0 {
            assert!(battle(&mut p), "no battle with damage taken");
        }
        players.push(p);
    }
    let sb1s: Vec<u32> = players.iter().map(|p| Oracle::sb1(&p.g)).collect();
    assert!(sb1s[0] != sb1s[1] && sb1s[1] != sb1s[2], "sessions should relocate the save block: {sb1s:X?}");

    let obs: Vec<Observation> = players.iter().flat_map(|p| p.obs.clone()).collect();
    let t = std::time::Instant::now();
    let (mut map, report) = discover_all(&obs, &rom, "BPEE", "POKEMON EMER");
    let elapsed = t.elapsed();
    println!("{} observations, discovery took {:.2?}", obs.len(), elapsed);
    for line in &report {
        println!("  {line}");
    }

    // Money never changes during the tour, so a few other constant words
    // (a copy, a cached value) fit too. Narrow them by experiment: poke
    // half, open the trainer card, read what it says.
    let money = discover_number(&obs, "Money").unwrap();
    if !money.is_unique() {
        let p = &mut players[0];
        let mut r = PokeRefiner::new(&money, &p.g.mmu);
        let start = r.remaining();
        while !r.done() {
            let before = p.g.save_state();
            let shown = Oracle::money(&p.g);
            r.poke(&mut p.g.mmu, shown);
            // What the player reads off the trainer card with the test
            // value in place.
            p.press(Key::Start, 4);
            p.idle(30);
            for _ in 0..3 {
                p.press(Key::Down, 4);
                p.idle(8);
            }
            p.press(Key::A, 4);
            p.idle(120);
            let card = Oracle::money(&p.g);
            r.answer(card);
            p.g.load_state(&before);
            assert!(r.questions < 20);
        }
        println!("  Money narrowed by poking: {start} -> 1 in {} question(s)", r.questions);
        map.set(r.result().unwrap().clone());
    }

    // Grade: a fresh session nobody trained on, plus the three used.
    let mut fresh = Player::boot(&dir, 9, 390);
    fresh.walk(Key::Right);
    fresh.walk(Key::Up);
    let mut all = players;
    all.push(fresh);
    let mut failures = Vec::new();
    for (name, f) in [
        ("HP", &(|g: &Gba| Oracle::hp(g) as i64) as &dyn Fn(&Gba) -> i64),
        ("Money", &|g: &Gba| Oracle::money(g) as i64),
        ("Player X", &|g: &Gba| Oracle::pos(g).0 as i64),
        ("Player Y", &|g: &Gba| Oracle::pos(g).1 as i64),
    ] {
        match check(&map, name, &all, f) {
            Ok(s) => println!("  OK {s}"),
            Err(e) => failures.push(e),
        }
    }
    // Text box: must be off in the fresh session and on while its sign
    // is up; the text must be the sign's ROM string.
    match map.get("Text box open") {
        Some(v) => println!("  text box flag = {} {:?}", v.describe(), v.kind),
        None => failures.push("Text box open: not found".into()),
    }
    let fresh = all.last_mut().unwrap();
    let flag = |g: &Gba| map.read_live("Text box open", &g.mmu);
    if flag(&fresh.g) != Some(Value::Flag(false)) {
        failures.push(format!("text box flag in fresh session while walking: {:?}", flag(&fresh.g)));
    }
    fresh.walk(Key::Down);
    fresh.walk(Key::Left);
    fresh.walk(Key::Left);
    fresh.press(Key::Down, 3);
    fresh.idle(10);
    fresh.press(Key::A, 4);
    fresh.idle(60);
    if flag(&fresh.g) != Some(Value::Flag(true)) {
        failures.push(format!("text box flag with the sign open: {:?}", flag(&fresh.g)));
    }
    match map.read_live("Text box text", &fresh.g.mmu) {
        Some(Value::Text(t)) => {
            let head = &t[..12];
            let in_rom = rom.windows(12).any(|w| w == head);
            println!("  text box text = {} head {:02X?} (in ROM: {in_rom})", map.get("Text box text").unwrap().describe(), head);
            if !in_rom {
                failures.push("text buffer doesn't hold a ROM string in the fresh session".into());
            }
        }
        other => failures.push(format!("Text box text: {other:?}")),
    }

    // Save, load, and check the loaded map reads the same.
    let tmp = std::env::temp_dir().join(format!("crabboy-m11-{}", std::process::id()));
    map.save(&tmp).unwrap();
    let loaded = MemoryMap::load(&tmp, "BPEE", "POKEMON EMER").expect("map reloads");
    assert_eq!(loaded, map);
    for v in &loaded.variables {
        assert_eq!(v.read_live(&fresh.g.mmu), map.get(&v.name).unwrap().read_live(&fresh.g.mmu));
        assert!(!matches!(v.kind, VarKind::Text { .. }) || v.sessions_confirmed == 3);
    }
    let _ = std::fs::remove_dir_all(tmp);

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
