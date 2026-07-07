# Savants uninstaller for Windows
# Usage: irm https://releases.savants.dev/latest/uninstall.ps1 | iex

$ErrorActionPreference = "Stop"
$SavantsHome = "$env:USERPROFILE\.savants"

Write-Host ""
Write-Host "  Savants Uninstaller" -ForegroundColor White
Write-Host ""

# Step 1: Remove Claude Code hooks
$SettingsFile = "$env:USERPROFILE\.claude\settings.json"
if (Test-Path $SettingsFile) {
    try {
        $settings = Get-Content $SettingsFile -Raw | ConvertFrom-Json
        $hooks = $settings.hooks
        $removed = 0
        if ($hooks) {
            foreach ($event in @($hooks.PSObject.Properties.Name)) {
                $filtered = @($hooks.$event | Where-Object { ($_ | ConvertTo-Json) -notlike "*savants*" })
                $removed += ($hooks.$event.Count - $filtered.Count)
                if ($filtered.Count -eq 0) {
                    $hooks.PSObject.Properties.Remove($event)
                } else {
                    $hooks.$event = $filtered
                }
            }
            $settings | ConvertTo-Json -Depth 10 | Set-Content $SettingsFile
        }
        if ($removed -gt 0) {
            Write-Host "  > Removed $removed Claude Code hooks" -ForegroundColor Green
        } else {
            Write-Host "  > No Claude Code hooks to remove" -ForegroundColor DarkGray
        }
    } catch {
        Write-Host "  ! Could not update settings.json" -ForegroundColor Yellow
    }
}

# Step 2: Remove PATH
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -like "*$SavantsHome*") {
    $NewPath = ($UserPath -split ";" | Where-Object { $_ -notlike "*savants*" }) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host "  > Removed from PATH" -ForegroundColor Green
}

# Step 3: Stop processes
Get-Process -Name "savants" -ErrorAction SilentlyContinue | Stop-Process -Force
Write-Host "  > Stopped savants processes" -ForegroundColor Green

# Step 4: Remove directory
if (Test-Path $SavantsHome) {
    $size = "{0:N1} MB" -f ((Get-ChildItem $SavantsHome -Recurse -Force | Measure-Object Length -Sum).Sum / 1MB)
    Remove-Item -Recurse -Force $SavantsHome
    Write-Host "  > Removed $SavantsHome ($size)" -ForegroundColor Green
} else {
    Write-Host "  > No .savants directory found" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "  Savants uninstalled." -ForegroundColor Green
Write-Host ""
Write-Host "  To reinstall: irm releases.savants.dev/latest/install.ps1 | iex" -ForegroundColor DarkGray
Write-Host ""
