//! Automatic GitHub Release Checker & Self-Updater for CrabBoy Advance
//!
//! Features:
//! - Asynchronous, non-blocking startup check against GitHub Releases API.
//! - Semver comparison between currently running binary and latest GitHub tag.
//! - Live download tracking of the platform's release asset.
//! - Atomic executable replacement (.old swap) and automatic restart.
//! - Startup cleanup of legacy .old executable files.
//!
//! Platform notes: the release asset name, the curl binary location, and the
//! post-download `chmod +x` step all differ per OS and are isolated behind the
//! `EXE_ASSET_NAME`, `curl_path()`, and `mark_executable()` helpers below.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const REPO_OWNER: &str = "ssilkdev";
pub const REPO_NAME: &str = "CrabBoy-Advance";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECKSUMS_ASSET_NAME: &str = "SHA256SUMS.txt";

/// Release asset this build knows how to install over itself. Each target OS
/// publishes a distinctly named artifact, so a Linux build must never try to
/// install a Windows `.exe` over its own running binary.
#[cfg(target_os = "windows")]
pub const EXE_ASSET_NAME: &str = "crabboy-advance.exe";
#[cfg(target_os = "linux")]
pub const EXE_ASSET_NAME: &str = "crabboy-advance-linux-x86_64";
#[cfg(target_os = "macos")]
pub const EXE_ASSET_NAME: &str = "crabboy-advance-macos-universal";

/// Resolves the curl executable.
///
/// On Windows this pins the absolute path to the OS-shipped `curl.exe` rather
/// than trusting whatever "curl.exe" resolves to first on PATH. On Unix, curl
/// lives in a root-owned directory already on PATH, so a bare name is fine.
fn curl_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(system_root) = std::env::var("SystemRoot") {
            let candidate = PathBuf::from(system_root).join("System32").join("curl.exe");
            if candidate.exists() {
                return candidate;
            }
        }
        return PathBuf::from("curl.exe");
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("curl")
    }
}

/// Hides the console window curl would otherwise flash on Windows. No-op elsewhere.
fn silence_console(cmd: &mut std::process::Command) {
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

/// Returns a sibling path of the running binary with `suffix` appended to its
/// file name (e.g. `crabboy-advance` -> `crabboy-advance.new`).
///
/// This appends rather than using `Path::with_extension`, which would mangle
/// Unix binaries that have no extension at all.
fn sibling_with_suffix(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    exe.with_file_name(name)
}

/// Restores the executable bit on a freshly downloaded binary.
///
/// curl writes the file with the default 0644 mode, so on Unix the downloaded
/// update would be non-executable and the post-update restart would fail with
/// EACCES. Windows infers executability from the file extension instead.
fn mark_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| format!("Cannot stat downloaded update: {}", e))?
            .permissions();
        perms.set_mode(perms.mode() | 0o755);
        std::fs::set_permissions(path, perms)
            .map_err(|e| format!("Cannot mark update executable: {}", e))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Computes the lowercase hex SHA-256 digest of a file's contents.
fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read {:?} for checksum: {}", path, e))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{:02x}", b)).collect())
}

/// Parses a `SHA256SUMS.txt`-style file (`<hex digest>  <filename>` per line,
/// as produced by `sha256sum`/`Get-FileHash`) and returns the digest for `name`.
fn find_checksum(sums_text: &str, name: &str) -> Option<String> {
    for line in sums_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, |c: char| c.is_whitespace());
        let digest = parts.next()?.trim();
        let file = parts.next()?.trim().trim_start_matches('*');
        if file.eq_ignore_ascii_case(name) && digest.len() == 64 {
            return Some(digest.to_lowercase());
        }
    }
    None
}

