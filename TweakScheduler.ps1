#Requires -Version 5.1
<#
  Tweak Scheduler - shows the current Windows CPU scheduler setting (Win32PrioritySeparation)
  and switches between three presets, with undo.

  The setting controls how long a thread may run before Windows lets another one in (the "quantum"),
  whether the foreground app gets longer turns than background apps, and how big a priority boost
  the foreground app's threads get when they wake up.

  This is the same value Bitsum TweakScheduler edits - use one tool or the other, not both.

  Registry: HKLM\SYSTEM\CurrentControlSet\Control\PriorityControl\Win32PrioritySeparation
  History:  %LOCALAPPDATA%\TweakScheduler\history.json

  Launch with TweakScheduler.cmd (double-click), or:
      powershell -ExecutionPolicy Bypass -File "<folder>\TweakScheduler.ps1"
#>

# ---------------------------------------------------------------- elevation
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    try {
        Start-Process powershell.exe -Verb RunAs -ArgumentList "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSCommandPath`""
    } catch {
        Write-Host 'Administrator rights are required.' -ForegroundColor Red
    }
    return
}

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

trap {
    [System.Windows.Forms.MessageBox]::Show("Unexpected error:`n`n$($_.Exception.Message)`n`n$($_.InvocationInfo.PositionMessage)", 'Tweak Scheduler', 'OK', 'Error') | Out-Null
    continue
}

# ---------------------------------------------------------------- data
$RegKey   = 'HKLM:\SYSTEM\CurrentControlSet\Control\PriorityControl'
$RegName  = 'Win32PrioritySeparation'
$DataDir  = Join-Path $env:LOCALAPPDATA 'TweakScheduler'
$HistFile = Join-Path $DataDir 'history.json'
if (-not (Test-Path $DataDir)) { New-Item -ItemType Directory -Path $DataDir | Out-Null }

# The three presets. Value = the registry number (shown as hex "bitmask" in Bitsum TweakScheduler).
$Presets = @(
    @{ Key = 1; Name = '1 - Windows default  (recommended for Halo Infinite)'; Value = 0x02
       Text = 'What Windows ships with, and the best-supported choice: the game in focus already gets turns 3x longer than background apps plus the maximum wake-up boost. No controlled benchmark shows any other value beating it.' }
    @{ Key = 2; Name = '2 - Game  (community favourite)'; Value = 0x2A
       Text = 'The value most gaming guides call the "game" setting. The game gets exactly what the default gives it; the only change is that background apps (Discord, capture) get equally long turns. No measured benefit - try it only as an A/B test.' }
    @{ Key = 3; Name = '3 - Long turns  (alternate to test)'; Value = 0x16
       Text = 'Keeps the 3:1 advantage and the wake-up boost but doubles every turn, so threads are switched out half as often when they share a core. Sound in theory, unmeasured in practice - keep it only if your own captures show a repeatable gain.' }
)

$script:History = New-Object System.Collections.ArrayList

# ---------------------------------------------------------------- decoding
# Bits 5-4: interval (1 = long, 2 = short, 0/3 = Windows picks: short on desktop Windows)
# Bits 3-2: 1 = variable, 2 = fixed, 0/3 = Windows picks: variable on desktop Windows
# Bits 1-0: foreground boost 0 / 1 / 2  (3 is treated as 2)
function Get-Decoded([int]$value) {
    $value = $value -band 0x3F          # Windows only reads the low 6 bits
    $i = ($value -shr 4) -band 3
    $t = ($value -shr 2) -band 3
    $b = $value -band 3
    if ($b -eq 3) { $b = 2 }
    $long  = ($i -eq 1)
    $fixed = ($t -eq 2)
    $intervalText = if ($i -eq 1) { 'Long' } elseif ($i -eq 2) { 'Short' } else { 'Short (Windows default)' }
    $typeText     = if ($t -eq 2) { 'Fixed' } elseif ($t -eq 1) { 'Variable' } else { 'Variable (Windows default)' }

    # Quantum table in "units" (3 units = 1 clock tick of 15.625 ms)
    $table = if ($fixed) { if ($long) { 36, 36, 36 } else { 18, 18, 18 } } else { if ($long) { 12, 24, 36 } else { 6, 12, 18 } }
    $fgMs = [math]::Round($table[$b] / 3 * 15.625, 1)
    $bgMs = [math]::Round($table[0] / 3 * 15.625, 1)
    $ratio = if ($fixed) { '1:1 (equal turns)' } else { "$($b + 1):1" }

    [pscustomobject]@{
        Value = $value; Hex = ('0x{0:X2}' -f $value); Interval = $intervalText; Type = $typeText
        Boost = $b; Fixed = $fixed; Long = $long; Ratio = $ratio; FgMs = $fgMs; BgMs = $bgMs
        # Two values behave the same when interval, type and boost all resolve to the same thing
        Signature = "$([int]$long)|$([int]$fixed)|$b"
    }
}

