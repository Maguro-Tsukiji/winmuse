# Registers the Computer Use bridge as a logon scheduled task so it
# always runs inside your interactive desktop session (required: a
# session-0 process cannot see or drive the desktop).
#
# Run once, from this folder, in PowerShell:
#   powershell -ExecutionPolicy Bypass -File .\register-task.ps1
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$action = New-ScheduledTaskAction -Execute (Join-Path $here 'start-bridge.cmd')
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 0
Register-ScheduledTask -TaskName 'ComputerUseBridge' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
Write-Output 'TASK_REGISTERED'
Start-ScheduledTask -TaskName 'ComputerUseBridge'
Write-Output 'TASK_STARTED'