/// Downloads a small text asset (like SHA256SUMS.txt) via curl and returns its contents.
fn fetch_text_asset(url: &str) -> Result<String, String> {
    let mut cmd = std::process::Command::new(curl_path());
    cmd.args(&[
        "-s",
        "-L",
        "--max-time", "20",
        "-H", "User-Agent: CrabBoy-Advance-Updater",
        url,
    ]);

    silence_console(&mut cmd);

    let output = cmd.output().map_err(|e| format!("Failed to fetch checksums (curl error): {}", e))?;
    if !output.status.success() {
        return Err(format!("Checksum download returned status: {:?}", output.status.code()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub version: String,
    pub title: String,
    pub body: String,
    pub html_url: String,
    pub published_at: String,
    pub exe_download_url: Option<String>,
    pub exe_size: u64,
    pub zip_download_url: Option<String>,
    pub zip_size: u64,
    pub checksums_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate {
        version: String,
        checked_at: Instant,
    },
    UpdateAvailable {
        current: String,
        latest: ReleaseInfo,
    },
    Downloading {
        progress: f32,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    DownloadedReadyToRestart {
        new_exe_path: PathBuf,
        latest: ReleaseInfo,
        expected_sha256: String,
    },
    Failed(String),
}

#[derive(Clone)]
pub struct UpdateManager {
    pub status: Arc<Mutex<UpdateStatus>>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateManager {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(UpdateStatus::Idle)),
        }
    }

    /// Clean up any leftover `.old` executable from a previous update.
    pub fn cleanup_old_exe() {
        if let Ok(current_exe) = std::env::current_exe() {
            let old_exe = sibling_with_suffix(&current_exe, ".old");
            if old_exe.exists() {
                let _ = std::fs::remove_file(old_exe);
            }
            let new_exe = sibling_with_suffix(&current_exe, ".new");
            if new_exe.exists() {
                // If an aborted .new exists from an incomplete download, clean it up
                let _ = std::fs::remove_file(new_exe);
            }
        }
    }

    /// Triggers an asynchronous, non-blocking check for updates on GitHub.
    pub fn check_for_updates_async(&self) {
        let status_clone = Arc::clone(&self.status);

        // Don't recheck if currently checking or downloading
        {
            let current = status_clone.lock().unwrap_or_else(|e| e.into_inner());
            match *current {
                UpdateStatus::Checking | UpdateStatus::Downloading { .. } => return,
                _ => {}
            }
        }

        *status_clone.lock().unwrap_or_else(|e| e.into_inner()) = UpdateStatus::Checking;

        std::thread::spawn(move || {
            match fetch_latest_release(REPO_OWNER, REPO_NAME) {
                Ok(release) => {
                    let is_update = is_newer(&release.version, CURRENT_VERSION);
                    let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                    if is_update {
                        *lock = UpdateStatus::UpdateAvailable {
                            current: CURRENT_VERSION.to_string(),
                            latest: release,
                        };
                    } else {
                        *lock = UpdateStatus::UpToDate {
                            version: release.version,
                            checked_at: Instant::now(),
                        };
                    }
                }
                Err(err) => {
                    let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                    *lock = UpdateStatus::Failed(err);
                }
            }
        });
    }

    /// Spawns a background thread to download the latest `crabboy-advance.exe` asset.
    pub fn download_and_install_async(&self) {
        let status_clone = Arc::clone(&self.status);

        let release_info = {
            let lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
            match &*lock {
                UpdateStatus::UpdateAvailable { latest, .. } => latest.clone(),
                _ => return,
            }
        };

        let exe_url = match &release_info.exe_download_url {
            Some(url) => url.clone(),
            None => {
                let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                *lock = UpdateStatus::Failed("No crabboy-advance.exe binary asset found in latest release.".to_string());
                return;
            }
        };

        *status_clone.lock().unwrap_or_else(|e| e.into_inner()) = UpdateStatus::Downloading {
            progress: 0.0,
            downloaded_bytes: 0,
            total_bytes: release_info.exe_size,
        };

        std::thread::spawn(move || {
            let current_exe = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                    *lock = UpdateStatus::Failed(format!("Cannot determine current exe location: {}", e));
                    return;
                }
            };

            let temp_download = sibling_with_suffix(&current_exe, ".new");
            let _ = std::fs::remove_file(&temp_download);

            // Execute curl with silent flags (and CREATE_NO_WINDOW on Windows)
            let mut cmd = std::process::Command::new(curl_path());
            cmd.args(&[
                "-L",
                "-f",
                "--max-time", "180",
                "-H", "User-Agent: CrabBoy-Advance-Updater",
                "-o", &temp_download.to_string_lossy(),
                &exe_url,
            ]);

            silence_console(&mut cmd);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                    *lock = UpdateStatus::Failed(format!("Failed to start curl download: {}", e));
                    return;
                }
            };

            // Monitor download progress
            let total_bytes = release_info.exe_size;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if status.success() {
                            if temp_download.exists() {
                                let file_size = std::fs::metadata(&temp_download).map(|m| m.len()).unwrap_or(0);
                                if file_size > 100_000 {
                                    // Verify integrity against the release's published SHA256SUMS.txt
                                    // before ever treating this binary as trusted. This is the
                                    // primary defense against a corrupted download, a MITM'd
                                    // transfer, or a tampered release asset.
                                    match verify_download_checksum(&temp_download, &release_info) {
                                        Ok(expected_sha256) => {
                                            // Restore the executable bit before the
                                            // binary is ever handed to the installer:
                                            // curl wrote it 0644, so on Unix the
                                            // post-update restart would hit EACCES.
                                            if let Err(e) = mark_executable(&temp_download) {
                                                let _ = std::fs::remove_file(&temp_download);
                                                let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                                                *lock = UpdateStatus::Failed(e);
                                                return;
                                            }
                                            let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                                            *lock = UpdateStatus::DownloadedReadyToRestart {
                                                new_exe_path: temp_download,
                                                latest: release_info,
                                                expected_sha256,
                                            };
                                        }
                                        Err(e) => {
                                            let _ = std::fs::remove_file(&temp_download);
                                            let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                                            *lock = UpdateStatus::Failed(format!(
                                                "Update verification failed, refusing to install: {}",
                                                e
                                            ));
                                        }
                                    }
                                    return;
                                }
                            }
                            let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                            *lock = UpdateStatus::Failed("Downloaded binary is corrupt or too small.".to_string());
                        } else {
                            let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                            *lock = UpdateStatus::Failed(format!("Download failed with exit code: {:?}", status.code()));
                        }
                        return;
                    }
                    Ok(None) => {
                        // Still running: update progress
                        let downloaded = std::fs::metadata(&temp_download).map(|m| m.len()).unwrap_or(0);
                        let progress = if total_bytes > 0 {
                            (downloaded as f32 / total_bytes as f32).clamp(0.0, 0.99)
                        } else {
                            0.5
                        };
                        {
                            let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                            if let UpdateStatus::Downloading { .. } = *lock {
                                *lock = UpdateStatus::Downloading {
                                    progress,
                                    downloaded_bytes: downloaded,
                                    total_bytes,
                                };
                            }
                        }
                        std::thread::sleep(Duration::from_millis(150));
                    }
                    Err(e) => {
                        let mut lock = status_clone.lock().unwrap_or_else(|e| e.into_inner());
                        *lock = UpdateStatus::Failed(format!("Error monitoring download: {}", e));
                        return;
                    }
                }
            }
        });
    }

    /// Replaces the currently running executable with the updated executable and restarts.
    pub fn restart_and_apply(&self) -> Result<(), String> {
        let current_exe = std::env::current_exe().map_err(|e| format!("Current exe error: {}", e))?;
        let old_exe = sibling_with_suffix(&current_exe, ".old");
        let new_exe = sibling_with_suffix(&current_exe, ".new");

        if !new_exe.exists() {
            return Err(format!(
                "Updated binary ({}) not found. Download may not have finished.",
                new_exe.display()
            ));
        }

        // Re-verify the checksum immediately before installing, not only right after
        // download: the file could have sat on disk for a while (user reading the
        // changelog) and this closes that window against local tampering.
        let expected_sha256 = {
            let lock = self.status.lock().unwrap_or_else(|e| e.into_inner());
            match &*lock {
                UpdateStatus::DownloadedReadyToRestart { expected_sha256, .. } => expected_sha256.clone(),
                _ => return Err("No verified update is ready to install.".to_string()),
            }
        };
        let actual_sha256 = sha256_file(&new_exe)?;
        if !actual_sha256.eq_ignore_ascii_case(&expected_sha256) {
            let _ = std::fs::remove_file(&new_exe);
            return Err(
                "Update file changed since it was verified (checksum mismatch); refusing to install.".to_string(),
            );
        }

        // Clean up previous old backup if present
        if old_exe.exists() {
            let _ = std::fs::remove_file(&old_exe);
        }

        // 1. Rename current running executable to .old
        std::fs::rename(&current_exe, &old_exe)
            .map_err(|e| format!("Failed to move running executable to backup (.old): {}", e))?;

        // 2. Rename new executable into place
        if let Err(e) = std::fs::rename(&new_exe, &current_exe) {
            // Attempt rollback
            let _ = std::fs::rename(&old_exe, &current_exe);
            return Err(format!("Failed to install new executable (rolled back): {}", e));
        }

        // 3. Launch newly replaced executable
        let mut spawn_cmd = std::process::Command::new(&current_exe);
        // Forward any command-line arguments if needed
        let args: Vec<String> = std::env::args().skip(1).collect();
        spawn_cmd.args(&args);

        spawn_cmd.spawn()
            .map_err(|e| format!("Failed to spawn updated executable: {}", e))?;

        // 4. Terminate the old process
        std::process::exit(0);
    }
}

