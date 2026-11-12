@echo off
rem Double-click to open Startup Cleaner (asks for administrator rights).
start "" powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0StartupCleaner.ps1"
