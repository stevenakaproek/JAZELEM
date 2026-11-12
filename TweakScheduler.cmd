@echo off
rem Double-click to open Tweak Scheduler (asks for administrator rights).
start "" powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0TweakScheduler.ps1"
