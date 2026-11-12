@echo off
rem Double-click to open Service Manager (asks for administrator rights).
start "" powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0ServiceManager.ps1"
