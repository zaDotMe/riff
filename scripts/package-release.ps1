param(
    [string]$OutDir = "dist\release"
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

Copy-Item "target\release\riff-slint.exe" "$OutDir\Riff-Slint-windows-x64.exe" -Force
Copy-Item "target\release\riff-core.exe" "$OutDir\Riff-Core-windows-x64.exe" -Force

$stage = Join-Path $OutDir "portable-stage"
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item "target\release\riff-slint.exe" "$stage\riff-slint.exe" -Force
Copy-Item "target\release\riff-core.exe" "$stage\riff-core.exe" -Force
Copy-Item "src-tauri\icons\icon.ico" "$stage\icon.ico" -Force

$zipPath = Join-Path $OutDir "Riff-windows-x64-portable.zip"
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
Compress-Archive -Path "$stage\*" -DestinationPath $zipPath -Force
Remove-Item $stage -Recurse -Force

$makensis = (Get-Command makensis -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source)
if (-not $makensis -and (Test-Path "C:\Program Files (x86)\NSIS\makensis.exe")) {
    $makensis = "C:\Program Files (x86)\NSIS\makensis.exe"
}
if ($makensis) {
    $setupOut = Join-Path (Resolve-Path $OutDir) "Riff-Setup-windows-x64.exe"
    & $makensis "/DAPP_VERSION=0.7.6" "/XOutFile $setupOut" "packaging\windows\riff-slint.nsi"
}

Get-ChildItem $OutDir | Select-Object Name, Length
