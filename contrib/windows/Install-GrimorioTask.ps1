# Registers grimoriod to start at user logon via Windows Task Scheduler.
#
# IMPORTANT: grimorio's IPC currently uses Unix domain sockets (see src/ipc.rs),
# which are gated to Unix targets, so `grimoriod` does not build for native
# Windows today. Use this under WSL, or treat this script as the scheduling
# template to use once a Windows (named-pipe) transport is available.
#
# Usage (from an elevated-not-required PowerShell prompt):
#   powershell -ExecutionPolicy Bypass -File .\Install-GrimorioTask.ps1
#
# Uninstall:
#   Unregister-ScheduledTask -TaskName 'grimoriod' -Confirm:$false

$ErrorActionPreference = 'Stop'

# Path to the daemon binary. Edit if you installed it elsewhere.
$Exe       = Join-Path $env:USERPROFILE '.cargo\bin\grimoriod.exe'
$SocketDir = Join-Path $env:USERPROFILE '.grimorio'

if (-not (Test-Path $Exe)) {
    Write-Warning "grimoriod not found at $Exe. Edit the Exe path at the top of this script."
}

# The daemon does not create the socket's parent directory; ensure it exists.
New-Item -ItemType Directory -Force -Path $SocketDir | Out-Null

$action = New-ScheduledTaskAction -Execute $Exe -Argument '--timeout 300'
$trigger = New-ScheduledTaskTrigger -AtLogOn
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1)
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive

Register-ScheduledTask `
    -TaskName 'grimoriod' `
    -Description 'grimorio secret daemon' `
    -Action $action `
    -Trigger $trigger `
    -Settings $settings `
    -Principal $principal `
    -Force

Write-Host "Registered scheduled task 'grimoriod'. It will start at your next logon."
Write-Host "Start it now with: Start-ScheduledTask -TaskName 'grimoriod'"
