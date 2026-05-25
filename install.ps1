#Requires -Version 5
<#
.SYNOPSIS
  kamaji installer for Windows (PowerShell).

.DESCRIPTION
  Downloads the latest kamaji release for this machine, verifies its SHA-256,
  and installs kamaji.exe to a user directory on PATH. The Windows counterpart
  to install.sh (which stays Unix-only).

.EXAMPLE
  irm https://raw.githubusercontent.com/alveflo/kamaji/main/install.ps1 | iex

.NOTES
  Override the install directory with the KAMAJI_INSTALL_DIR environment
  variable (default: %LOCALAPPDATA%\Programs\kamaji).
#>

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'alveflo/kamaji'

function Fail($msg) {
    Write-Error $msg
    exit 1
}

# Resolve the install directory.
if ($env:KAMAJI_INSTALL_DIR) {
    $InstallDir = $env:KAMAJI_INSTALL_DIR
} elseif ($env:LOCALAPPDATA) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\kamaji'
} else {
    Fail 'LOCALAPPDATA is not set; set KAMAJI_INSTALL_DIR to choose an install directory'
}

# Map architecture -> Rust target triple (must match release asset names). On a
# 64-bit OS, a 32-bit PowerShell reports x86 in PROCESSOR_ARCHITECTURE but the
# real arch in PROCESSOR_ARCHITEW6432, so prefer the latter when present.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
switch ($arch) {
    'AMD64' { $target = 'x86_64-pc-windows-msvc' }
    default { Fail "unsupported architecture: $arch (only x86_64 Windows builds are published)" }
}

$asset = "kamaji-$target.zip"
$base = "https://github.com/$Repo/releases/latest/download"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("kamaji-install-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zipPath = Join-Path $tmp $asset
    $sumPath = "$zipPath.sha256"

    Write-Host "Downloading $asset ..."
    Invoke-WebRequest -Uri "$base/$asset" -OutFile $zipPath -UseBasicParsing
    Invoke-WebRequest -Uri "$base/$asset.sha256" -OutFile $sumPath -UseBasicParsing

    Write-Host 'Verifying checksum ...'
    $expected = ((Get-Content -Raw $sumPath).Trim() -split '\s+')[0]
    $actual = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
    if ($expected.ToLower() -ne $actual) {
        Fail "checksum mismatch (expected $expected, got $actual)"
    }

    Write-Host "Installing to $InstallDir ..."
    Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
    $extracted = Join-Path $tmp 'kamaji.exe'
    if (-not (Test-Path $extracted)) {
        Fail "archive did not contain a 'kamaji.exe' binary"
    }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path $extracted -Destination (Join-Path $InstallDir 'kamaji.exe') -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$exe = Join-Path $InstallDir 'kamaji.exe'
Write-Host -NoNewline 'Installed: '
& $exe --version

# Persist the install dir on the user PATH if it isn't already (the same thing
# rustup and scoop do). A fresh terminal is needed to pick it up.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = $userPath -and (($userPath -split ';') -contains $InstallDir)
if (-not $onPath) {
    $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host ""
    Write-Host "Added $InstallDir to your user PATH. Restart your terminal for it to take effect."
}
