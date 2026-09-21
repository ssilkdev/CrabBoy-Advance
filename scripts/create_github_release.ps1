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

$body = Get-Content -Path $BodyPath -Raw

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

Upload-Asset "target\crabboy-advance-v$Version-windows-x64.zip" "crabboy-advance-v$Version-windows-x64.zip" "application/zip"
Upload-Asset "target\release\crabboy-advance.exe" "crabboy-advance.exe" "application/vnd.microsoft.portable-executable"
Upload-Asset "assets\guide\manual.pdf" "CrabBoy_Advance_Trainers_Guide.pdf" "application/pdf"
Upload-Asset "assets\icon.png" "icon.png" "image/png"
# SHA256SUMS.txt is what the in-app updater (src/ui/updater.rs) fetches and
# verifies crabboy-advance.exe against before ever installing an update.
Upload-Asset "target\SHA256SUMS.txt" "SHA256SUMS.txt" "text/plain"

Write-Host "All assets uploaded successfully for $Title!"
Write-Host "Release URL: $($release.html_url)"
