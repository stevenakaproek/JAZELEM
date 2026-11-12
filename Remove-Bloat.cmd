@echo off
rem Double-click: opens the Remove-Bloat menu in an elevated Windows PowerShell window.
powershell.exe -NoProfile -Command "Start-Process powershell.exe -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -NoExit -File \"%~dp0Remove-Bloat.ps1\"'"