/// Fetches latest release information from GitHub API using silent curl.exe.
fn fetch_latest_release(owner: &str, repo: &str) -> Result<ReleaseInfo, String> {
    let url = format!("https://api.github.com/repos/{}/{}/releases/latest", owner, repo);

    let mut cmd = std::process::Command::new(curl_path());
    cmd.args(&[
        "-s",
        "-L",
        "--max-time", "15",
        "-H", "User-Agent: CrabBoy-Advance-Updater",
        "-H", "Accept: application/vnd.github.v3+json",
        &url,
    ]);

    silence_console(&mut cmd);

    let output = cmd.output().map_err(|e| format!("Network request failed (curl error): {}", e))?;

    if !output.status.success() {
        return Err(format!("curl request returned status: {:?}", output.status.code()));
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    if stdout_str.trim().is_empty() {
        return Err("GitHub API returned empty response.".to_string());
    }

    let v: serde_json::Value = serde_json::from_str(&stdout_str)
        .map_err(|e| format!("Failed to parse GitHub JSON response: {}", e))?;

    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        return Err(format!("GitHub API message: {}", msg));
    }

    let tag_name = v.get("tag_name")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    if tag_name.is_empty() {
        return Err("No tag_name in GitHub release.".to_string());
    }

    let version = tag_name.trim_start_matches(|c| c == 'v' || c == 'V').to_string();
    let title = v.get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(&tag_name)
        .to_string();
    let body = v.get("body")
        .and_then(|b| b.as_str())
        .unwrap_or("")
        .to_string();
    let html_url = v.get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let published_at = v.get("published_at")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();

    let mut exe_download_url = None;
    let mut exe_size = 0u64;
    let mut zip_download_url = None;
    let mut zip_size = 0u64;
    let mut checksums_url = None;

    if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
        for asset in assets {
            let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let download_url = asset.get("browser_download_url").and_then(|u| u.as_str()).map(|s| s.to_string());
            let size = asset.get("size").and_then(|s| s.as_u64()).unwrap_or(0);

            if name.eq_ignore_ascii_case(EXE_ASSET_NAME) {
                exe_download_url = download_url.clone();
                exe_size = size;
            } else if is_archive_asset_for_this_platform(name) {
                zip_download_url = download_url;
                zip_size = size;
            } else if name.eq_ignore_ascii_case(CHECKSUMS_ASSET_NAME) {
                checksums_url = download_url;
            }
        }
    }

    Ok(ReleaseInfo {
        tag_name,
        version,
        title,
        body,
        html_url,
        published_at,
        exe_download_url,
        exe_size,
        zip_download_url,
        zip_size,
        checksums_url,
    })
}

