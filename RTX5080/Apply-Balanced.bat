@echo off
rem RTX 5080 Balanced: 2800 MHz @ 925 mV, memory +1500, Afterburner slot 1. Add -DryRun to preview or -Restore to undo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Apply-Balanced.ps1" %*
if errorlevel 1 pause
