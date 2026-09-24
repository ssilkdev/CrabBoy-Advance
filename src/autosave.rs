//! Auto-save: periodic save states in a rotation of three files, separate
//! from the manual save slots.
//!
//! Every `interval` of *play* time (paused, in a menu or with no game loaded
//! doesn't count) the front-end writes a state to the next file in the
//! rotation: the first empty one, otherwise the oldest. Files live next to
//! the manual slots as `<game>_auto1.state` .. `<game>_auto3.state`, so they
//! survive restarts. Writes go through a temp file + rename, so a crash
//! mid-write never leaves a broken auto-save behind.
//!
//! Shared by the desktop and Android apps; pure logic, host-tested.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Number of auto-save files kept per game.
pub const ROTATION: usize = 3;
pub const DEFAULT_INTERVAL_MINUTES: u32 = 5;
pub const MIN_INTERVAL_MINUTES: u32 = 1;
pub const MAX_INTERVAL_MINUTES: u32 = 60;

/// One auto-save on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoSaveEntry {
    /// Position in the rotation, 1-based (matches the file name).
    pub number: usize,
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size_bytes: u64,
}

impl AutoSaveEntry {
    /// "Just now", "4 min ago", "2 h ago", "3 days ago".
    pub fn age_label(&self) -> String {
        let secs = SystemTime::now().duration_since(self.modified).unwrap_or_default().as_secs();
        match secs {
            0..=59 => "Just now".into(),
            60..=3599 => format!("{} min ago", secs / 60),
            3600..=86_399 => format!("{} h ago", secs / 3600),
            _ => format!("{} days ago", secs / 86_400),
        }
    }
}

/// File-name stem for a game: letters, digits, `-` and `_` kept, the rest
/// replaced with `_` (same rule as the manual slots).
pub fn game_key(rom_name: &str) -> String {
    rom_name.chars().map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// Clamp an interval to the allowed range.
pub fn clamp_minutes(m: u32) -> u32 {
    m.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES)
}

/// Timer + rotation for one saves directory.
#[derive(Debug, Clone)]
pub struct AutoSaver {
    dir: PathBuf,
    pub enabled: bool,
    interval_minutes: u32,
    /// Play time since the last auto-save (or since the game started).
    played: Duration,
}

impl AutoSaver {
    pub fn new(dir: impl Into<PathBuf>, enabled: bool, interval_minutes: u32) -> Self {
        Self { dir: dir.into(), enabled, interval_minutes: clamp_minutes(interval_minutes), played: Duration::ZERO }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn interval_minutes(&self) -> u32 {
        self.interval_minutes
    }

    pub fn set_interval_minutes(&mut self, m: u32) {
        self.interval_minutes = clamp_minutes(m);
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_minutes as u64 * 60)
    }

    /// Play time left until the next auto-save.
    pub fn time_until_next(&self) -> Duration {
        self.interval().saturating_sub(self.played)
    }

    /// Start counting from zero (a game was loaded, or a state restored).
    pub fn restart_timer(&mut self) {
        self.played = Duration::ZERO;
    }

    /// Advance the timer by `dt` of real time. Only counts while `playing`.
    /// Returns true when an auto-save is due; the caller then saves with
    /// [`AutoSaver::write`] (the timer restarts here either way, so a failed
    /// write retries after one interval instead of every frame).
    pub fn tick(&mut self, dt: Duration, playing: bool) -> bool {
        if !self.enabled || !playing {
            return false;
        }
        // Ignore huge gaps (machine asleep, app in the background).
        self.played += dt.min(Duration::from_secs(1));
        if self.played >= self.interval() {
            self.played = Duration::ZERO;
            return true;
        }
        false
    }

    /// Path of auto-save `number` (1..=ROTATION) for a game.
    pub fn path(&self, game: &str, number: usize) -> PathBuf {
        self.dir.join(format!("{}_auto{}.state", game_key(game), number))
    }

