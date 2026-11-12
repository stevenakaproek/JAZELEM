@echo off
rem Double-click: RTX 5080 Profile Manager (asks for admin, because MSI Afterburner needs it).
start "" powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0RTX5080-ProfileManager.ps1"
