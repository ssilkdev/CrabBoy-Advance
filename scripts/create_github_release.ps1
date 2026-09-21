# Creates and publishes a GitHub release for CrabBoy Advance.
# Usage: .\scripts\create_github_release.ps1 -Version 0.4.0 -TagName v0.4.0 -Title "..." -BodyPath release_notes.md

param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$TagName,
    [Parameter(Mandatory = $true)]
    [string]$Title,
    [Parameter(Mandatory = $true)]
    [string]$BodyPath,
    [switch]$Draft
)

$ErrorActionPreference = "Stop"

# Anchor all relative paths (target\..., assets\...) to the repo root
# regardless of the caller's current directory.
Set-Location -Path (Resolve-Path (Join-Path $PSScriptRoot ".."))

# Get GitHub credentials from git-credential. Piping a multi-line string
# directly to a native command from PowerShell mangles the blank-line
# terminator git-credential expects, so go through a temp file + cmd.exe
# redirection instead, which preserves it correctly.
$credFile = [System.IO.Path]::GetTempFileName()
try {
    [System.IO.File]::WriteAllText($credFile, "protocol=https`nhost=github.com`n`n")
    $credOutput = cmd /c "git credential fill < `"$credFile`""
} finally {
    Remove-Item -Path $credFile -Force -ErrorAction SilentlyContinue
}
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

# Get-Content -Raw can return a string decorated with PSPath/etc. note
# properties that break ConvertTo-Json's serialization of the payload
# hashtable; read via .NET directly for a genuinely plain string.
$body = [System.IO.File]::ReadAllText((Resolve-Path $BodyPath))

Write-Host "Creating release: $Title ($TagName)..."
$releasePayload = @{
    tag_name = $TagName
    target_commitish = "main"
    name = $Title
    body = $body
    draft = [bool]$Draft
    prerelease = $false
} | ConvertTo-Json

$createUrl = "https://api.github.com/repos/$owner/$repo/releases"
$release = Invoke-RestMethod -Uri $createUrl -Method Post -Headers $headers -Body $releasePayload -ContentType "application/json; charset=utf-8"

Write-Host "Release created successfully! ID: $($release.id), URL: $($release.html_url)"

$uploadBase = $release.upload_url -replace '\{\?name,label\}', ''

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

function Upload-Asset($relativePath, $assetName, $contentType) {
    Write-Host "Uploading asset: $assetName ($contentType)..."
    # Resolve against $repoRoot explicitly: relative paths passed to .NET
    # APIs resolve against Environment.CurrentDirectory, which doesn't
    # reliably follow PowerShell's Set-Location in this environment.
    $filePath = Join-Path $repoRoot $relativePath
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

Upload-Asset "target\crabboy-advance-v$Version-windows-x64.zip" "crabboy-advance-v$Version-windows-x64.zip" "application/zip"
Upload-Asset "target\release\crabboy-advance.exe" "crabboy-advance.exe" "application/vnd.microsoft.portable-executable"
Upload-Asset "assets\guide\manual.pdf" "CrabBoy_Advance_Trainers_Guide.pdf" "application/pdf"
Upload-Asset "assets\icon.png" "icon.png" "image/png"
# SHA256SUMS.txt is what the in-app updater (src/ui/updater.rs) fetches and
# verifies crabboy-advance.exe against before ever installing an update.
Upload-Asset "target\SHA256SUMS.txt" "SHA256SUMS.txt" "text/plain"

Write-Host "All assets uploaded successfully for $Title!"
Write-Host "Release URL: $($release.html_url)"
