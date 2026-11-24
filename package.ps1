# Builds the rekordbox module and CLUB685 in release mode and writes dist\CLUB685-<version>\ plus the ZIP.
# usage: powershell -ExecutionPolicy Bypass -File package.ps1 [-SkipBuild]
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$version = '0.2.0'
# Keep local build paths (user profile, checkout folder) out of the binaries.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$env:RUSTFLAGS = "--remap-path-prefix=$cargoHome=cargo --remap-path-prefix=$PSScriptRoot=club685 --remap-path-prefix=$env:USERPROFILE=user"

$root = $PSScriptRoot
Set-Location $root

# The module source: module\ in the repo, or the original rbmod folder next to the workspace.
$modDir = Join-Path $root 'module'
if (-not (Test-Path (Join-Path $modDir 'Cargo.toml'))) { $modDir = Join-Path $root '..\re\mod\rbmod' }
$modDll = Join-Path $modDir 'target\release\version.dll'

if (-not $SkipBuild) {
    # cargo writes progress to stderr; under 'Stop' PowerShell would turn that into a terminating error
    $ErrorActionPreference = 'Continue'
    cargo build --release --manifest-path (Join-Path $modDir 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "module build failed ($LASTEXITCODE)" }
    # the app bakes in the hash of the module it ships; the install helper refuses any other file
    $env:CLUB685_MODULE_SHA256 = (Get-FileHash -Algorithm SHA256 $modDll).Hash.ToLower()
    cargo build --release -p club685-app --bin club685
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    cargo run --release -p club685-app --example make_icon
    if ($LASTEXITCODE -ne 0) { throw "icon generation failed ($LASTEXITCODE)" }
    $ErrorActionPreference = 'Stop'
}

$exe = Join-Path $root 'target\release\club685.exe'
$ico = Join-Path $root 'assets\club685.ico'
if (-not (Test-Path $exe)) { throw "missing $exe" }
if (-not (Test-Path $ico)) { throw "missing $ico" }
if (-not (Test-Path $modDll)) { throw "missing $modDll" }

$name = "CLUB685-$version"
$dist = Join-Path $root 'dist'
$out = Join-Path $dist $name
$zip = Join-Path $dist "$name.zip"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
if (Test-Path $zip) { Remove-Item -Force $zip }
New-Item -ItemType Directory -Force (Join-Path $out 'Build') | Out-Null
New-Item -ItemType Directory -Force (Join-Path $out 'Assets\fonts') | Out-Null
New-Item -ItemType Directory -Force (Join-Path $out 'Module') | Out-Null

Copy-Item $exe (Join-Path $out 'Build\club685.exe')
Copy-Item $modDll (Join-Path $out 'Module\version.dll')
Copy-Item $ico (Join-Path $out 'Assets\club685.ico')
Copy-Item (Join-Path $root 'assets\fonts\*') (Join-Path $out 'Assets\fonts')

$utf8 = New-Object System.Text.UTF8Encoding($false)
$cmd = @(
    '@echo off',
    'if not exist "%~dp0Build\club685.exe" (',
    '  echo CLUB685 cannot start: Build\club685.exe was not found.',
    '  echo Please extract the full ZIP first and start this file from the extracted folder.',
    '  pause',
    '  exit /b 1',
    ')',
    'start "" "%~dp0Build\club685.exe"'
) -join "`r`n"
[System.IO.File]::WriteAllText((Join-Path $out 'Start CLUB685.cmd'), $cmd + "`r`n", [System.Text.Encoding]::ASCII)

$readme = @"
CLUB685 $version  -  an overlay for rekordbox 6.8.5
====================================================

WHAT IT DOES
CLUB685 sits on top of the rekordbox waveform area and adds what rekordbox 6.8.5 does not have:

  * Phase meter (Box or Line) for both decks, with MASTER / PLAYER label and the bar counter ("85.3 Bars").
  * Waveform modes: Normal (untouched), Split (both waveforms side by side, each centred on the playhead),
    Hide Both (black band, with both phase meters stacked in the middle when the phase meter is on).
  * A tray icon: the yellow button minimises to the tray, click the tray icon to bring the window back.

