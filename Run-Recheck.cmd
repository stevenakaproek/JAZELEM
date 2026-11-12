@echo off
rem Double-click: runs the Remove-Bloat re-check task now (asks for admin) and shows the result.
powershell.exe -NoProfile -Command "Start-Process powershell.exe -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -NoExit -Command \"Start-ScheduledTask -TaskPath ''\Debloat\'' -TaskName ReapplyDebloat; ''Running re-check (40 s)...''; Start-Sleep 40; $left = Get-AppxPackage -AllUsers *Ink.Handwriting* | %% Name; if($left){ ''Still installed: '' + ($left -join '', '') } else { ''Ink Handwriting removed.'' }; Get-Content C:\ProgramData\Debloat\debloat.log -Tail 8\"'"
