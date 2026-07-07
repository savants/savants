# Savants installer for Windows
# Usage: irm savants.sh/windows | iex
# Or:    powershell -c "irm https://releases.savants.dev/latest/install.ps1 | iex"

$ErrorActionPreference = "Stop"
$SavantsHome = "$env:USERPROFILE\.savants"
$BinDir = "$SavantsHome\bin"
$R2Url = "https://releases.savants.dev"
$Target = "x86_64-pc-windows-gnu"

Write-Host ""
Write-Host "  savants installer (Windows)" -ForegroundColor Cyan
Write-Host ""

# Create dirs
New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
New-Item -ItemType Directory -Path "$SavantsHome\data" -Force | Out-Null

# Check current version
$CurrentVersion = ""
if (Test-Path "$BinDir\savants.exe") {
    try {
        $CurrentVersion = (& "$BinDir\savants.exe" --version 2>$null) -replace "savants ", ""
    } catch {}
}

# Get latest version
try {
    $LatestVersion = (Invoke-RestMethod -Uri "$R2Url/latest/version.txt" -TimeoutSec 5).Trim()
} catch {
    $LatestVersion = ""
}

if ($CurrentVersion -and $LatestVersion -and ($CurrentVersion -eq $LatestVersion)) {
    Write-Host "  Already on latest: v$CurrentVersion" -ForegroundColor Green
    exit 0
}

$VersionLabel = if ($LatestVersion) { $LatestVersion } else { "latest" }

# Download
Write-Host "  [1/3] Platform: $Target"
Write-Host "  [2/3] Downloading v$VersionLabel..." -NoNewline

$TmpFile = "$env:TEMP\savants-$Target.tar.gz"
$TmpDir = "$env:TEMP\savants-extract"
try {
    Invoke-WebRequest -Uri "$R2Url/latest/savants-$Target.tar.gz" -OutFile $TmpFile -UseBasicParsing
    Write-Host " done" -ForegroundColor Green
} catch {
    Write-Host " failed" -ForegroundColor Red
    Write-Host "  Download failed. Check https://github.com/savants/savants/releases" -ForegroundColor Red
    exit 1
}

# Extract
Write-Host "  [3/3] Installing..." -NoNewline
if (Test-Path $TmpDir) { Remove-Item -Recurse -Force $TmpDir }
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

# Use .NET to extract tar.gz (avoids GNU tar drive letter issues)
try {
    $fs = [System.IO.File]::OpenRead($TmpFile)
    $gz = New-Object System.IO.Compression.GZipStream($fs, [System.IO.Compression.CompressionMode]::Decompress)
    $tarFile = Join-Path $TmpDir "savants.tar"
    $tarFs = [System.IO.File]::Create($tarFile)
    $gz.CopyTo($tarFs)
    $tarFs.Close(); $gz.Close(); $fs.Close()
    # Use tar on the uncompressed .tar (no gzip = no drive letter issue)
    Push-Location $TmpDir
    & tar xf "savants.tar" 2>$null
    Pop-Location
    Remove-Item $tarFile -Force -ErrorAction SilentlyContinue
} catch {
    # Fallback: try system tar directly
    Push-Location $TmpDir
    & tar xzf $TmpFile 2>$null
    Pop-Location
}

# Find the binary
$ExtractedBin = Get-ChildItem -Path $TmpDir -Filter "savants*" -Recurse -File | Where-Object { $_.Extension -eq '.exe' -or $_.Extension -eq '' } | Select-Object -First 1
if (-not $ExtractedBin) {
    Write-Host " failed" -ForegroundColor Red
    Write-Host "  Could not find savants binary in archive" -ForegroundColor Red
    exit 1
}

# Install
Copy-Item -Path $ExtractedBin.FullName -Destination "$BinDir\savants.exe" -Force
Remove-Item -Recurse -Force $TmpDir
Remove-Item -Force $TmpFile

# Verify
try {
    $InstalledVersion = (& "$BinDir\savants.exe" --version 2>$null) -replace "savants ", ""
} catch {
    $InstalledVersion = "unknown"
}

# Version verification
if ($LatestVersion -and $InstalledVersion -and ($InstalledVersion -ne $LatestVersion)) {
    Write-Host " VERSION MISMATCH" -ForegroundColor Red
    Write-Host "  Expected v$LatestVersion but binary reports v$InstalledVersion" -ForegroundColor Yellow
    Write-Host "  The binary on the CDN may be outdated. Please report this issue." -ForegroundColor Yellow
    exit 1
}

Write-Host " done" -ForegroundColor Green

# Setup guard protection
try {
    & "$BinDir\savants.exe" guard preset standard 2>$null | Out-Null
} catch {}

# Output
Write-Host ""
Write-Host "  savants v$InstalledVersion installed" -ForegroundColor Green
Write-Host "  Installed to: $BinDir\savants.exe" -ForegroundColor DarkGray
if ($CurrentVersion) {
    Write-Host "  Updated from v$CurrentVersion" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "  savants guard list" -ForegroundColor White -NoNewline
Write-Host "     see active guard rules" -ForegroundColor DarkGray
Write-Host "  savants guard stats" -ForegroundColor White -NoNewline
Write-Host "    see what got blocked" -ForegroundColor DarkGray
Write-Host "  savants up" -ForegroundColor White -NoNewline
Write-Host "             index your repo for code intelligence" -ForegroundColor DarkGray

Write-Host ""
Write-Host "  Customize: savants guard preset battle-tested" -ForegroundColor DarkGray
Write-Host "  To update:  irm savants.sh/windows | iex" -ForegroundColor DarkGray

# PATH notice
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$BinDir*") {
    Write-Host ""
    Write-Host "  ──────────────────────────────────────────" -ForegroundColor DarkGray
    Write-Host "  Add to your PATH (run once):" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"$BinDir;`$env:Path`", 'User')" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  Then restart your terminal." -ForegroundColor DarkGray
}
Write-Host ""
