<#
  Shared engine for the RTX 5080 undervolt profiles (MSI Afterburner).
  Run Apply-Efficiency / Apply-Balanced / Apply-Performance (.bat or .ps1) instead of this file.
  - Core: flat TargetMHz from TargetMv upward (points below get the same offset, capped at the target)

  What it does:
    1. Closes MSI Afterburner (it rewrites its profile files on exit).
    2. Backs up your GPU profile file next to this script (backups\ folder).
    3. Builds the curve from YOUR card's stock curve and saves it into a profile slot (default: 1).
    4. Starts Afterburner with that profile applied.

  Usage (right-click > Run with PowerShell, or use Apply-Balanced.bat):
    .\Apply-Balanced.ps1                  # apply Balanced into slot 1
    .\Apply-Balanced.ps1 -Slot 3          # use a different slot (1-5)
    .\Apply-Balanced.ps1 -DryRun          # show the curve only, change nothing
    .\Apply-Balanced.ps1 -Restore         # put back the most recent backup
#>
param(
    [ValidateRange(1,5)][int]$Slot = 1,
    [int]$TargetMHz = 2800,
    [int]$TargetMv = 925,
    [int]$MemOffsetMHz = 1500,
    [int]$PowerLimit = 100,
    [string]$Name = 'Balanced',
    [switch]$DryRun,
    [switch]$Restore
)
$ErrorActionPreference = 'Stop'

