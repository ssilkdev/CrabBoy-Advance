//! Crash-safe file writes.

use std::io::Write;
use std::path::Path;

/// Replace `path` with `data` so that a reader only ever sees the old
/// contents or the new ones, never a truncated mix.
///
/// A plain `fs::write` truncates the file first. If the process dies
/// mid-write -- which on Android happens whenever the system reclaims a
/// backgrounded app, right after the app has flushed its saves -- the save
/// is left cut short. Writing a temporary file next to it and renaming it
/// over the old one is atomic on every filesystem the emulator runs on.
pub fn write_atomic(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    let (path, data) = (path.as_ref(), data.as_ref());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        // Flush to the device before the rename makes it visible, so a
        // power cut can't leave a renamed but empty file.
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_file_and_leaves_no_temporary() {
        let dir = std::env::temp_dir().join(format!("crabboy-fs-util-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.sav");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second, longer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second, longer");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no .tmp left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_write_keeps_the_old_file() {
        let dir = std::env::temp_dir().join(format!("crabboy-fs-util-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.sav");
        write_atomic(&path, b"old").unwrap();
        // A directory where the temporary file should go makes the write fail.
        std::fs::create_dir(dir.join("game.sav.tmp")).unwrap();
        assert!(write_atomic(&path, b"new").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