/// Fetches the release's published SHA256SUMS.txt and verifies the just-downloaded
/// `crabboy-advance.exe` against it. Returns the expected digest on success so the
/// caller can re-verify it again immediately before installing (closes the TOCTOU
/// window between download and restart/apply).
fn verify_download_checksum(downloaded_path: &Path, release: &ReleaseInfo) -> Result<String, String> {
    let checksums_url = release
        .checksums_url
        .as_ref()
        .ok_or_else(|| format!("Release does not publish {} for verification.", CHECKSUMS_ASSET_NAME))?;

    let sums_text = fetch_text_asset(checksums_url)?;
    let expected = find_checksum(&sums_text, EXE_ASSET_NAME)
        .ok_or_else(|| format!("No checksum entry for {} in {}.", EXE_ASSET_NAME, CHECKSUMS_ASSET_NAME))?;

    let actual = sha256_file(downloaded_path)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(format!(
            "SHA-256 mismatch (expected {}, got {}).",
            expected, actual
        ));
    }

    Ok(expected)
}

/// True if `name` is the full redistributable archive for the *running* platform.
///
/// Releases carry one archive per OS, so each build must ignore the other
/// platforms' archives rather than offering a Windows .zip to a Linux user.
fn is_archive_asset_for_this_platform(name: &str) -> bool {
    let lower = name.to_lowercase();
    #[cfg(target_os = "windows")]
    {
        lower.ends_with(".zip") && lower.contains("windows")
    }
    #[cfg(target_os = "linux")]
    {
        (lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".appimage"))
            && lower.contains("linux")
    }
    #[cfg(target_os = "macos")]
    {
        (lower.ends_with(".tar.gz") || lower.ends_with(".dmg"))
            && (lower.contains("macos") || lower.contains("darwin"))
    }
}

