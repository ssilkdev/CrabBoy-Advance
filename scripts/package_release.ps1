# Packages a CrabBoy Advance release: builds the release exe (if not already
# built), assembles the portable zip, and computes real SHA-256 checksums for
# every asset into SHA256SUMS.txt. The updater (src/ui/updater.rs) verifies
# downloads against this file before ever installing them, so it must ship
# with every release.

param(
    [Parameter(Mandatory = $true)]
    [string]$Version
)

$ErrorActionPreference = "Stop"

$packageDir = "target\package_v$Version"
$zipName = "crabboy-advance-v$Version-windows-x64.zip"
$zipPath = "target\$zipName"
$sumsPath = "target\SHA256SUMS.txt"

if (Test-Path $packageDir) {
    Remove-Item -Path $packageDir -Recurse -Force
}
New-Item -ItemType Directory -Path $packageDir -Force | Out-Null

Copy-Item "target\release\crabboy-advance.exe" "$packageDir\"
Copy-Item "assets\guide\manual.pdf" "$packageDir\CrabBoy_Advance_Trainers_Guide.pdf"
Copy-Item "README.md" "$packageDir\"
Copy-Item "LICENSE" "$packageDir\"

if (Test-Path $zipPath) {
    Remove-Item -Path $zipPath -Force
}
Compress-Archive -Path "$packageDir\*" -DestinationPath $zipPath -CompressionLevel Optimal

# Compute real checksums from the actual built/packaged files (not hardcoded).
$exeHash = (Get-FileHash "target\release\crabboy-advance.exe" -Algorithm SHA256).Hash.ToLower()
$zipHash = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
$pdfHash = (Get-FileHash "assets\guide\manual.pdf" -Algorithm SHA256).Hash.ToLower()
$iconHash = (Get-FileHash "assets\icon.png" -Algorithm SHA256).Hash.ToLower()

# SHA256SUMS.txt format: "<hex digest>  <filename>" per line (matches
# sha256sum/Get-FileHash convention; parsed by updater.rs's find_checksum()).
@(
    "$exeHash  crabboy-advance.exe"
    "$zipHash  $zipName"
    "$pdfHash  CrabBoy_Advance_Trainers_Guide.pdf"
    "$iconHash  icon.png"
) | Set-Content -Path $sumsPath -Encoding ascii

Write-Host "EXE Hash:  $exeHash"
Write-Host "ZIP Hash:  $zipHash"
Write-Host "PDF Hash:  $pdfHash"
Write-Host "ICON Hash: $iconHash"
Write-Host "Wrote checksums to $sumsPath"

@{
    exe_hash  = $exeHash
    zip_hash  = $zipHash
    pdf_hash  = $pdfHash
    icon_hash = $iconHash
    zip_path  = $zipPath
    zip_name  = $zipName
    sums_path = $sumsPath
} | ConvertTo-Json
