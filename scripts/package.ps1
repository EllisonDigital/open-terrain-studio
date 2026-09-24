# Build the release library and export the packaged Windows app with Godot.
# Windows counterpart of scripts/package.sh (which also handles Linux and macOS).
#
#   scripts\package.ps1
#   scripts\package.ps1 -Godot C:\Tools\Godot_v4.7.2-stable_win64_console.exe
#   scripts\package.ps1 -LibDir dist     # use a prebuilt terrain_godot.dll
#
# Output: build\windows\ (the exported app) and build\OpenTerrainStudio-<version>-windows-x86_64.zip
# Godot must be exactly the version in .godot-version, with export templates installed.
param(
    [string]$Godot = $(if ($env:GODOT) { $env:GODOT } else { "godot" }),
    [string]$LibDir = $env:LIB_DIR
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

$godotVersion = (Get-Content .godot-version -Raw).Trim()
$appVersion = (Select-String -Path app\project.godot -Pattern '^config/version="(.*)"').Matches[0].Groups[1].Value

$have = (& $Godot --headless --version 2>$null | Select-Object -Last 1)
if (-not "$have".StartsWith("$godotVersion.stable")) {
    throw "Godot $godotVersion is required (export templates must match exactly), found '$have'"
}

New-Item -ItemType Directory -Force app\bin | Out-Null
if ($LibDir) {
    Copy-Item (Join-Path $LibDir terrain_godot.dll) app\bin\
} else {
    cargo build --release -p terrain-godot
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    Copy-Item target\release\terrain_godot.dll app\bin\
}

# The first import of a fresh checkout crashes on exit inside Godot (4.6, 4.7)
# after importing successfully, so its exit code is ignored.
if (-not (Test-Path app\.godot\imported)) {
    & $Godot --headless --path app --import *> $null
}

$out = "build\windows"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
$target = Join-Path $root "$out\OpenTerrainStudio.exe"
Write-Host "==> Exporting Windows to $target"
& $Godot --headless --path app --export-release Windows $target
if (-not (Test-Path $target)) { throw "export produced no $target" }
if (-not (Test-Path "$out\terrain_godot.dll")) { throw "the exported app is missing terrain_godot.dll" }
Copy-Item LICENSE-MIT, LICENSE-APACHE $out

$name = "OpenTerrainStudio-$appVersion-windows-x86_64"
$stage = "build\$name"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
Copy-Item -Recurse $out $stage
$zip = "build\$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item -Recurse -Force $stage
Write-Host "==> $zip"
