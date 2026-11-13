#Requires -Version 5.1
<#
  RTX 5080 Profile Manager
  One window to switch the RTX 5080 between the undervolt presets (via MSI Afterburner profile slots)
  and see which one is applied right now.

    Stock        slot 3   no offsets (Afterburner defaults)            memory +0
    Efficiency   slot 2   2700 MHz flat from 900 mV                    memory +1000
    Balanced     slot 1   2800 MHz flat from 925 mV                    memory +1500
    Performance  slot 4   2900 MHz flat from 950 mV                    memory +2000

  "Currently applied" is worked out two ways:
    - Live: the GPU's memory clock above stock (nvidia-smi) - each preset uses a different memory offset.
    - Saved: the settings Afterburner restores at startup ([Startup] section of its GPU profile file).

  If a preset's slot is missing or does not match, the app writes it (closes Afterburner first, backs the
  profile file up into .\backups, then reopens Afterburner with the preset applied). Otherwise it just tells
  the running Afterburner to load that slot.

  Launch with RTX5080-ProfileManager.cmd (double-click).
#>

# ---------------------------------------------------------------- elevation (Afterburner needs admin)
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    try { Start-Process powershell.exe -Verb RunAs -ArgumentList "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSCommandPath`"" }
    catch { Write-Host 'Administrator rights are required.' -ForegroundColor Red }
    return
}

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
trap {
    [System.Windows.Forms.MessageBox]::Show("Unexpected error:`n`n$($_.Exception.Message)`n`n$($_.InvocationInfo.PositionMessage)", 'RTX 5080 Profile Manager', 'OK', 'Error') | Out-Null
    continue
}

# ---------------------------------------------------------------- presets
$Presets = @(
    [pscustomobject]@{ Name = 'Stock';       Slot = 3; TargetMHz = 0;    TargetMv = 0;   Mem = 0;    Power = 100
                       Text = 'Factory behaviour - no undervolt, no memory overclock. Use this to reset or to compare.' }
    [pscustomobject]@{ Name = 'Efficiency';  Slot = 2; TargetMHz = 2700; TargetMv = 900; Mem = 1000; Power = 100
                       Text = 'Coolest and quietest. 2700 MHz at 900 mV, memory +1000.' }
    [pscustomobject]@{ Name = 'Balanced';    Slot = 1; TargetMHz = 2800; TargetMv = 925; Mem = 1500; Power = 100
                       Text = 'Everyday gaming. 2800 MHz at 925 mV, memory +1500.' }
    [pscustomobject]@{ Name = 'Performance'; Slot = 4; TargetMHz = 2900; TargetMv = 950; Mem = 2000; Power = 100
                       Text = 'Most fps. 2900 MHz at 950 mV, memory +2000. Runs warmest of the three undervolts.' }
)
$ClockTolerance = 30   # MHz: Afterburner re-saves curves with slightly different base points

