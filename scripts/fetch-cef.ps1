#requires -Version 5.1
<#
.SYNOPSIS
    Downloads the CEF version pinned for CyberDesk and sets it up under
    vendor/cef/.

.DESCRIPTION
    The CEF binaries are several hundred MB and are NEVER committed. This script
    downloads the exact pinned CEF distribution (see docs/cyberdesk-decisions.md,
    D-0002) from the official Spotify CDN, verifies the SHA-1, extracts it, and
    flattens it into exactly the layout the `cef-dll-sys` crate expects (Release/
    + Resources/ + include/ + libcef_dll/ + cmake/ into the root, plus an
    archive.json marker). That makes the build use vendor/cef/ directly (no
    second download).

    Idempotent: a second run without -Force detects a valid existing install and
    does nothing.

.PARAMETER Dest
    Target directory. Default: <repo>/vendor/cef.

.PARAMETER Force
    Remove an existing install and set it up again.

.EXAMPLE
    ./scripts/fetch-cef.ps1
.EXAMPLE
    ./scripts/fetch-cef.ps1 -Force
#>
[CmdletBinding()]
param(
    [string]$Dest,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Robustly resolve the script directory (-File with forward slashes can leave
# $PSScriptRoot empty), then default $Dest to <repo>/vendor/cef.
$ScriptDir = $PSScriptRoot
if (-not $ScriptDir) { $ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $ScriptDir) { $ScriptDir = (Get-Location).Path }
if (-not $Dest) { $Dest = Join-Path $ScriptDir '..\vendor\cef' }

# --- Pinned CEF distribution (D-0002) ---------------------------------------
$CdnBase          = 'https://cef-builds.spotifycdn.com'
$CefArchive       = 'cef_binary_149.0.6+g0d0eeb6+chromium-149.0.7827.201_windows64_minimal.tar.bz2'
$CefSha1          = 'fe8f461b743f03dc640e998ae08264407d8bc2c9'
$ExtractedDirName = 'cef_binary_149.0.6+g0d0eeb6+chromium-149.0.7827.201_windows64_minimal'
# ----------------------------------------------------------------------------

function Write-Step([string]$Message) { Write-Host "==> $Message" -ForegroundColor Cyan }

$Dest = [System.IO.Path]::GetFullPath($Dest)
$archiveJsonPath = Join-Path $Dest 'archive.json'

# --- Idempotency check ------------------------------------------------------
if ((Test-Path -LiteralPath $archiveJsonPath) -and -not $Force) {
    Write-Step "CEF is already set up at: $Dest"
    Write-Host "    (use -Force to reinstall)"
    exit 0
}

# --- Locate tar (Windows bsdtar); it extracts .tar.bz2 directly -------------
$TarExe = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $TarExe)) { $TarExe = 'tar' }

