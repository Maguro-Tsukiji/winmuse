# Stops the resident winmuse process (the logon task stays registered;
# winmuse will start again at the next logon, or run
# Start-ScheduledTask -TaskName Winmuse to bring it back now).
Stop-Process -Name winmuse -Force -ErrorAction SilentlyContinue
Write-Host "winmuse stopped."
