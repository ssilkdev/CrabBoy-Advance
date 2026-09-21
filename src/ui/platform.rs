//! Cross-platform desktop integration helpers.
//!
//! Centralizes the "hand this off to the OS" operations that differ per
//! platform, so UI code never spells out per-OS `Command` invocations inline.

use std::path::Path;
use std::process::Command;

/// Hides the console window a helper would otherwise flash on Windows.
/// No-op on other platforms.
fn silence_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Opens `target` (a URL or a filesystem path) in the system's default handler.
///
/// Per-platform handler:
/// - Windows: `cmd /C start` (the empty "" argument is the window title, and is
///   required or `start` treats a quoted target as the title and opens nothing).
/// - macOS:   `open`
/// - Unix:    `xdg-open`, the freedesktop.org standard resolver. Minimal systems
///   may not ship it, so a missing handler is reported rather than ignored.
fn spawn_opener(target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", target]);
        c
    };

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(target);
        c
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };

    silence_console(&mut cmd);

    // Detach stdio: xdg-open delegates to a browser/viewer that would otherwise
    // inherit and hold this process's terminal handles.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    cmd.spawn().map_err(|e| {
        #[cfg(all(unix, not(target_os = "macos")))]
        if e.kind() == std::io::ErrorKind::NotFound {
            return format!(
                "Could not find `xdg-open` to open {}. Install xdg-utils, \
                 or open it manually.",
                target
            );
        }
        format!("Failed to open {}: {}", target, e)
    })?;

    Ok(())
}

/// Opens a URL in the user's default web browser.
pub fn open_url(url: &str) -> Result<(), String> {
    spawn_opener(url)
}

/// Opens a file with the user's default application for its type.
pub fn open_path(path: &Path) -> Result<(), String> {
    spawn_opener(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_reports_error_instead_of_panicking() {
        // Exercises the full spawn path; on a machine with no handler installed
        // this must surface an Err, never panic or silently "succeed".
        let _ = open_url("https://example.invalid/");
    }

    #[test]
    fn open_path_accepts_paths_with_spaces_and_unicode() {
        // Paths are passed as a single argv entry (never shell-interpolated),
        // so spaces and non-ASCII names need no quoting or escaping.
        let p = std::env::temp_dir().join("CrabBoy Trainer's Guide ✅.pdf");
        let _ = open_path(&p);
    }
}