# --- Download into a temporary directory ------------------------------------
$TmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("cyberdesk-cef-" + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $TmpRoot -Force | Out-Null

try {
    $ArchivePath = Join-Path $TmpRoot $CefArchive
    $Url = "$CdnBase/$CefArchive"

    Write-Step "Downloading CEF:"
    Write-Host "    $Url"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $client = New-Object System.Net.WebClient
    try {
        $client.DownloadFile($Url, $ArchivePath)
    } finally {
        $client.Dispose()
    }
    $sw.Stop()
    $sizeMb = [math]::Round((Get-Item -LiteralPath $ArchivePath).Length / 1MB, 1)
    Write-Host "    downloaded: $sizeMb MB in $([math]::Round($sw.Elapsed.TotalSeconds,1)) s"

    # --- Verify SHA-1 -------------------------------------------------------
    Write-Step "Verifying SHA-1 ..."
    $actual = (Get-FileHash -Algorithm SHA1 -LiteralPath $ArchivePath).Hash.ToLowerInvariant()
    if ($actual -ne $CefSha1.ToLowerInvariant()) {
        throw "SHA-1 mismatch: expected $CefSha1, got $actual. Download corrupted."
    }
    Write-Host "    OK ($actual)"

    # --- Extract ------------------------------------------------------------
    Write-Step "Extracting archive ..."
    $ExtractRoot = Join-Path $TmpRoot 'extract'
    New-Item -ItemType Directory -Path $ExtractRoot -Force | Out-Null
    & $TarExe -x -f $ArchivePath -C $ExtractRoot
    if ($LASTEXITCODE -ne 0) { throw "tar extraction failed (exit $LASTEXITCODE)." }
    $SrcDir = Join-Path $ExtractRoot $ExtractedDirName
    if (-not (Test-Path -LiteralPath $SrcDir)) {
        throw "Expected directory not found after extraction: $SrcDir"
    }

    # --- Create a fresh target directory ------------------------------------
    if (Test-Path -LiteralPath $Dest) {
        Write-Step "Removing existing target directory ..."
        Remove-Item -LiteralPath $Dest -Recurse -Force
    }
    New-Item -ItemType Directory -Path $Dest -Force | Out-Null

    # --- Flatten into the layout cef-dll-sys expects ------------------------
    # Order/content per download-cef::extract_target_archive:
    #   Release/*   -> vendor/cef/      (libcef.dll, libcef.lib, chrome_elf.dll, *.bin, ...)
    #   Resources/* -> vendor/cef/      (icudtl.dat, *.pak, locales/)
    #   include, libcef_dll, cmake, CMakeLists.txt, CREDITS.html -> vendor/cef/
    Write-Step "Laying out at: $Dest"

    foreach ($sub in @('Release', 'Resources')) {
        $subPath = Join-Path $SrcDir $sub
        if (-not (Test-Path -LiteralPath $subPath)) { throw "Missing in archive: $sub" }
        Get-ChildItem -Force -LiteralPath $subPath | ForEach-Object {
            Move-Item -LiteralPath $_.FullName -Destination $Dest -Force
        }
    }

    foreach ($item in @('include', 'libcef_dll', 'cmake', 'CMakeLists.txt', 'CREDITS.html', 'LICENSE.txt', 'README.txt')) {
        $p = Join-Path $SrcDir $item
        if (Test-Path -LiteralPath $p) {
            Move-Item -LiteralPath $p -Destination $Dest -Force
        }
    }

    # --- Write the marker (archive.json) ------------------------------------
    # cef-dll-sys/build.rs reads this marker (check_archive_json) and then uses
    # vendor/cef directly, without downloading again.
    $archiveJson = ([ordered]@{ type = 'minimal'; name = $CefArchive; sha1 = $CefSha1 } | ConvertTo-Json)
    [System.IO.File]::WriteAllText($archiveJsonPath, $archiveJson)  # UTF-8 without BOM

    # --- Sanity check -------------------------------------------------------
    $required = @('libcef.dll', 'libcef.lib', 'icudtl.dat', 'CMakeLists.txt')
    $missing = @()
    foreach ($r in $required) {
        if (-not (Test-Path -LiteralPath (Join-Path $Dest $r))) { $missing += $r }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Dest 'include'))) { $missing += 'include/' }
    if (-not (Test-Path -LiteralPath (Join-Path $Dest 'libcef_dll'))) { $missing += 'libcef_dll/' }
    if (-not (Test-Path -LiteralPath (Join-Path $Dest 'locales'))) { $missing += 'locales/' }
    if ($missing.Count -gt 0) {
        throw ("Layout incomplete, missing: " + ($missing -join ', '))
    }

    Write-Host ""
    Write-Step "Done. CEF is set up."
    Write-Host "    Version: $ExtractedDirName"
    Write-Host "    Path   : $Dest"
    Write-Host "    Next   : cargo run --release   (or -- --windowed)"
}
finally {
    if (Test-Path -LiteralPath $TmpRoot) {
        Remove-Item -LiteralPath $TmpRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