    /// Existing auto-saves for a game, newest first.
    pub fn list(&self, game: &str) -> Vec<AutoSaveEntry> {
        let mut v: Vec<AutoSaveEntry> = (1..=ROTATION)
            .filter_map(|n| {
                let path = self.path(game, n);
                let meta = std::fs::metadata(&path).ok()?;
                Some(AutoSaveEntry { number: n, modified: meta.modified().ok()?, size_bytes: meta.len(), path })
            })
            .collect();
        v.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.number.cmp(&a.number)));
        v
    }

    /// Rotation number the next auto-save goes to: the first empty file,
    /// otherwise the oldest one.
    pub fn next_number(&self, game: &str) -> usize {
        if let Some(n) = (1..=ROTATION).find(|&n| !self.path(game, n).exists()) {
            return n;
        }
        self.list(game).last().map(|e| e.number).unwrap_or(1)
    }

    /// Write a state into the rotation. Returns the rotation number used.
    pub fn write(&self, game: &str, state: &[u8]) -> std::io::Result<usize> {
        std::fs::create_dir_all(&self.dir)?;
        let n = self.next_number(game);
        let path = self.path(game, n);
        let tmp = path.with_extension("state.tmp");
        std::fs::write(&tmp, state)?;
        std::fs::rename(&tmp, &path)?;
        Ok(n)
    }

    /// Read auto-save `number`.
    pub fn read(&self, game: &str, number: usize) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.path(game, number))
    }

    /// Delete every auto-save for a game.
    pub fn clear(&self, game: &str) {
        for n in 1..=ROTATION {
            let _ = std::fs::remove_file(self.path(game, n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("crabboy-autosave-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Filesystems differ in timestamp precision; space writes out so
    /// "oldest" is unambiguous.
    fn pause() {
        std::thread::sleep(Duration::from_millis(20));
    }

    #[test]
    fn default_is_five_minutes_and_minimum_is_one() {
        let a = AutoSaver::new("x", true, DEFAULT_INTERVAL_MINUTES);
        assert_eq!(a.interval(), Duration::from_secs(300));
        let mut a = AutoSaver::new("x", true, 0);
        assert_eq!(a.interval_minutes(), 1, "clamped up to 1 minute");
        a.set_interval_minutes(1000);
        assert_eq!(a.interval_minutes(), MAX_INTERVAL_MINUTES);
    }

    #[test]
    fn fires_after_interval_of_play_time_only() {
        let mut a = AutoSaver::new("x", true, 1);
        let frame = Duration::from_micros(16_667);
        let mut fired = 0;
        // 30 s paused: nothing counts.
        for _ in 0..1800 {
            fired += a.tick(frame, false) as u32;
        }
        assert_eq!(fired, 0);
        assert_eq!(a.time_until_next(), Duration::from_secs(60));
        // 59 s of play: not yet.
        for _ in 0..(59 * 60) {
            fired += a.tick(frame, true) as u32;
        }
        assert_eq!(fired, 0);
        // Past 60 s: exactly once, then the timer restarts.
        for _ in 0..90 {
            fired += a.tick(frame, true) as u32;
        }
        assert_eq!(fired, 1);
        assert!(a.time_until_next() > Duration::from_secs(55));
    }

    #[test]
    fn disabled_never_fires_and_long_gaps_are_capped() {
        let mut a = AutoSaver::new("x", false, 1);
        assert!(!a.tick(Duration::from_secs(3600), true));
        a.enabled = true;
        // Waking from sleep after an hour isn't an hour of play.
        assert!(!a.tick(Duration::from_secs(3600), true));
    }

    #[test]
    fn rotation_fills_three_then_overwrites_the_oldest() {
        let dir = tmpdir("rotate");
        let a = AutoSaver::new(&dir, true, 5);
        let game = "Pokemon - Emerald Version (USA, Europe)";
        let mut used = Vec::new();
        for i in 0..7u8 {
            used.push(a.write(game, &[i]).unwrap());
            pause();
        }
        assert_eq!(used, [1, 2, 3, 1, 2, 3, 1]);
        // Newest first: the last three writes.
        let list = a.list(game);
        assert_eq!(list.len(), ROTATION);
        let contents: Vec<u8> = list.iter().map(|e| a.read(game, e.number).unwrap()[0]).collect();
        assert_eq!(contents, [6, 5, 4]);
        assert_eq!(list[0].number, 1);
        // Separate from manual slots and other games.
        assert!(a.path(game, 1).file_name().unwrap().to_string_lossy().ends_with("_auto1.state"));
        assert!(a.list("Other game").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn persists_across_restarts() {
        let dir = tmpdir("persist");
        let game = "Game";
        {
            let a = AutoSaver::new(&dir, true, 5);
            a.write(game, b"one").unwrap();
            pause();
            a.write(game, b"two").unwrap();
        }
        // A new instance (app restart) sees them and continues the rotation.
        let a = AutoSaver::new(&dir, true, 5);
        let list = a.list(game);
        assert_eq!(list.len(), 2);
        assert_eq!(a.read(game, list[0].number).unwrap(), b"two");
        assert_eq!(a.next_number(game), 3);
        pause();
        a.write(game, b"three").unwrap();
        pause();
        assert_eq!(a.write(game, b"four").unwrap(), 1, "oldest (\"one\") replaced");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_deleted_file_is_refilled_first() {
        let dir = tmpdir("gap");
        let a = AutoSaver::new(&dir, true, 5);
        for i in 0..3u8 {
            a.write("g", &[i]).unwrap();
            pause();
        }
        std::fs::remove_file(a.path("g", 2)).unwrap();
        assert_eq!(a.write("g", &[9]).unwrap(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
