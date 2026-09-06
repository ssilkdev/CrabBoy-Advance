# Create and publish GitHub release for CrabBoy Advance v0.3.0
$ErrorActionPreference = "Stop"

# Get GitHub credentials from git-credential
$credInput = "protocol=https`nhost=github.com`n`n"
$credOutput = $credInput | git credential fill
$token = ""
foreach ($line in $credOutput) {
    if ($line -match "^password=(.+)$") {
        $token = $matches[1].Trim()
        break
    }
}

if ([string]::IsNullOrWhiteSpace($token)) {
    throw "Failed to retrieve GitHub token from git-credential."
}

$headers = @{
    "Accept" = "application/vnd.github+json"
    "Authorization" = "Bearer $token"
    "User-Agent" = "Crabboy-Advance-Release"
    "X-GitHub-Api-Version" = "2022-11-28"
}

$owner = "ssilkdev"
$repo = "CrabBoy-Advance"
$tagName = "v0.3.0"
$releaseName = "Crabboy-Advance (v0.3.0)"

$body = @"
# 🦀 Crabboy-Advance (v0.3.0)

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
"@

Write-Host "Creating release: $releaseName ($tagName)..."
$releasePayload = @{
    tag_name = $tagName
    target_commitish = "main"
    name = $releaseName
    body = $body
    draft = $false
    prerelease = $false
} | ConvertTo-Json

$createUrl = "https://api.github.com/repos/$owner/$repo/releases"
$release = Invoke-RestMethod -Uri $createUrl -Method Post -Headers $headers -Body $releasePayload -ContentType "application/json; charset=utf-8"

Write-Host "Release created successfully! ID: $($release.id), URL: $($release.html_url)"

$uploadBase = $release.upload_url -replace '\{\?name,label\}', ''

# Helper function to upload an asset
function Upload-Asset($filePath, $assetName, $contentType) {
    Write-Host "Uploading asset: $assetName ($contentType)..."
    $fileBytes = [System.IO.File]::ReadAllBytes($filePath)
    $uploadUrl = "$uploadBase`?name=$assetName"
    
    $assetHeaders = @{
        "Accept" = "application/vnd.github+json"
        "Authorization" = "Bearer $token"
        "User-Agent" = "Crabboy-Advance-Release"
        "Content-Type" = $contentType
        "X-GitHub-Api-Version" = "2022-11-28"
    }

    $assetResp = Invoke-RestMethod -Uri $uploadUrl -Method Post -Headers $assetHeaders -Body $fileBytes
    Write-Host "Uploaded $assetName successfully! Size: $($assetResp.size) bytes."
}

Upload-Asset "target\crabboy-advance-v0.3.0-windows-x64.zip" "crabboy-advance-v0.3.0-windows-x64.zip" "application/zip"
Upload-Asset "target\release\crabboy-advance.exe" "crabboy-advance.exe" "application/vnd.microsoft.portable-executable"
Upload-Asset "assets\guide\manual.pdf" "CrabBoy_Advance_Trainers_Guide.pdf" "application/pdf"
Upload-Asset "assets\icon.png" "icon.png" "image/png"

Write-Host "All assets uploaded successfully for $releaseName!"
