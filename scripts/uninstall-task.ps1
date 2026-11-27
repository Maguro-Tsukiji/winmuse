# Removes the Winmuse logon task and stops the resident process.
$ErrorActionPreference = "SilentlyContinue"
Stop-ScheduledTask -TaskName "Winmuse"
Stop-Process -Name winmuse -Force
Unregister-ScheduledTask -TaskName "Winmuse" -Confirm:$false
Write-Host "Task 'Winmuse' removed and winmuse stopped."
