@echo off
rem RTX 5080 Efficiency: 2700 MHz @ 900 mV, memory +1000, Afterburner slot 2. Add -DryRun to preview or -Restore to undo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Apply-Efficiency.ps1" %*
if errorlevel 1 pause