function Get-Current {
    $v = (Get-ItemProperty -LiteralPath $RegKey -Name $RegName -ErrorAction SilentlyContinue).$RegName
    if ($null -eq $v) { return 2 }
    return [int]$v
}

# ---------------------------------------------------------------- live apply
# Writing the registry alone only takes effect at the next boot. Windows' own Performance Options
# dialog also tells the running kernel; we do the same so the change applies immediately.
$native = @'
using System;
using System.Runtime.InteropServices;
public static class TsNative {
    [DllImport("ntdll.dll")] static extern int NtSetSystemInformation(int infoClass, ref uint info, int length);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool LookupPrivilegeValue(string system, string name, out long luid);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TP newState, int len, IntPtr prev, IntPtr retLen);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [StructLayout(LayoutKind.Sequential, Pack = 1)] struct TP { public int Count; public long Luid; public int Attr; }

    static void Enable(string privilege) {
        IntPtr token;
        if (!OpenProcessToken(GetCurrentProcess(), 0x28, out token)) return;   // ADJUST_PRIVILEGES | QUERY
        try {
            TP tp; tp.Count = 1; tp.Attr = 2;                                    // SE_PRIVILEGE_ENABLED
            if (LookupPrivilegeValue(null, privilege, out tp.Luid)) AdjustTokenPrivileges(token, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero);
        } finally { CloseHandle(token); }
    }

    // Returns the NTSTATUS (0 = applied to the running system).
    public static int SetPrioritySeparation(uint value) {
        Enable("SeIncreaseBasePriorityPrivilege");
        Enable("SeTcbPrivilege");
        return NtSetSystemInformation(39, ref value, 4);                         // SystemPrioritySeparation
    }
}
'@
try { Add-Type -TypeDefinition $native -ErrorAction Stop } catch { }

# Returns a short text describing how the change took effect; throws if the registry write fails.
function Set-Scheduler([int]$value) {
    Set-ItemProperty -LiteralPath $RegKey -Name $RegName -Value $value -Type DWord -ErrorAction Stop
    $status = -1
    try { $status = [TsNative]::SetPrioritySeparation([uint32]$value) } catch { }
    if ($status -eq 0) { return 'Applied now - no restart needed.' }
    # The direct kernel call needs a privilege administrators normally lack. Windows usually picks the
    # registry value up on its own, but that cannot be confirmed from here - so say so.
    return 'Saved. Restart the PC to be certain it is in effect.'
}

# ---------------------------------------------------------------- history
function Load-History {
    $script:History.Clear()
    if (Test-Path $HistFile) {
        try { foreach ($h in @(ConvertFrom-Json (Get-Content -LiteralPath $HistFile -Raw))) { if ($h) { [void]$script:History.Add($h) } } } catch { }
    }
}
function Save-History { ConvertTo-Json -InputObject @($script:History.ToArray()) -Depth 3 | Set-Content -LiteralPath $HistFile -Encoding UTF8 }

# ---------------------------------------------------------------- window
$form = New-Object System.Windows.Forms.Form
$form.Text = 'Tweak Scheduler'
$form.ClientSize = New-Object System.Drawing.Size(640, 560)
$form.FormBorderStyle = 'FixedDialog'; $form.MaximizeBox = $false
$form.StartPosition = 'CenterScreen'
$form.Font = New-Object System.Drawing.Font('Segoe UI', 9)
$bold = New-Object System.Drawing.Font('Segoe UI', 9, [System.Drawing.FontStyle]::Bold)

$grpCur = New-Object System.Windows.Forms.GroupBox
$grpCur.Text = 'Current schedule'; $grpCur.Location = New-Object System.Drawing.Point(12, 10); $grpCur.Size = New-Object System.Drawing.Size(616, 132)
$lblCurHead = New-Object System.Windows.Forms.Label
$lblCurHead.Location = New-Object System.Drawing.Point(14, 24); $lblCurHead.Size = New-Object System.Drawing.Size(590, 20); $lblCurHead.Font = $bold
$lblCurBody = New-Object System.Windows.Forms.Label
$lblCurBody.Location = New-Object System.Drawing.Point(14, 48); $lblCurBody.Size = New-Object System.Drawing.Size(590, 78)
$grpCur.Controls.AddRange(@($lblCurHead, $lblCurBody))

$grpPre = New-Object System.Windows.Forms.GroupBox
$grpPre.Text = 'Presets'; $grpPre.Location = New-Object System.Drawing.Point(12, 150); $grpPre.Size = New-Object System.Drawing.Size(616, 330)