INSTALL (once)
  1. Extract the whole ZIP to a folder (do not start it from inside the ZIP).
  2. Close rekordbox.
  3. Double-click "Start CLUB685.cmd". The display says "One-time setup: install the rekordbox module".
  4. Click INSTALL and allow the Windows prompt (administrator). That copies Module\version.dll into the rekordbox
     folder and nothing else.
  5. Start rekordbox. The green screen shows what CLUB685 sees. The overlays appear only while rekordbox is the
     window in front ("Ready - switch to rekordbox" otherwise), and disappear when a menu or a move/resize is open.

To remove the module again: SETTINGS, then REMOVE MODULE (administrator prompt). Close rekordbox first.

HOW IT WORKS
The module (version.dll, loaded by rekordbox from its own folder) only reads deck state: phase, bar, MASTER, BPM.
It sends it to CLUB685 over a local UDP port (127.0.0.1:47990). CLUB685 draws the Split view as two Windows desktop
thumbnails of the rekordbox window. DWM composes them on the graphics card every screen refresh: nothing is copied and
no screen capture runs. The meters are drawn once per display frame, extrapolated between the module's 60 Hz updates.
The position of the waveform band is read from one screenshot that is taken while CLUB685's own windows are hidden
(a brief flicker when you switch mode or move rekordbox).
If rekordbox runs but sends no data, the display says "rekordbox module not responding: restart rekordbox" (for
example when rekordbox was already open during the install).

The overlay windows let every click through to rekordbox. CLUB685 never clicks, types into, moves or
closes rekordbox.

REQUIREMENTS
  * Windows 10 (2004 or newer) or 11, 64 bit.
  * rekordbox 6.8.5, started as a normal user (not as administrator). CLUB685 also refuses to run as administrator;
    only the one-time install and remove step asks for it.
  * rekordbox in the layout "2Deck Horizontal" (two waveform rows, one above the other).
  * For the phase meter: zoom the waveform in until the beat markers (small grey triangles) are visible.
  * rekordbox on your MAIN display. On a second monitor with a different scale Windows stretches the
    window and the pixel rules no longer match: CLUB685 then shows "Move rekordbox to the main display".

SETTINGS
Your choices are saved in %LOCALAPPDATA%\CLUB685\preferences.json and restored on the next start.
SETTINGS (bottom right) opens an about / diagnostics panel with "Copy diagnostics" for support.
DISCO only animates the green screen of CLUB685. It never touches rekordbox.

PRIVACY
CLUB685 makes no internet connection, has no telemetry and stores nothing but the small preferences file above.
It listens on 127.0.0.1:47990 (this PC only) for the module; only one program can hold that port at a time.
Diagnostics are copied to the clipboard only when you press the button.

NOTES
  * Screenshots and screen recordings of the overlay come out black on purpose.
  * Set CLUB685_LOG=1 to write a diagnostic log to %LOCALAPPDATA%\CLUB685\club685.log.
  * Fonts: Orbitron and Share Tech Mono, both SIL Open Font License 1.1 (see Assets\fonts).
  * rekordbox is a trademark of AlphaTheta Corporation. CLUB685 is an independent project and is not affiliated
    with or endorsed by AlphaTheta.
"@
[System.IO.File]::WriteAllText((Join-Path $out 'README.txt'), ($readme -replace "`r?`n", "`r`n"), $utf8)

# SHA256SUMS.txt: lowercase hash, two spaces, relative path with forward slashes.
$lines = Get-ChildItem -Recurse -File $out | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object FullName | ForEach-Object {
    $rel = $_.FullName.Substring($out.Length + 1).Replace('\', '/')
    $h = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLower()
    "$h  $rel"
}
[System.IO.File]::WriteAllText((Join-Path $out 'SHA256SUMS.txt'), (($lines -join "`n") + "`n"), $utf8)

Compress-Archive -Path $out -DestinationPath $zip -CompressionLevel Optimal
$zh = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
Write-Host "dist folder: $out"
Write-Host "zip:         $zip"
Write-Host "zip sha256:  $zh"
Get-ChildItem -Recurse -File $out | ForEach-Object { Write-Host ("  {0,10}  {1}" -f $_.Length, $_.FullName.Substring($out.Length + 1)) }
