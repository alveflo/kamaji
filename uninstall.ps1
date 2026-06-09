#Requires -Version 5
<#
.SYNOPSIS
  kamaji uninstaller for Windows (PowerShell).

.DESCRIPTION
  Stops a running kamajid daemon, removes kamaji.exe + kamajid.exe from the
  install directory, and drops that directory from the user PATH if it becomes
  empty. User data (board database, config, cache) is KEPT by default — pass
  -Purge to delete it too. The Windows counterpart to uninstall.sh.

.PARAMETER Purge
  Also delete kamaji's data directories (%APPDATA%\kamaji, %LOCALAPPDATA%\kamaji).

.EXAMPLE
  irm https://raw.githubusercontent.com/alveflo/kamaji/main/uninstall.ps1 | iex

.EXAMPLE
  # With -Purge (download then run, since piping can't pass parameters):
  iwr https://raw.githubusercontent.com/alveflo/kamaji/main/uninstall.ps1 -OutFile uninstall.ps1
  ./uninstall.ps1 -Purge

.NOTES
  Override the install directory with the KAMAJI_INSTALL_DIR environment
  variable (default: %LOCALAPPDATA%\Programs\kamaji), matching install.ps1.
#>

param(
    [switch]$Purge
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Resolve the install directory the same way install.ps1 does.
if ($env:KAMAJI_INSTALL_DIR) {
    $InstallDir = $env:KAMAJI_INSTALL_DIR
} elseif ($env:LOCALAPPDATA) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\kamaji'
} else {
    Write-Error 'LOCALAPPDATA is not set; set KAMAJI_INSTALL_DIR to the directory kamaji was installed to'
    exit 1
}

# 1. Stop a running daemon (best-effort).
$proc = Get-Process -Name 'kamajid' -ErrorAction SilentlyContinue
if ($proc) {
    $proc | Stop-Process -Force -ErrorAction SilentlyContinue
    Write-Host 'Stopped running kamajid'
}

# 2. Remove the installed binaries.
$removedAny = $false
foreach ($bin in 'kamaji.exe', 'kamajid.exe') {
    $path = Join-Path $InstallDir $bin
    if (Test-Path $path) {
        Remove-Item -Path $path -Force
        Write-Host "Removed $path"
        $removedAny = $true
    }
}
if (-not $removedAny) {
    Write-Host "No kamaji binaries found in $InstallDir"
}

# 3. If the install dir is now empty, drop it from the user PATH and remove it.
if ((Test-Path $InstallDir) -and -not (Get-ChildItem -Path $InstallDir -Force)) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath) {
        $entries = $userPath -split ';' | Where-Object { $_ -and $_ -ne $InstallDir }
        $newPath = $entries -join ';'
        if ($newPath -ne $userPath) {
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            Write-Host "Removed $InstallDir from your user PATH (restart your terminal to pick it up)"
        }
    }
    Remove-Item -Path $InstallDir -Force -ErrorAction SilentlyContinue
}

# 4. Data: kept by default, deleted with -Purge.
$dataDirs = @()
if ($env:APPDATA)      { $dataDirs += (Join-Path $env:APPDATA 'kamaji') }
if ($env:LOCALAPPDATA) { $dataDirs += (Join-Path $env:LOCALAPPDATA 'kamaji') }
$dataDirs = $dataDirs | Select-Object -Unique | Where-Object { Test-Path $_ }

if ($Purge) {
    foreach ($d in $dataDirs) {
        Remove-Item -Path $d -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host "Purged $d"
    }
    Write-Host ''
    Write-Host 'kamaji fully uninstalled.'
} else {
    Write-Host ''
    Write-Host 'kamaji uninstalled. Your data was kept:'
    foreach ($d in $dataDirs) { Write-Host "  $d" }
    Write-Host 'Re-run with -Purge to delete it.'
}