# ---------------------------------------------------------------- Afterburner file access
function Get-AbDir {
    $d = $null
    try { $d = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\MSI\Afterburner' -ErrorAction Stop).InstallDir } catch { }
    if (-not $d) { $d = 'C:\Program Files (x86)\MSI Afterburner' }
    return $d
}
$AbDir = Get-AbDir
$AbExe = Join-Path $AbDir 'MSIAfterburner.exe'
$BackupDir = Join-Path $PSScriptRoot 'backups'

# Afterburner keeps a file for every card it has seen (an old card's file stays after a swap):
# use the installed card's file, else the one Afterburner saved last.
function Get-CfgFile {
    $files = @(Get-ChildItem -LiteralPath (Join-Path $AbDir 'Profiles') -Filter 'VEN_10DE&DEV_2C02*.cfg' -ErrorAction SilentlyContinue)
    foreach ($pnp in @(Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue | ForEach-Object { "$($_.PNPDeviceID)" })) {
        if ($pnp -match '^PCI\\(VEN_10DE&DEV_2C02[^\\]*)') {
            $want = $Matches[1] + '&'
            $own = $files | Where-Object { $_.Name.StartsWith($want, [StringComparison]::OrdinalIgnoreCase) } | Select-Object -First 1
            if ($own) { return $own }
        }
    }
    $files | Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

function Read-Cfg {
    $f = Get-CfgFile
    if (-not $f) { return $null }
    $lines = [System.Collections.Generic.List[string]](Get-Content -LiteralPath $f.FullName)
    [pscustomobject]@{ File = $f; Lines = $lines }
}

function Get-Section($cfg, [string]$name) {
    $i = $cfg.Lines.IndexOf("[$name]")
    if ($i -lt 0) { return $null }
    $h = @{}
    for ($j = $i + 1; $j -lt $cfg.Lines.Count -and -not $cfg.Lines[$j].StartsWith('['); $j++) {
        $k, $v = $cfg.Lines[$j] -split '=', 2
        $h[$k] = $v
    }
    return $h
}

# VFCurve hex: 12-byte header (version 0x00020001, point count) then {float mV, float base MHz, float offset MHz}
function ConvertFrom-Curve([string]$hex) {
    if (-not $hex) { return $null }
    $b = [byte[]]::new($hex.Length / 2)
    for ($i = 0; $i -lt $b.Length; $i++) { $b[$i] = [Convert]::ToByte($hex.Substring($i * 2, 2), 16) }
    if ([BitConverter]::ToUInt32($b, 0) -ne 0x00020001) { return $null }
    $n = [BitConverter]::ToUInt32($b, 4)
    if ($n -lt 50 -or $n -gt 255) { return $null }
    $pts = for ($k = 0; $k -lt $n; $k++) {
        $o = 12 + $k * 12
        [pscustomobject]@{ Off = $o; mV = [BitConverter]::ToSingle($b, $o); Base = [BitConverter]::ToSingle($b, $o + 4); Ofs = [BitConverter]::ToSingle($b, $o + 8) }
    }
    [pscustomobject]@{ Bytes = $b; Points = $pts }
}

# Does a saved section (Startup / ProfileN) contain this preset?
function Test-SectionIsPreset($sec, $p) {
    if (-not $sec -or -not $sec.VFCurve) { return $false }
    $mem = [int]([double]$sec.MemClkBoost / 1000)
    if ($mem -ne $p.Mem) { return $false }
    if ($sec.PowerLimit -and [int]$sec.PowerLimit -ne $p.Power) { return $false }
    $c = ConvertFrom-Curve $sec.VFCurve
    if (-not $c) { return $false }
    if ($p.TargetMHz -eq 0) {
        $maxOfs = ($c.Points | ForEach-Object { [Math]::Abs($_.Ofs) } | Measure-Object -Maximum).Maximum
        return ($maxOfs -lt 1 -and [int]$sec.CoreClkBoost -eq 0)
    }
    $maxEff = ($c.Points | ForEach-Object { $_.Base + $_.Ofs } | Measure-Object -Maximum).Maximum
    return ([Math]::Abs($maxEff - $p.TargetMHz) -le $ClockTolerance)
}

function Find-PresetForSection($sec) {
    foreach ($p in $Presets) { if (Test-SectionIsPreset $sec $p) { return $p } }
    return $null
}

# Build a preset's curve from the card's own stock curve (same method as RTX5080-Profile.ps1)
function New-PresetSectionLines($cfg, $p) {
    $startup = Get-Section $cfg 'Startup'
    if (-not $startup -or -not $startup.VFCurve) { throw "Afterburner has not saved this card's voltage/frequency curve yet. Open Afterburner, leave every setting at stock, click Apply (the tick) once, close Afterburner and retry." }
    $c = ConvertFrom-Curve $startup.VFCurve
    if (-not $c) { throw 'Unexpected curve format in the Afterburner profile. Not touching anything.' }
    $pts = $c.Points; $b = $c.Bytes
    if ($pts[0].mV -lt 300 -or $pts[0].mV -gt 700 -or ($pts | Measure-Object Base -Maximum).Maximum -gt 5000) { throw 'Curve data does not look like a voltage/frequency table. Not touching anything.' }
    if ($p.TargetMHz -eq 0) {
        foreach ($pt in $pts) { [BitConverter]::GetBytes([float]0).CopyTo($b, $pt.Off + 8) }
    } else {
        $anchor = $pts | Where-Object { $_.mV -ge $p.TargetMv } | Select-Object -First 1
        if (-not $anchor) { throw "No curve point at or above $($p.TargetMv) mV." }
        $lift = $p.TargetMHz - $anchor.Base
        if ($lift -gt 300) { throw "Preset needs +$lift MHz over this card's stock curve at $($anchor.mV) mV - too aggressive. Not writing it." }
        foreach ($pt in $pts) {
            $want = if ($pt.mV -lt $anchor.mV) { [Math]::Min($pt.Base + $lift, $p.TargetMHz) } else { $p.TargetMHz }
            [BitConverter]::GetBytes([float]($want - $pt.Base)).CopyTo($b, $pt.Off + 8)
        }
    }
    $hex = -join ($b | ForEach-Object { $_.ToString('X2') })
    $fan = @(); foreach ($k in 'FanMode', 'FanSpeed', 'FanMode2', 'FanSpeed2') { if ($startup.ContainsKey($k)) { $fan += "$k=$($startup[$k])" } }
    return @("[Profile$($p.Slot)]", 'Format=2', 'CoreVoltageBoost=0', "PowerLimit=$($p.Power)", 'CoreClkBoost=0', "VFCurve=$hex", "MemClkBoost=$($p.Mem * 1000)") + $fan
}

function Write-PresetSlot($p) {
    if (Get-Process MSIAfterburner -ErrorAction SilentlyContinue) { Get-Process MSIAfterburner | Stop-Process -Force; Start-Sleep -Seconds 2 }
    $cfg = Read-Cfg
    if (-not $cfg) { throw "No RTX 5080 profile file found in $AbDir\Profiles. Open Afterburner once, close it, and retry." }
    New-Item -ItemType Directory -Force -Path $BackupDir | Out-Null
    $backup = Join-Path $BackupDir ("{0}-{1}" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), $cfg.File.Name)
    Copy-Item -LiteralPath $cfg.File.FullName -Destination $backup
    $section = New-PresetSectionLines $cfg $p
    $s = $cfg.Lines.IndexOf("[Profile$($p.Slot)]")
    if ($s -ge 0) { $e = $s + 1; while ($e -lt $cfg.Lines.Count -and -not $cfg.Lines[$e].StartsWith('[')) { $e++ }; $cfg.Lines.RemoveRange($s, $e - $s) }
    $cfg.Lines.AddRange([string[]]$section)
    Set-Content -LiteralPath $cfg.File.FullName -Value $cfg.Lines -Encoding ASCII
    return $backup
}

# ---------------------------------------------------------------- live GPU readout
$Smi = Join-Path $env:SystemRoot 'System32\nvidia-smi.exe'
function Get-GpuLive {
    if (-not (Test-Path $Smi)) { return $null }
    $o = & $Smi --query-gpu=clocks.gr,clocks.mem,clocks.max.memory,power.draw,temperature.gpu,pstate,utilization.gpu --format=csv,noheader,nounits 2>$null
    if (-not $o) { return $null }
    $f = ($o | Select-Object -First 1) -split ',\s*'
    $mem = [int]$f[1]; $memMax = [int]$f[2]
    [pscustomobject]@{
        Core = [int]$f[0]; Mem = $mem; MemStock = $memMax; MemOffset = ($mem - $memMax)
        Power = [double]$f[3]; Temp = [int]$f[4]; PState = $f[5].Trim(); Util = [int]$f[6]
        FullClocks = ($f[5].Trim() -eq 'P0' -and $mem -ge $memMax)   # memory only runs at full speed in P0
    }
}

# ---------------------------------------------------------------- window
$form = New-Object System.Windows.Forms.Form
$form.Text = 'RTX 5080 Profile Manager'
$form.ClientSize = New-Object System.Drawing.Size(600, 560)
$form.FormBorderStyle = 'FixedDialog'; $form.MaximizeBox = $false; $form.StartPosition = 'CenterScreen'
$form.Font = New-Object System.Drawing.Font('Segoe UI', 9)
$bold = New-Object System.Drawing.Font('Segoe UI', 9, [System.Drawing.FontStyle]::Bold)
$big = New-Object System.Drawing.Font('Segoe UI', 13, [System.Drawing.FontStyle]::Bold)

$grpCur = New-Object System.Windows.Forms.GroupBox
$grpCur.Text = 'Currently applied'; $grpCur.Location = New-Object System.Drawing.Point(12, 10); $grpCur.Size = New-Object System.Drawing.Size(576, 150)
$lblCur = New-Object System.Windows.Forms.Label
$lblCur.Location = New-Object System.Drawing.Point(14, 22); $lblCur.Size = New-Object System.Drawing.Size(550, 28); $lblCur.Font = $big
$lblCurInfo = New-Object System.Windows.Forms.Label
$lblCurInfo.Location = New-Object System.Drawing.Point(14, 54); $lblCurInfo.Size = New-Object System.Drawing.Size(550, 90)
$grpCur.Controls.AddRange(@($lblCur, $lblCurInfo))

$grpPre = New-Object System.Windows.Forms.GroupBox
$grpPre.Text = 'Presets'; $grpPre.Location = New-Object System.Drawing.Point(12, 168); $grpPre.Size = New-Object System.Drawing.Size(576, 300)
$radios = @(); $y = 22
foreach ($p in $Presets) {
    $r = New-Object System.Windows.Forms.RadioButton
    $r.Location = New-Object System.Drawing.Point(14, $y); $r.Size = New-Object System.Drawing.Size(550, 22); $r.Font = $bold; $r.Tag = $p
    $r.Text = "$($p.Name)   (Afterburner slot $($p.Slot))"
    $l = New-Object System.Windows.Forms.Label
    $l.Location = New-Object System.Drawing.Point(32, ($y + 23)); $l.Size = New-Object System.Drawing.Size(530, 36); $l.Text = $p.Text
    $grpPre.Controls.AddRange(@($r, $l)); $radios += $r; $y += 68
}

$btnApply = New-Object System.Windows.Forms.Button
$btnApply.Text = 'Apply selected preset'; $btnApply.Location = New-Object System.Drawing.Point(12, 478); $btnApply.Size = New-Object System.Drawing.Size(170, 32)
$btnRestore = New-Object System.Windows.Forms.Button
$btnRestore.Text = 'Restore last backup'; $btnRestore.Location = New-Object System.Drawing.Point(190, 478); $btnRestore.Size = New-Object System.Drawing.Size(150, 32)
$btnRefresh = New-Object System.Windows.Forms.Button
$btnRefresh.Text = 'Refresh'; $btnRefresh.Location = New-Object System.Drawing.Point(348, 478); $btnRefresh.Size = New-Object System.Drawing.Size(90, 32)
$btnClose = New-Object System.Windows.Forms.Button
$btnClose.Text = 'Close'; $btnClose.Location = New-Object System.Drawing.Point(498, 478); $btnClose.Size = New-Object System.Drawing.Size(90, 32)
$lblStatus = New-Object System.Windows.Forms.Label
$lblStatus.Location = New-Object System.Drawing.Point(12, 518); $lblStatus.Size = New-Object System.Drawing.Size(576, 36); $lblStatus.ForeColor = [System.Drawing.Color]::DimGray
$form.Controls.AddRange(@($grpCur, $grpPre, $btnApply, $btnRestore, $btnRefresh, $btnClose, $lblStatus))

# ---------------------------------------------------------------- logic
function Get-CurrentState {
    $cfg = Read-Cfg
    $saved = if ($cfg) { Find-PresetForSection (Get-Section $cfg 'Startup') } else { $null }
    $live = Get-GpuLive
    $livePreset = $null
    if ($live -and $live.FullClocks) { $livePreset = $Presets | Where-Object { [Math]::Abs($live.MemOffset - $_.Mem) -le 5 } | Select-Object -First 1 }
    [pscustomobject]@{ Cfg = $cfg; Saved = $saved; Live = $live; LivePreset = $livePreset; AbRunning = [bool](Get-Process MSIAfterburner -ErrorAction SilentlyContinue) }
}

function Update-View {
    $st = Get-CurrentState
    $name = if ($st.LivePreset) { $st.LivePreset.Name } elseif ($st.Saved) { $st.Saved.Name } else { 'Custom / unknown' }
    $lblCur.Text = $name
    $lblCur.ForeColor = if ($name -eq 'Custom / unknown') { [System.Drawing.Color]::DarkOrange } else { [System.Drawing.Color]::FromArgb(0, 120, 60) }
    $lines = @()
    if ($st.Live) {
        $memTxt = if ($st.Live.FullClocks) { "memory {0} MHz (stock {1}, offset {2:+#;-#;0})" -f $st.Live.Mem, $st.Live.MemStock, $st.Live.MemOffset } else { "memory {0} MHz (GPU idle in {1} - offset shows under load)" -f $st.Live.Mem, $st.Live.PState }
        $lines += "Live:  core $($st.Live.Core) MHz,  $memTxt"
        $lines += ("        {0:N0} W,  {1} C,  load {2}%,  {3}" -f $st.Live.Power, $st.Live.Temp, $st.Live.Util, $st.Live.PState)
        $lines += "Live check:  $(if ($st.LivePreset) { "memory offset matches $($st.LivePreset.Name)" } elseif ($st.Live.FullClocks) { 'memory offset matches no preset' } else { 'waiting for the GPU to clock up' })"
    } else { $lines += 'Live: nvidia-smi not available.' }
    $lines += "Afterburner saved settings:  $(if ($st.Saved) { $st.Saved.Name } else { 'custom / not a preset' })    (Afterburner $(if ($st.AbRunning) { 'running' } else { 'NOT running - settings only apply while it runs' }))"
    $lblCurInfo.Text = $lines -join "`r`n"
    $current = if ($st.LivePreset) { $st.LivePreset } else { $st.Saved }
    foreach ($r in $radios) {
        $r.Text = "$($r.Tag.Name)   (Afterburner slot $($r.Tag.Slot))" + $(if ($current -and $r.Tag.Name -eq $current.Name) { '   <- current' } else { '' })
        if (-not ($radios | Where-Object Checked)) { $r.Checked = ($current -and $r.Tag.Name -eq $current.Name) }
    }
}

function Wait-ForLive($p) {
    for ($i = 0; $i -lt 10; $i++) {
        Start-Sleep -Seconds 1; [System.Windows.Forms.Application]::DoEvents()
        $l = Get-GpuLive
        if ($l -and $l.FullClocks -and [Math]::Abs($l.MemOffset - $p.Mem) -le 5) { return 'confirmed live (memory offset matches)' }
    }
    $l = Get-GpuLive
    if ($l -and -not $l.FullClocks) { return 'sent - the GPU is idle, so the live check confirms it once a game is running' }
    return 'sent, but the live memory offset does not match yet - check Afterburner'
}

$btnApply.Add_Click({
    $sel = $radios | Where-Object Checked | Select-Object -First 1
    if (-not $sel) { $lblStatus.Text = 'Pick a preset first.'; return }
    $p = $sel.Tag
    $form.Cursor = 'WaitCursor'
    try {
        $cfg = Read-Cfg
        if (-not $cfg) { throw "No RTX 5080 profile file in $AbDir\Profiles. Open Afterburner once, close it, and retry." }
        $slotOk = Test-SectionIsPreset (Get-Section $cfg "Profile$($p.Slot)") $p
        $note = ''
        if (-not $slotOk) {
            $msg = "Afterburner slot $($p.Slot) does not contain the $($p.Name) preset yet.`n`nThe app will close Afterburner, back up its profile file, write the preset into slot $($p.Slot), and reopen Afterburner. Continue?"
            if ([System.Windows.Forms.MessageBox]::Show($msg, $p.Name, 'YesNo', 'Question') -ne 'Yes') { return }
            $backup = Write-PresetSlot $p
            $note = "  Slot $($p.Slot) written (backup: $(Split-Path $backup -Leaf))."
        }
        Start-Process -FilePath $AbExe -ArgumentList "-Profile$($p.Slot)"
        $lblStatus.Text = "Applying $($p.Name)...$note"
        $result = Wait-ForLive $p
        $lblStatus.Text = "$($p.Name): $result.$note"
    } catch {
        [System.Windows.Forms.MessageBox]::Show("Could not apply $($p.Name):`n$($_.Exception.Message)", 'RTX 5080 Profile Manager', 'OK', 'Warning') | Out-Null
    } finally { $form.Cursor = 'Default'; Update-View }
})

$btnRestore.Add_Click({
    $last = Get-ChildItem -LiteralPath $BackupDir -Filter '*.cfg' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $last) { $lblStatus.Text = "No backups in $BackupDir."; return }
    if ([System.Windows.Forms.MessageBox]::Show("Put back the Afterburner profile file from $($last.LastWriteTime)?`n`n$($last.Name)", 'Restore', 'YesNo', 'Question') -ne 'Yes') { return }
    $cfg = Read-Cfg
    Get-Process MSIAfterburner -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep -Seconds 2
    Copy-Item -LiteralPath $last.FullName -Destination $cfg.File.FullName -Force
    Start-Process -FilePath $AbExe
    $lblStatus.Text = "Restored $($last.Name) and restarted Afterburner."
    Update-View
})

$btnRefresh.Add_Click({ Update-View })
$btnClose.Add_Click({ $form.Close() })

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 3000
$timer.Add_Tick({ Update-View })
$form.Add_Shown({ Update-View; $timer.Start() })
$form.Add_FormClosed({ $timer.Stop() })

if (-not (Test-Path $AbExe)) {
    [System.Windows.Forms.MessageBox]::Show("MSI Afterburner was not found at:`n$AbExe", 'RTX 5080 Profile Manager', 'OK', 'Error') | Out-Null
    return
}
[void]$form.ShowDialog()
