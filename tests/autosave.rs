//! Auto-save end to end on a real game: states go into the 3-file rotation,
//! stay separate from the manual slots, and restore the exact game state.

use gba_simulator::autosave::AutoSaver;
use gba_simulator::gba::Gba;
use std::time::Duration;

fn emerald() -> Option<Gba> {
    let home = std::env::var("HOME").ok()?;
    let p = std::path::Path::new(&home).join("Downloads/Pokemon - Emerald Version (USA, Europe).gba");
    if !p.exists() {
        eprintln!("skipping: Emerald ROM not found");
        return None;
    }
    let mut g = Gba::new_headless();
    g.load_rom(&p).ok()?;
    Some(g)
}

fn hash(g: &Gba) -> u64 {
    g.get_framebuffer().iter().fold(0xcbf29ce484222325u64, |h, &p| (h ^ p as u64).wrapping_mul(0x100000001b3))
}

#[test]
fn autosave_rotation_restores_the_game() {
    let Some(mut g) = emerald() else { return };
    let dir = std::env::temp_dir().join(format!("crabboy-autosave-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // A manual slot file that must never be touched.
    std::fs::create_dir_all(&dir).unwrap();
    let slot = dir.join("Emerald_slot0.state");
    std::fs::write(&slot, b"manual").unwrap();

    let mut saver = AutoSaver::new(&dir, true, 1);
    let frame = Duration::from_micros(16_667);
    let mut saved = Vec::new(); // (rotation number, frame hash at save, frame index)
    for f in 0..(4 * 60 * 60 + 30) {
        g.run_frame();
        if saver.tick(frame, true) {
            let n = saver.write("Emerald", &g.save_state()).unwrap();
            saved.push((n, hash(&g), f));
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    // Every minute of play: 4 saves into 3 files, the 4th replacing #1.
    assert_eq!(saved.iter().map(|s| s.0).collect::<Vec<_>>(), [1, 2, 3, 1]);
    assert_eq!(std::fs::read(&slot).unwrap(), b"manual", "manual slot untouched");

    // The newest auto-save restores the game exactly: same picture after
    // the next frame as a straight run from that point.
    let newest = &saver.list("Emerald")[0];
    assert_eq!(newest.number, 1);
    let data = saver.read("Emerald", 1).unwrap();
    let mut a = emerald().unwrap();
    assert!(a.load_state(&data));
    let mut b = emerald().unwrap();
    assert!(b.load_state(&data));
    for _ in 0..30 {
        a.run_frame();
        b.run_frame();
    }
    assert_eq!(hash(&a), hash(&b), "restored games run identically");

    // And it continues exactly where the save was made: replaying the
    // original run from the 4th save's frame matches the restored game.
    let mut replay = emerald().unwrap();
    let at = saved[3].2;
    for _ in 0..=at + 30 {
        replay.run_frame();
    }
    assert_eq!(hash(&a), hash(&replay), "restored game matches the original run");
    let _ = std::fs::remove_dir_all(dir);
}
