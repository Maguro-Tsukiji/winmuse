# Registers the Winmuse logon task: winmuse starts (tray-resident)
# when the user logs on, in the interactive session.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\install-task.ps1
#         ... -ExePath "C:\path\to\winmuse.exe"
param(
    [string]$ExePath = (Join-Path $PSScriptRoot "..\winmuse\target\release\winmuse.exe")
)
$ErrorActionPreference = "Stop"
$ExePath = [IO.Path]::GetFullPath($ExePath)
if (-not (Test-Path $ExePath)) { throw "winmuse.exe not found: $ExePath (build it first: cargo build --release)" }
$action = New-ScheduledTaskAction -Execute $ExePath
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName "Winmuse" -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
Write-Host "Task 'Winmuse' registered -> $ExePath"
Write-Host "It will start at the next logon. To start it now: Start-ScheduledTask -TaskName Winmuse"
