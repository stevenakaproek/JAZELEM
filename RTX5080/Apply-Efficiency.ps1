# RTX 5080 Efficiency: 2700 MHz @ 900 mV, memory +1000, power 100%, Afterburner slot 2.
# Right-click > Run with PowerShell.  Options: -DryRun (preview), -Restore (undo using latest backup)
param([switch]$DryRun, [switch]$Restore)
& "$PSScriptRoot\RTX5080-Profile.ps1" -Name Efficiency -Slot 2 -TargetMHz 2700 -TargetMv 900 -MemOffsetMHz 1000 -DryRun:$DryRun -Restore:$Restore
