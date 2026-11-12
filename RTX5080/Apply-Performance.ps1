# RTX 5080 Performance: 2900 MHz @ 950 mV, memory +2000, power 100%, Afterburner slot 4.
# Right-click > Run with PowerShell.  Options: -DryRun (preview), -Restore (undo using latest backup)
param([switch]$DryRun, [switch]$Restore)
& "$PSScriptRoot\RTX5080-Profile.ps1" -Name Performance -Slot 4 -TargetMHz 2900 -TargetMv 950 -MemOffsetMHz 2000 -DryRun:$DryRun -Restore:$Restore
