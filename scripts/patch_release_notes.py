import json
import subprocess
import urllib.request

# Get token from git credential
cred_process = subprocess.Popen(
    ['git', 'credential', 'fill'],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True
)
stdout, _ = cred_process.communicate(input="protocol=https\nhost=github.com\n\n")

token = ""
for line in stdout.splitlines():
    if line.startswith("password="):
        token = line.split("=", 1)[1].strip()
        break

if not token:
    raise RuntimeError("Could not retrieve GitHub token from git credential")

owner = "ssilkdev"
repo = "CrabBoy-Advance"
release_id = "383672781"

body_text = """# 🦀 Crabboy-Advance (v0.3.0)

> **Created fully with AI using Google Antigravity with Gemini Flash 3.8 Flash and Claude Opus 4.6.**

We are excited to announce **Crabboy-Advance (v0.3.0)**, introducing the brand new **Automatic Background Release Checker**, an **Update** tab and notification ribbon, a dedicated **Software Update Center**, and **1-Click Self-Updating** with Windows atomic restart!

---

## 🌟 What's New in v0.3.0

### 🔄 Automatic Update Checker & One-Click Self-Updater
- **Non-Blocking Startup Check**: Immediately upon launch, CrabBoy Advance connects asynchronously to GitHub Releases in the background without blocking emulator initialization or 60 FPS gameplay.
- **Top Menu Ribbon "Update" Tab**: Added a dedicated **🔄 Update** tab in the main menu ribbon. Displays real-time status: Up to date, Update available, Downloading (with live %), or Ready to restart.
- **Dynamic Visual Badge**: When a newer release is published on GitHub, the tab lights up as **🔄 Update (New!)** and an eye-catching **🎉 New Update Available!** badge appears in the top-right status bar for instant access.
- **Software Update Center Dialog**:
  - Version comparison (`v0.2.0` ➔ `v0.3.0`).
  - Formatted release notes and changelog viewer fetched directly from GitHub API.
  - Live animated download progress bar with byte metrics.
  - Single-click action buttons: **Download & Install**, **Restart & Apply**, and **View on GitHub**.
- **Windows In-Place Atomic Replacement**:
  - Automatically downloads the update asset to `crabboy-advance.exe.new`.
  - Performs safe NTFS binary swap (`.exe` ➔ `.old`, `.new` ➔ `.exe`).
  - Spawns the updated executable and terminates the old process cleanly.
  - Automatically sweeps and cleans leftover `.old` backup files on subsequent startup.

### ⚡ Core & UI Enhancements
- **Dynamic Versioning**: Synchronized all Help menus, About windows, and telemetry tags with `env!("CARGO_PKG_VERSION")`.
- **Pure-Rust Deserialization**: Integrated zero-dependency `serde_json` for robust release metadata parsing.
- **Silent Background Networking**: Built-in Windows `curl.exe` invocation with `CREATE_NO_WINDOW` ensures 0 terminal flash or prompt interruptions.

---

## 💾 Downloads & Verification

| File | Description | SHA-256 Checksum |
|---|---|---|
| `crabboy-advance-v0.3.0-windows-x64.zip` | Standalone portable release package (EXE, Guide PDF, README, License) | `3B6583DFE51DFCF03D9F467D14EC9CFE6F4E05DD5B258158DB08BC1F437A5F48` |
| `crabboy-advance.exe` | Self-contained executable with embedded guide, mascot icons, and auto-updater | `3D4125EA42896CF0DE1F548835889E23FCB419B2F4AF2F6D12A4D41C6C1191D3` |
| `CrabBoy_Advance_Trainers_Guide.pdf` | Official 8-page illustrated field manual (PDF) | `6721073F405D2D9D7F0542FCE2837C63280F5F991EBDEC3CED3F0BEC9C08E5FE` |
| `icon.png` | High-resolution 1024×1024 mascot app icon | `EC25CF26C7625913E6A87E50AC176F6CE4E47646B1D568795CE0934ED2157483` |

---

*CrabBoy Advance is an open-source Game Boy Advance emulation project developed in pure Rust.*
"""

payload = {
    "name": "Crabboy-Advance (v0.3.0)",
    "body": body_text
}

data = json.dumps(payload, ensure_ascii=False).encode("utf-8")

req = urllib.request.Request(
    f"https://api.github.com/repos/{owner}/{repo}/releases/{release_id}",
    data=data,
    headers={
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "Content-Type": "application/json; charset=utf-8",
        "User-Agent": "Crabboy-Advance-Release",
        "X-GitHub-Api-Version": "2022-11-28"
    },
    method="PATCH"
)

with urllib.request.urlopen(req) as resp:
    res_data = json.loads(resp.read().decode("utf-8"))
    print("SUCCESS! HTTP Status:", resp.status)
    print("Release Name:", res_data.get("name"))
    print("Body preview:")
    print(res_data.get("body")[:300])