# --- elevate (Afterburner's folder is in Program Files) ---
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $DryRun) {
    $argList = @('-NoProfile','-ExecutionPolicy','Bypass','-File',"`"$PSCommandPath`"",'-Slot',$Slot,'-TargetMHz',$TargetMHz,'-TargetMv',$TargetMv,'-MemOffsetMHz',$MemOffsetMHz,'-PowerLimit',$PowerLimit,'-Name',$Name)
    if ($Restore) { $argList += '-Restore' }
    try { Start-Process powershell.exe -Verb RunAs -ArgumentList $argList -ErrorAction Stop }
    catch { Write-Host 'Admin permission is needed (click Yes on the prompt). Nothing was changed.' -ForegroundColor Red; Read-Host 'Press Enter to close' | Out-Null }
    exit
}

function Pause-End { if (-not $DryRun) { Write-Host ''; Read-Host 'Press Enter to close' | Out-Null } }

try {
    # --- locate Afterburner + the RTX 5080 profile file ---
    $abDir = $null
    try { $abDir = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\MSI\Afterburner' -ErrorAction Stop).InstallDir } catch {}
    if (-not $abDir) { $abDir = 'C:\Program Files (x86)\MSI Afterburner' }
    $abExe = Join-Path $abDir 'MSIAfterburner.exe'
    # Afterburner keeps a file for every card it has seen (an old card's file stays after a swap):
    # use the installed card's file, else the one Afterburner saved last.
    $files = @(Get-ChildItem -LiteralPath (Join-Path $abDir 'Profiles') -Filter 'VEN_10DE&DEV_2C02*.cfg' -ErrorAction SilentlyContinue)
    $cfg = $null
    foreach ($pnp in @(Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue | ForEach-Object { "$($_.PNPDeviceID)" })) {
        if (-not $cfg -and $pnp -match '^PCI\\(VEN_10DE&DEV_2C02[^\\]*)') {
            $want = $Matches[1] + '&'
            $cfg = $files | Where-Object { $_.Name.StartsWith($want, [StringComparison]::OrdinalIgnoreCase) } | Select-Object -First 1
        }
    }
    if (-not $cfg) { $cfg = $files | Sort-Object LastWriteTime -Descending | Select-Object -First 1 }
    if (-not $cfg) { throw "No RTX 5080 profile file (VEN_10DE&DEV_2C02*.cfg) found in $abDir\Profiles. Open Afterburner once, then close it and retry." }

    $backupDir = Join-Path $PSScriptRoot 'backups'

    # --- restore mode ---
    if ($Restore) {
        $last = Get-ChildItem -LiteralPath $backupDir -Filter '*.cfg' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (-not $last) { throw "No backups found in $backupDir" }
        Get-Process MSIAfterburner -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Seconds 2
        Copy-Item -LiteralPath $last.FullName -Destination $cfg.FullName -Force
        Write-Host "Restored $($last.Name). Starting Afterburner - press Reset in it if clocks still look tuned." -ForegroundColor Green
        Start-Process -FilePath $abExe
        Pause-End; exit
    }

    $lines = [System.Collections.Generic.List[string]](Get-Content -LiteralPath $cfg.FullName)

    # --- read the stock curve from [Startup] ---
    $start = $lines.IndexOf('[Startup]')
    if ($start -lt 0) { throw '[Startup] section not found in profile file.' }
    $curveLine = $null
    for ($i = $start + 1; $i -lt $lines.Count -and -not $lines[$i].StartsWith('['); $i++) {
        if ($lines[$i].StartsWith('VFCurve=')) { $curveLine = $lines[$i]; break }
    }
    if (-not $curveLine) { throw "Afterburner has not saved this card's voltage/frequency curve yet. Open Afterburner, leave every setting at stock, click Apply (the tick) once, close Afterburner and retry." }
    $hex = $curveLine.Substring(8)
    $b = [byte[]]::new($hex.Length / 2)
    for ($i = 0; $i -lt $b.Length; $i++) { $b[$i] = [Convert]::ToByte($hex.Substring($i * 2, 2), 16) }

    # layout: 12-byte header (version, point count, reserved) then points of {float mV, float MHz, float offsetMHz}
    $count = [BitConverter]::ToUInt32($b, 4)
    if ([BitConverter]::ToUInt32($b, 0) -ne 0x00020001 -or $count -lt 50 -or $count -gt 255) { throw ('Unexpected curve format (header {0:X8}, {1} points). Not touching anything.' -f [BitConverter]::ToUInt32($b,0), $count) }
    $pts = for ($k = 0; $k -lt $count; $k++) {
        $o = 12 + $k * 12
        [pscustomobject]@{ Idx = $k; Off = $o; mV = [BitConverter]::ToSingle($b, $o); Base = [BitConverter]::ToSingle($b, $o + 4); OldOfs = [BitConverter]::ToSingle($b, $o + 8) }
    }
    # sanity: voltages must climb from ~450 mV, base clocks must look like MHz
    if ($pts[0].mV -lt 300 -or $pts[0].mV -gt 700 -or $pts[$count-1].mV -le $pts[0].mV -or ($pts | Measure-Object Base -Maximum).Maximum -gt 5000) {
        throw 'Curve data does not look like a voltage/frequency table. Not touching anything.'
    }
    if ($pts | Where-Object { $_.OldOfs -ne 0 }) {
        Write-Host 'Note: the saved startup curve already has offsets; they will be replaced (base clocks are used).' -ForegroundColor Yellow
    }

    $anchor = $pts | Where-Object { $_.mV -ge $TargetMv } | Select-Object -First 1
    if (-not $anchor) { throw "No curve point at or above $TargetMv mV." }
    $lift = $TargetMHz - $anchor.Base
    if ($lift -gt 300) { throw "Target needs +$lift MHz over this card's stock curve at $($anchor.mV) mV. That's too aggressive for a 'works on most cards' profile." }

    # new curve: below anchor = stock + same lift (never above target); anchor and above = flat at target
    foreach ($p in $pts) {
        $want = if ($p.mV -lt $anchor.mV) { [Math]::Min($p.Base + $lift, $TargetMHz) } else { $TargetMHz }
        $ofs = [float]($want - $p.Base)
        [BitConverter]::GetBytes($ofs).CopyTo($b, $p.Off + 8)
        $p | Add-Member -NotePropertyName New -NotePropertyValue $want
    }
    $newHex = -join ($b | ForEach-Object { $_.ToString('X2') })

    Write-Host ("RTX 5080 {6}: {0} MHz @ {1} mV (stock there: {2} MHz, +{3}), memory +{4} MHz, power {5}%" -f $TargetMHz, $anchor.mV, $anchor.Base, $lift, $MemOffsetMHz, $PowerLimit, $Name) -ForegroundColor Cyan
    $pts | Where-Object { $_.mV -ge 850 -and $_.mV -le 1100 -and ($_.Idx % 3 -eq 0 -or $_.Idx -eq $anchor.Idx) } |
        Select-Object @{n='mV';e={$_.mV}}, @{n='Stock MHz';e={$_.Base}}, @{n='New MHz';e={$_.New}} | Format-Table -AutoSize | Out-Host

    if ($DryRun) { Write-Host 'Dry run: nothing was changed.'; exit }

    # --- close Afterburner, back up, write the profile slot ---
    $wasRunning = [bool](Get-Process MSIAfterburner -ErrorAction SilentlyContinue)
    if ($wasRunning) { Get-Process MSIAfterburner | Stop-Process -Force; Start-Sleep -Seconds 2 }

    New-Item -ItemType Directory -Force -Path $backupDir | Out-Null
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $backup = Join-Path $backupDir "$stamp-$($cfg.Name)"
    Copy-Item -LiteralPath $cfg.FullName -Destination $backup

    # copy fan settings from [Startup] so the profile doesn't change your fans
    $fanLines = @()
    for ($i = $start + 1; $i -lt $lines.Count -and -not $lines[$i].StartsWith('['); $i++) {
        if ($lines[$i] -match '^(FanMode|FanSpeed|FanMode2|FanSpeed2)=') { $fanLines += $lines[$i] }
    }
    $section = @("[Profile$Slot]", 'Format=2', 'CoreVoltageBoost=0', "PowerLimit=$PowerLimit", 'CoreClkBoost=0', "VFCurve=$newHex", "MemClkBoost=$($MemOffsetMHz * 1000)") + $fanLines

    # remove an existing section for this slot, then append ours
    $s = $lines.IndexOf("[Profile$Slot]")
    if ($s -ge 0) {
        $e = $s + 1
        while ($e -lt $lines.Count -and -not $lines[$e].StartsWith('[')) { $e++ }
        $lines.RemoveRange($s, $e - $s)
        Write-Host "Replaced existing profile slot $Slot (old copy is in the backup)." -ForegroundColor Yellow
    }
    $lines.AddRange([string[]]$section)
    Set-Content -LiteralPath $cfg.FullName -Value $lines -Encoding ASCII

    Write-Host "Saved to Afterburner profile slot $Slot. Backup: $backup" -ForegroundColor Green
    Start-Process -FilePath $abExe -ArgumentList "-Profile$Slot"
    Write-Host ''
    Write-Host "Check it worked: in Afterburner press Ctrl+F. The curve should be flat at ~$TargetMHz MHz from $($anchor.mV) mV up,"
    Write-Host "and the memory clock should show +$MemOffsetMHz. If not, run:  Apply-$Name.bat -Restore  and set it by hand."
    Write-Host 'Test stability before ticking "Apply overclocking at system startup".'
}
catch {
    Write-Host "ERROR: $($_.Exception.Message)" -ForegroundColor Red
}
Pause-End
