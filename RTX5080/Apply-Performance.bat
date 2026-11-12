@echo off
rem RTX 5080 Performance: 2900 MHz @ 950 mV, memory +2000, Afterburner slot 4. Add -DryRun to preview or -Restore to undo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Apply-Performance.ps1" %*
if errorlevel 1 pause
