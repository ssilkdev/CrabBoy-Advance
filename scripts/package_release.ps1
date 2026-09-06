# Package CrabBoy Advance Release v0.3.0

$ErrorActionPreference = "Stop"

$packageDir = "target\package_v0.3.0"
$zipPath = "target\crabboy-advance-v0.3.0-windows-x64.zip"

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

$exeHash = (Get-FileHash "target\release\crabboy-advance.exe" -Algorithm SHA256).Hash
$zipHash = (Get-FileHash $zipPath -Algorithm SHA256).Hash
$pdfHash = (Get-FileHash "assets\guide\manual.pdf" -Algorithm SHA256).Hash
$iconHash = (Get-FileHash "assets\icon.png" -Algorithm SHA256).Hash

Write-Host "EXE Hash: $exeHash"
Write-Host "ZIP Hash: $zipHash"
Write-Host "PDF Hash: $pdfHash"
Write-Host "ICON Hash: $iconHash"

# Return hashes as JSON
@{
    exe_hash = $exeHash
    zip_hash = $zipHash
    pdf_hash = $pdfHash
    icon_hash = $iconHash
    zip_path = $zipPath
} | ConvertTo-Json