$radios = @()
$y = 22
foreach ($p in $Presets) {
    $d = Get-Decoded $p.Value
    $r = New-Object System.Windows.Forms.RadioButton
    $r.Location = New-Object System.Drawing.Point(14, $y); $r.Size = New-Object System.Drawing.Size(590, 22); $r.Font = $bold
    $r.Text = "$($p.Name)    (bitmask $($d.Hex) = $($p.Value))"
    $r.Tag = $p
    $l1 = New-Object System.Windows.Forms.Label
    $l1.Location = New-Object System.Drawing.Point(32, ($y + 24)); $l1.Size = New-Object System.Drawing.Size(572, 18); $l1.ForeColor = [System.Drawing.Color]::FromArgb(0, 90, 158)
    $l1.Text = "$($d.Interval), $($d.Type), foreground boost $($d.Boost)  -  game turn $($d.FgMs) ms, background turn $($d.BgMs) ms"
    $l2 = New-Object System.Windows.Forms.Label
    $l2.Location = New-Object System.Drawing.Point(32, ($y + 44)); $l2.Size = New-Object System.Drawing.Size(572, 50)
    $l2.Text = $p.Text
    $grpPre.Controls.AddRange(@($r, $l1, $l2))
    $radios += $r
    $y += 102
}

$btnApply = New-Object System.Windows.Forms.Button
$btnApply.Text = 'Apply selected preset'; $btnApply.Location = New-Object System.Drawing.Point(12, 490); $btnApply.Size = New-Object System.Drawing.Size(170, 30)
$btnUndo = New-Object System.Windows.Forms.Button
$btnUndo.Text = 'Undo last change'; $btnUndo.Location = New-Object System.Drawing.Point(190, 490); $btnUndo.Size = New-Object System.Drawing.Size(140, 30)
$btnClose = New-Object System.Windows.Forms.Button
$btnClose.Text = 'Close'; $btnClose.Location = New-Object System.Drawing.Point(538, 490); $btnClose.Size = New-Object System.Drawing.Size(90, 30)
$lblStatus = New-Object System.Windows.Forms.Label
$lblStatus.Location = New-Object System.Drawing.Point(12, 530); $lblStatus.Size = New-Object System.Drawing.Size(616, 22); $lblStatus.ForeColor = [System.Drawing.Color]::DimGray

$form.Controls.AddRange(@($grpCur, $grpPre, $btnApply, $btnUndo, $btnClose, $lblStatus))

# ---------------------------------------------------------------- logic
function Update-View {
    $cur = Get-Current
    $d = Get-Decoded $cur
    $match = $Presets | Where-Object { (Get-Decoded $_.Value).Signature -eq $d.Signature } | Select-Object -First 1
    $name = if ($match) { $match.Name } else { 'Custom (not one of the presets)' }
    $lblCurHead.Text = "$name    -    bitmask $($d.Hex) (decimal $cur)"
    $boostText = if ($d.Boost -eq 0) { 'none' } else { "+$($d.Boost) priority levels when the foreground app's threads wake up" }
    $lblCurBody.Text = @(
        "Interval length:   $($d.Interval)"
        "Interval type:       $($d.Type)   -   foreground : background turn length = $($d.Ratio)"
        "Turn length:         foreground ~$($d.FgMs) ms, background ~$($d.BgMs) ms  (nominal; only matters when threads share a core)"
        "Foreground boost: $boostText"
    ) -join "`r`n"
    foreach ($r in $radios) { $r.Checked = ($match -and $r.Tag.Key -eq $match.Key) }
    $btnUndo.Enabled = (@($script:History | Where-Object { -not $_.Undone }).Count -gt 0)
}

# Returns $true when the value is now set (or already was).
function Invoke-Apply([int]$value, [string]$label, [bool]$record = $true) {
    $from = Get-Current
    if ($from -eq $value) { $lblStatus.Text = "Already set to $label."; return $true }
    try {
        $how = Set-Scheduler $value
    } catch {
        [System.Windows.Forms.MessageBox]::Show("Could not change the setting:`n$($_.Exception.Message)", 'Tweak Scheduler', 'OK', 'Warning') | Out-Null
        return $false
    }
    if ($record) {
        [void]$script:History.Add([pscustomobject]@{ Time = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss'); From = $from; To = $value; Undone = $false })
        Save-History
    }
    Update-View
    $lblStatus.Text = "$label set.  $how"
    return $true
}

$btnApply.Add_Click({
    $sel = $radios | Where-Object { $_.Checked } | Select-Object -First 1
    if (-not $sel) { $lblStatus.Text = 'Pick a preset first.'; return }
    [void](Invoke-Apply ([int]$sel.Tag.Value) $sel.Tag.Name)
})

$btnUndo.Add_Click({
    $last = @($script:History | Where-Object { -not $_.Undone }) | Select-Object -Last 1
    if (-not $last) { return }
    if (Invoke-Apply ([int]$last.From) ('Previous value 0x{0:X2}' -f [int]$last.From) $false) {
        $last.Undone = $true
        Save-History
        Update-View
    }
})

$btnClose.Add_Click({ $form.Close() })

$form.Add_Shown({ Load-History; Update-View })
[void]$form.ShowDialog()
