//! Automatic GitHub Release Checker & Self-Updater for CrabBoy Advance
//!
//! Features:
//! - Asynchronous, non-blocking startup check against GitHub Releases API.
//! - Semver comparison between currently running binary and latest GitHub tag.
//! - Live download tracking of release asset (crabboy-advance.exe).
//! - Windows atomic executable replacement (.old swap) and automatic restart.
//! - Startup cleanup of legacy .old executable files.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const REPO_OWNER: &str = "ssilkdev";
pub const REPO_NAME: &str = "CrabBoy-Advance";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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
            let old_exe = current_exe.with_extension("exe.old");
            if old_exe.exists() {
                let _ = std::fs::remove_file(old_exe);
            }
            let new_exe = current_exe.with_extension("exe.new");
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
            let current = status_clone.lock().unwrap();
            match *current {
                UpdateStatus::Checking | UpdateStatus::Downloading { .. } => return,
                _ => {}
            }
        }

        *status_clone.lock().unwrap() = UpdateStatus::Checking;

        std::thread::spawn(move || {
            match fetch_latest_release(REPO_OWNER, REPO_NAME) {
                Ok(release) => {
                    let is_update = is_newer(&release.version, CURRENT_VERSION);
                    let mut lock = status_clone.lock().unwrap();
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
                    let mut lock = status_clone.lock().unwrap();
                    *lock = UpdateStatus::Failed(err);
                }
            }
        });
    }

    /// Spawns a background thread to download the latest `crabboy-advance.exe` asset.
    pub fn download_and_install_async(&self) {
        let status_clone = Arc::clone(&self.status);

        let release_info = {
            let lock = status_clone.lock().unwrap();
            match &*lock {
                UpdateStatus::UpdateAvailable { latest, .. } => latest.clone(),
                _ => return,
            }
        };

        let exe_url = match &release_info.exe_download_url {
            Some(url) => url.clone(),
            None => {
                let mut lock = status_clone.lock().unwrap();
                *lock = UpdateStatus::Failed("No crabboy-advance.exe binary asset found in latest release.".to_string());
                return;
            }
        };

        *status_clone.lock().unwrap() = UpdateStatus::Downloading {
            progress: 0.0,
            downloaded_bytes: 0,
            total_bytes: release_info.exe_size,
        };

        std::thread::spawn(move || {
            let current_exe = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    let mut lock = status_clone.lock().unwrap();
                    *lock = UpdateStatus::Failed(format!("Cannot determine current exe location: {}", e));
                    return;
                }
            };

            let temp_download = current_exe.with_extension("exe.new");
            let _ = std::fs::remove_file(&temp_download);

            // Execute curl.exe with silent flags and CREATE_NO_WINDOW
            let mut cmd = std::process::Command::new("curl.exe");
            cmd.args(&[
                "-L",
                "-f",
                "--max-time", "180",
                "-H", "User-Agent: CrabBoy-Advance-Updater",
                "-o", temp_download.to_str().unwrap_or("crabboy-advance.exe.new"),
                &exe_url,
            ]);

            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let mut lock = status_clone.lock().unwrap();
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
                                    let mut lock = status_clone.lock().unwrap();
                                    *lock = UpdateStatus::DownloadedReadyToRestart {
                                        new_exe_path: temp_download,
                                        latest: release_info,
                                    };
                                    return;
                                }
                            }
                            let mut lock = status_clone.lock().unwrap();
                            *lock = UpdateStatus::Failed("Downloaded binary is corrupt or too small.".to_string());
                        } else {
                            let mut lock = status_clone.lock().unwrap();
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
                            let mut lock = status_clone.lock().unwrap();
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
                        let mut lock = status_clone.lock().unwrap();
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
        let old_exe = current_exe.with_extension("exe.old");
        let new_exe = current_exe.with_extension("exe.new");

        if !new_exe.exists() {
            return Err("Updated binary (crabboy-advance.exe.new) not found. Download may not have finished.".to_string());
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

    let mut cmd = std::process::Command::new("curl.exe");
    cmd.args(&[
        "-s",
        "-L",
        "--max-time", "15",
        "-H", "User-Agent: CrabBoy-Advance-Updater",
        "-H", "Accept: application/vnd.github.v3+json",
        &url,
    ]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

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

    if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
        for asset in assets {
            let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let download_url = asset.get("browser_download_url").and_then(|u| u.as_str()).map(|s| s.to_string());
            let size = asset.get("size").and_then(|s| s.as_u64()).unwrap_or(0);

            if name.eq_ignore_ascii_case("crabboy-advance.exe") {
                exe_download_url = download_url.clone();
                exe_size = size;
            } else if name.ends_with(".zip") && name.to_lowercase().contains("windows") {
                zip_download_url = download_url;
                zip_size = size;
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
    })
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
    fn test_fetch_latest_release_live() {
        let res = fetch_latest_release(REPO_OWNER, REPO_NAME);
        assert!(res.is_ok(), "Expected live GitHub release check to succeed: {:?}", res);
        let info = res.unwrap();
        assert!(info.tag_name.starts_with('v') && info.tag_name.len() >= 4, "Invalid tag name: {}", info.tag_name);
        assert!(info.exe_download_url.is_some(), "Expected crabboy-advance.exe asset");
        assert!(info.exe_size > 5_000_000, "Expected valid exe asset size");
    }
}

