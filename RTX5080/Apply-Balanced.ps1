# RTX 5080 Balanced: 2800 MHz @ 925 mV, memory +1500, power 100%, Afterburner slot 1.
# Right-click > Run with PowerShell.  Options: -DryRun (preview), -Restore (undo using latest backup)
param([switch]$DryRun, [switch]$Restore)
& "$PSScriptRoot\RTX5080-Profile.ps1" -Name Balanced -Slot 1 -TargetMHz 2800 -TargetMv 925 -MemOffsetMHz 1500 -DryRun:$DryRun -Restore:$Restore