/// Parses a version string like "0.2.0" or "v0.2.1" into (major, minor, patch).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let clean = s.trim().trim_start_matches(|c| c == 'v' || c == 'V');
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.is_empty() {
        return None;
    }

    let major = parts.get(0)?.parse::<u32>().ok()?;
    let minor = parts.get(1).unwrap_or(&"0").parse::<u32>().ok()?;
    let patch = parts.get(2).unwrap_or(&"0").parse::<u32>().ok()?;

    Some((major, minor, patch))
}

/// Returns true if `latest` is strictly newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some((l_maj, l_min, l_pat)), Some((c_maj, c_min, c_pat))) => {
            (l_maj, l_min, l_pat) > (c_maj, c_min, c_pat)
        }
        _ => latest != current && !latest.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        assert_eq!(parse_version("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("V1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("3"), Some((3, 0, 0)));
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer("v0.3.1", "0.3.0"));
        assert!(is_newer("v0.4.0", "0.3.0"));
        assert!(is_newer("v1.0.0", "0.3.0"));
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.2.0", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.3.0"));
    }

    #[test]
    #[ignore = "hits the live GitHub API; run explicitly with `cargo test -- --ignored`"]
    fn test_fetch_latest_release_live() {
        let res = fetch_latest_release(REPO_OWNER, REPO_NAME);
        assert!(res.is_ok(), "Expected live GitHub release check to succeed: {:?}", res);
        let info = res.unwrap();
        assert!(info.tag_name.starts_with('v') && info.tag_name.len() >= 4, "Invalid tag name: {}", info.tag_name);
        assert!(info.exe_download_url.is_some(), "Expected {} asset", EXE_ASSET_NAME);
        assert!(info.exe_size > 5_000_000, "Expected valid exe asset size");
    }

    #[test]
    fn sibling_suffix_handles_extensionless_unix_binaries() {
        // with_extension() would turn "crabboy-advance" into "crabboy-advance.new"
        // correctly but mangle "crabboy-advance.exe" into "crabboy-advance.new",
        // losing the .exe. Appending preserves both shapes.
        let unix = sibling_with_suffix(Path::new("/usr/local/bin/crabboy-advance"), ".new");
        assert_eq!(unix, PathBuf::from("/usr/local/bin/crabboy-advance.new"));

        let win = sibling_with_suffix(Path::new("/tmp/crabboy-advance.exe"), ".old");
        assert_eq!(win, PathBuf::from("/tmp/crabboy-advance.exe.old"));
    }

    #[test]
    fn archive_asset_matches_only_this_platform() {
        // Whatever platform the tests run on, the other platforms' archives
        // must never be selected as this build's update archive.
        #[cfg(target_os = "linux")]
        {
            assert!(is_archive_asset_for_this_platform("CrabBoy-linux-x86_64.tar.gz"));
            assert!(is_archive_asset_for_this_platform("CrabBoy-Linux.AppImage"));
            assert!(!is_archive_asset_for_this_platform("CrabBoy-windows-x64.zip"));
        }
        #[cfg(target_os = "windows")]
        {
            assert!(is_archive_asset_for_this_platform("CrabBoy-windows-x64.zip"));
            assert!(!is_archive_asset_for_this_platform("CrabBoy-linux-x86_64.tar.gz"));
        }
        #[cfg(target_os = "macos")]
        {
            assert!(is_archive_asset_for_this_platform("CrabBoy-macos-universal.dmg"));
            assert!(!is_archive_asset_for_this_platform("CrabBoy-windows-x64.zip"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn mark_executable_sets_user_exec_bit() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("crabboy_chmod_test_{}", std::process::id()));
        std::fs::write(&path, b"#!/bin/true\n").unwrap();
        // curl writes downloads as 0644; simulate that starting state.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        mark_executable(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode & 0o111, 0o111, "expected exec bits, got {:o}", mode);
    }

    #[test]
    fn find_checksum_parses_sha256sums_format() {
        let digest = format!("{}{}", "deadbeefcafef00d", "0".repeat(48));
        assert_eq!(digest.len(), 64);
        let text = format!("abc123  otherfile.exe\n{}  crabboy-advance.exe\n", digest);
        assert_eq!(find_checksum(&text, "crabboy-advance.exe"), Some(digest));
        assert_eq!(find_checksum(&text, "missing.exe"), None);
    }

    #[test]
    fn sha256_file_matches_known_vector() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("crabboy_sha_test_{}.bin", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        // Well-known NIST test vector: SHA-256("abc")
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(expected.len(), 64);
        let hash = sha256_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(hash, expected);
    }
}

