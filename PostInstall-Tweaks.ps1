<#
  PostInstall-Tweaks.ps1
  Re-applies the startup tweaks after a fresh Windows install (services are handled by ServiceManager).

    - Disables telemetry / tips / maps / family-safety scheduled tasks
      (keeps XblGameSaveTask and everything Xbox / Steam related)
    - Stops Edge preloading in the background (Startup Boost + background mode) - Edge stays installed
    - Optional: -RemoveOneDrive uninstalls OneDrive (removes its Run entry, tasks and Explorer overlays)
    - Xbox online play: turns Teredo on as "enterpriseclient" (Windows ships it disabled, and plain
      "client" mode switches itself off because the router's "lan" DNS suffix looks like a managed network).
      Set both with netsh and as a Group Policy so Windows keeps re-applying it.

  Before the first Apply changes anything, the state of every task, the Edge / Teredo policy values and
  the Teredo type are saved to -StatePath (default %LOCALAPPDATA%\PCTuner\tweaks-state.json; a second
  Apply keeps the first record). -Undo puts back exactly that state and then deletes the file. Tasks the
  Startup Cleaner switched off (startup-history.json in the same folder) are left off.
  When the first record is taken on a PC that already has the tweaks (applied by an older PC Tuner that
  saved no state), or the saved file cannot be read (it is moved aside as tweaks-state.corrupt-*.json),
  the record is marked Inexact and -Undo resets to the Windows defaults instead, as older versions did.

  Run from an ELEVATED PowerShell:
      powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1
      powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1 -RemoveOneDrive
      powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1 -Undo
      powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1 -Only Edge        (one tweak: Tasks, Edge, Teredo or OneDrive)

  -Only applies just the named tweaks (the state before is still saved first). -Undo always undoes all of them.
#>
[CmdletBinding()]
param(
    [switch]$RemoveOneDrive,
    [switch]$Undo,
    [ValidateSet('Tasks', 'Edge', 'Teredo', 'OneDrive')]
    [string[]]$Only,
    [string]$StatePath = (Join-Path $env:LOCALAPPDATA 'PCTuner\tweaks-state.json')
)

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole('Administrators')) {
    throw 'Run this script from an elevated (Administrator) PowerShell.'
}

$Tasks = @(
    '\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser'
    '\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser Exp'
    '\Microsoft\Windows\Application Experience\MareBackup'
    '\Microsoft\Windows\Application Experience\PcaPatchDbTask'
    '\Microsoft\Windows\Application Experience\ProgramDataUpdater'
    '\Microsoft\Windows\Application Experience\StartupAppTask'
    '\Microsoft\Windows\Customer Experience Improvement Program\Consolidator'
    '\Microsoft\Windows\Customer Experience Improvement Program\UsbCeip'
    '\Microsoft\Windows\Feedback\Siuf\DmClient'
    '\Microsoft\Windows\Feedback\Siuf\DmClientOnScenarioDownload'
    '\Microsoft\Windows\DiskDiagnostic\Microsoft-Windows-DiskDiagnosticDataCollector'
    '\Microsoft\Windows\Autochk\Proxy'
    '\Microsoft\Windows\Windows Error Reporting\QueueReporting'
    '\Microsoft\Windows\Maps\MapsToastTask'
    '\Microsoft\Windows\Maps\MapsUpdateTask'
    '\Microsoft\Windows\Shell\FamilySafetyMonitor'
    '\Microsoft\Windows\Shell\FamilySafetyRefreshTask'
    '\Microsoft\Windows\Device Information\Device'
    '\Microsoft\Windows\Device Information\Device User'
    '\Microsoft\Windows\Power Efficiency Diagnostics\AnalyzeSystem'
    '\Microsoft\Windows\Sustainability\PowerGridForecastTask'
    '\Microsoft\Windows\Sustainability\SustainabilityTelemetry'
    '\Microsoft\Windows\DiskFootprint\Diagnostics'
    '\Microsoft\Windows\Diagnosis\RecommendedTroubleshootingScanner'
    '\Microsoft\Windows\Diagnosis\UnexpectedCodepath'
    '\Microsoft\Windows\AccountHealth\RecoverabilityToastTask'
    '\Microsoft\Windows\WwanSvc\OobeDiscovery'
    '\Microsoft\Windows\WwanSvc\NotificationTask'
    '\Microsoft\Windows\RetailDemo\CleanupOfflineContent'
    '\Microsoft\Windows\Printing\PrinterCleanupTask'
    '\Microsoft\Windows\Printing\PrintJobCleanupTask'
    '\Microsoft\Windows\Maintenance\WinSAT'
)

$edgeKey = 'HKLM:\SOFTWARE\Policies\Microsoft\Edge'
$v6Key = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\TCPIP\v6Transition'
$TeredoServer = 'win1910.ipv6.microsoft.com'
$PolicyValues = @(
    @($edgeKey, 'StartupBoostEnabled'), @($edgeKey, 'BackgroundModeEnabled'),
    @($v6Key, 'Teredo_State'), @($v6Key, 'Teredo_ServerName')
)

# Which tweaks this run touches: all of them, or only those named in -Only (Undo always does all).
function Test-Do([string]$Name) { $Undo -or -not $Only -or ($Only -contains $Name) }
if ($Only -contains 'OneDrive') { $RemoveOneDrive = $true }

$script:Failed = 0
function Write-Fail([string]$What, [string]$Why) {
    $script:Failed++
    Write-Host "  FAIL      $What : $($Why.Trim())" -ForegroundColor Red
}
function Read-Value([string]$Key, [string]$Name) {
    $k = Get-Item -Path $Key -ErrorAction SilentlyContinue
    if ($k -and ($k.GetValueNames() -contains $Name)) {
        [pscustomobject]@{ Key = $Key; Name = $Name; Present = $true; Kind = $k.GetValueKind($Name).ToString(); Data = $k.GetValue($Name, $null, 'DoNotExpandEnvironmentNames') }
    } else {
        [pscustomobject]@{ Key = $Key; Name = $Name; Present = $false; Kind = $null; Data = $null }
    }
}
# Put a saved policy value back, or remove it when it was not there before.
function Restore-Value($V) {
    if ($V.Present) {
        if (-not (Test-Path -Path $V.Key)) { New-Item -Path $V.Key -Force -ErrorAction Stop | Out-Null }
        $data = $V.Data
        if ($V.Kind -eq 'Binary') { $data = [byte[]]@($data) } elseif ($V.Kind -eq 'MultiString') { $data = [string[]]@($data) }
        Set-ItemProperty -Path $V.Key -Name $V.Name -Value $data -Type $V.Kind -ErrorAction Stop
    } elseif (Test-Path -Path $V.Key) {
        Remove-ItemProperty -Path $V.Key -Name $V.Name -ErrorAction SilentlyContinue
    }
}
# Remove a policy key the tweaks created, if nothing else has been put in it since.
function Remove-NewKey([string]$Key) {
    $rec = @($State.PolicyKeys) | Where-Object { $_.Key -eq $Key } | Select-Object -First 1
    if (-not $rec -or $rec.Existed) { return }
    $k = Get-Item -Path $Key -ErrorAction SilentlyContinue
    if ($k -and $k.ValueCount -eq 0 -and $k.SubKeyCount -eq 0) { Remove-Item -Path $Key -ErrorAction SilentlyContinue }
}
function Read-Teredo {
    $s = (& netsh interface teredo show state) -join "`n"
    [pscustomobject]@{
        Type   = if ($s -match '(?m)^\s*Type\s*:\s*(.+?)\s*$') { $Matches[1] } else { '' }
        Server = if ($s -match '(?m)^\s*Server Name\s*:\s*(.+?)\s*$') { $Matches[1] } else { '' }
    }
}

# Tasks the Startup Cleaner switched off (its history is next to the state file) are never switched back on.
$KeepOff = @{}
$histPath = Join-Path (Split-Path $StatePath) 'startup-history.json'
if (Test-Path $histPath) {
    try {
        $hist = Get-Content -Path $histPath -Raw -ErrorAction Stop | ConvertFrom-Json
        foreach ($h in $hist) {
            if ($h.Kind -ne 'Task' -or -not $h.Loc -or -not $h.Loc.TaskName) { continue }
            $full = ([string]$h.Loc.TaskPath).TrimEnd('\') + '\' + $h.Loc.TaskName
            $on = if ($h.Undone) { [bool]$h.From } else { [bool]$h.To }
            $KeepOff[$full] = -not $on
        }
    } catch { }
}

# The state saved by the first Apply.
$State = $null
$StateBad = $false
$StateNote = $null
$HadStateFile = Test-Path $StatePath
if ($HadStateFile) {
    try {
        $State = Get-Content -Path $StatePath -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
        if (-not ($State -is [System.Management.Automation.PSCustomObject]) -or $null -eq $State.PSObject.Properties['Tasks']) { throw 'not a saved state' }
    } catch {
        # Never replace it quietly with the current (tweaked) state: move it aside and say so.
        $State = $null
        $StateBad = $true
        $bad = 'tweaks-state.corrupt-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.json'
        try { Rename-Item -Path $StatePath -NewName $bad -ErrorAction Stop; $where = "moved to $bad" } catch { $where = 'left in place' }
        $StateNote = "The saved state from Apply tweaks could not be read ($where) - exact undo is unavailable."
    }
}
# A record taken after the tweaks were already there (older PC Tuner) cannot be put back exactly.
if ($Undo -and $State -and $State.Inexact) {
    $StateNote = 'The saved state was taken after an older PC Tuner had already applied the tweaks - exact undo is unavailable.'
    $State = $null
}
$Saved = @{}
if ($State) { foreach ($r in @($State.Tasks)) { if ($r.Path) { $Saved[[string]$r.Path] = [string]$r.State } } }

# Every task the tweaks touch: the list above plus the Windows "tips" tasks (per-user folder names vary).
$Entries = New-Object System.Collections.ArrayList
foreach ($p in $Tasks) {
    $t = Get-ScheduledTask -TaskPath ((Split-Path $p) + '\') -TaskName (Split-Path $p -Leaf) -ErrorAction SilentlyContinue | Select-Object -First 1
    [void]$Entries.Add([pscustomobject]@{ Path = $p; Task = $t })
}
foreach ($t in @(Get-ScheduledTask -TaskPath '\SoftLanding\*' -ErrorAction SilentlyContinue)) {
    [void]$Entries.Add([pscustomobject]@{ Path = $t.TaskPath + $t.TaskName; Task = $t })
}

if (-not $Undo) {
    # Save the state before anything changes. A second Apply only adds tasks it has not seen before.
    $recs = New-Object System.Collections.ArrayList
    if ($State) { foreach ($r in @($State.Tasks)) { if ($r.Path) { [void]$recs.Add($r) } } }
    $changed = -not $State
    $inexact = if ($State) { [bool]$State.Inexact } else { $StateBad }
    $vals = if ($State) { @($State.Values) } else { @($PolicyValues | ForEach-Object { Read-Value $_[0] $_[1] }) }
    if (-not $State -and -not $inexact) {
        # The values an older PC Tuner set: the PC is already tweaked, so this is not the state before
        $is = { param($n, $kind, $data) @($vals | Where-Object { $_.Name -eq $n -and $_.Present -and $_.Kind -eq $kind -and "$($_.Data)" -eq "$data" }).Count -gt 0 }
        $edgeOld = (& $is 'StartupBoostEnabled' 'DWord' 0) -and (& $is 'BackgroundModeEnabled' 'DWord' 0)
        $teredoOld = (& $is 'Teredo_State' 'String' 'Enterprise Client') -and (& $is 'Teredo_ServerName' 'String' $TeredoServer)
        $inexact = $edgeOld -or $teredoOld
        if ($inexact) { $StateNote = 'The tweaks are already on this PC from an older PC Tuner - "Undo tweaks" will reset to the Windows defaults.' }
    }
    foreach ($e in $Entries) {
        if ($e.Task -and -not $Saved.ContainsKey($e.Path)) {
            $Saved[$e.Path] = "$($e.Task.State)"
            [void]$recs.Add([pscustomobject]@{ Path = $e.Path; State = "$($e.Task.State)" })
            $changed = $true
        }
    }
    if ($changed) {
        $obj = [ordered]@{
            Version    = 1
            Saved      = if ($State) { $State.Saved } else { (Get-Date).ToString('yyyy-MM-dd HH:mm:ss') }
            Inexact    = $inexact
            Tasks      = @($recs)
            Values     = $vals
            PolicyKeys = if ($State) { @($State.PolicyKeys) } else { @(foreach ($k in $edgeKey, $v6Key) { [pscustomobject]@{ Key = $k; Existed = [bool](Test-Path -Path $k) } }) }
            Teredo     = if ($State) { $State.Teredo } else { Read-Teredo }
        }
        try {
            New-Item -ItemType Directory -Force -Path (Split-Path $StatePath) -ErrorAction Stop | Out-Null
            ConvertTo-Json -InputObject $obj -Depth 6 | Set-Content -Path $StatePath -Encoding UTF8 -ErrorAction Stop
        } catch {
            throw "Could not save the state before the tweaks to $StatePath, nothing was changed: $($_.Exception.Message)"
        }
    }
}

if (Test-Do 'Tasks') {
Write-Host "Scheduled tasks:" -ForegroundColor Cyan
if ($StateNote -and $Undo) {
    Write-Host "  NOTE      $StateNote Every task is switched back on and the policies are reset to the Windows defaults." -ForegroundColor Yellow
} elseif ($StateNote) {
    Write-Host "  NOTE      $StateNote" -ForegroundColor Yellow
} elseif ($Undo -and -not $State) {
    Write-Host '  NOTE      No saved state from Apply tweaks - every task is switched back on and the policies are reset to the Windows defaults.' -ForegroundColor Yellow
}
foreach ($e in $Entries) {
    $p = $e.Path
    $t = $e.Task
    if (-not $t) { Write-Host "  skip      $p (not present)" -ForegroundColor DarkGray; continue }
    $off = "$($t.State)" -eq 'Disabled'
    if ($Undo) {
        if ($KeepOff[$p]) { Write-Host "  kept      $p (switched off by the Startup Cleaner - left off)" -ForegroundColor DarkGray; continue }
        if ($State -and -not $Saved.ContainsKey($p)) { Write-Host "  kept      $p (not changed by the tweaks - left as it is)" -ForegroundColor DarkGray; continue }
        if ($State -and $Saved[$p] -eq 'Disabled') { Write-Host "  kept      $p (was off before the tweaks - left off)" -ForegroundColor DarkGray; continue }
        if (-not $off) { Write-Host "  kept      $p (already on - nothing to do)" -ForegroundColor DarkGray; continue }
        try { Enable-ScheduledTask -InputObject $t -ErrorAction Stop | Out-Null; Write-Host "  enabled   $p" -ForegroundColor Green }
        catch { Write-Fail $p $_.Exception.Message }
    } else {
        if ($off) { Write-Host "  kept      $p (already off - nothing to do)" -ForegroundColor DarkGray; continue }
        try { Disable-ScheduledTask -InputObject $t -ErrorAction Stop | Out-Null; Write-Host "  disabled  $p" -ForegroundColor Green }
        catch { Write-Fail $p $_.Exception.Message }
    }
}
}

if (Test-Do 'Edge') {
Write-Host "`nEdge background launching:" -ForegroundColor Cyan
$before = $script:Failed
if ($Undo -and $State) {
    try {
        foreach ($v in @($State.Values) | Where-Object { $_.Key -eq $edgeKey }) { Restore-Value $v }
        Remove-NewKey $edgeKey
    } catch { Write-Fail 'Edge policies' $_.Exception.Message }
    if ($script:Failed -eq $before) { Write-Host '  Edge policies put back as they were before the tweaks' -ForegroundColor Green }
} elseif ($Undo) {
    foreach ($v in 'StartupBoostEnabled', 'BackgroundModeEnabled') { Remove-ItemProperty -Path $edgeKey -Name $v -ErrorAction SilentlyContinue }
    Write-Host '  Edge policies removed (Edge back to defaults)' -ForegroundColor Green
} else {
    try {
        # New-Item -Force on an existing key would wipe the other Edge policies in it
        if (-not (Test-Path -Path $edgeKey)) { New-Item -Path $edgeKey -Force -ErrorAction Stop | Out-Null }
        Set-ItemProperty -Path $edgeKey -Name StartupBoostEnabled  -Value 0 -Type DWord -ErrorAction Stop
        Set-ItemProperty -Path $edgeKey -Name BackgroundModeEnabled -Value 0 -Type DWord -ErrorAction Stop
        Write-Host '  Startup Boost and background mode turned off (Edge shows "Managed by your organization" - expected)' -ForegroundColor Green
    } catch { Write-Fail 'Edge policies' $_.Exception.Message }
}
}

if (Test-Do 'Teredo') {
Write-Host "`nXbox networking (Teredo):" -ForegroundColor Cyan
$before = $script:Failed
if ($Undo -and $State) {
    try {
        foreach ($v in @($State.Values) | Where-Object { $_.Key -eq $v6Key }) { Restore-Value $v }
        Remove-NewKey $v6Key
    } catch { Write-Fail 'Teredo policy' $_.Exception.Message }
    $hadPolicy = @(@($State.Values) | Where-Object { $_.Key -eq $v6Key -and $_.Present }).Count -gt 0
    $type = ''
    # The netsh setting only matters when no Teredo policy was there before (a policy overrides it)
    if (-not $hadPolicy -and $State.Teredo -and $State.Teredo.Type -and $State.Teredo.Type -notmatch 'Group Policy') {
        $type = ($State.Teredo.Type -replace '\s*\(.*$', '').Trim()
        if ($type -notin 'disabled', 'client', 'enterpriseclient', 'natawareclient', 'server', 'default') { $type = 'default' }
        $server = ([string]$State.Teredo.Server -replace '\s*\(.*$', '').Trim()
        if (-not $server -or [string]$State.Teredo.Server -match 'Default|Group Policy') { $server = 'default' }
        $out = & netsh interface teredo set state type=$type servername=$server 2>&1
        if ($LASTEXITCODE -ne 0) { Write-Fail 'Teredo (netsh)' "netsh exit code $($LASTEXITCODE) - $(($out | Out-String).Trim())" }
    }
    & gpupdate /target:computer /force | Out-Null
    if ($script:Failed -eq $before) {
        $what = if ($type) { "type $type" } elseif ($hadPolicy) { 'Group Policy' } else { 'no policy' }
        Write-Host "  Teredo put back as it was before the tweaks ($what)" -ForegroundColor Green
    }
} elseif ($Undo) {
    foreach ($v in 'Teredo_State', 'Teredo_ServerName') { Remove-ItemProperty -Path $v6Key -Name $v -ErrorAction SilentlyContinue }
    $out = & netsh interface teredo set state type=default 2>&1
    if ($LASTEXITCODE -ne 0) { Write-Fail 'Teredo (netsh)' "netsh exit code $($LASTEXITCODE) - $(($out | Out-String).Trim())" }
    & gpupdate /target:computer /force | Out-Null
    if ($script:Failed -eq $before) { Write-Host '  Teredo policy removed and Teredo set back to the Windows default' -ForegroundColor Green }
} else {
    $out = & netsh interface teredo set state type=enterpriseclient servername=$TeredoServer 2>&1
    if ($LASTEXITCODE -ne 0) { Write-Fail 'Teredo (netsh)' "netsh exit code $($LASTEXITCODE) - $(($out | Out-String).Trim())" }
    # Same values Group Policy writes for: Network > TCPIP Settings > IPv6 Transition Technologies
    try {
        if (-not (Test-Path -Path $v6Key)) { New-Item -Path $v6Key -Force -ErrorAction Stop | Out-Null }
        Set-ItemProperty -Path $v6Key -Name Teredo_State -Value 'Enterprise Client' -Type String -ErrorAction Stop
        Set-ItemProperty -Path $v6Key -Name Teredo_ServerName -Value $TeredoServer -Type String -ErrorAction Stop
    } catch { Write-Fail 'Teredo policy' $_.Exception.Message }
    & gpupdate /target:computer /force | Out-Null
    Start-Sleep -Seconds 5
    & ping -6 -n 2 2001:4860:4860::8888 | Out-Null    # wakes Teredo so the state below is current (timeouts are expected)
    $state = (& netsh interface teredo show state) -join "`n"
    $s = if ($state -match 'State\s*:\s*(\S+)') { $Matches[1] } else { '?' }
    $u = if ($state -match 'UPNP:\s*(\w+)') { $Matches[1] } else { '?' }
    $color = if ($s -in 'qualified', 'dormant') { 'Green' } else { 'Yellow' }
    Write-Host "  Teredo: enterpriseclient via $TeredoServer -> state '$s', router UPnP '$u'" -ForegroundColor $color
    if ($s -notin 'qualified', 'dormant') { Write-Host '  Not qualified yet - check again in a minute: netsh interface teredo show state' -ForegroundColor Yellow }
    if ($u -eq 'No') { Write-Host '  Tip: enable UPnP in your router for Open NAT.' -ForegroundColor Yellow }
}
}

if ($Undo -and $HadStateFile -and -not $StateBad) {
    if ($script:Failed -eq 0) { Remove-Item -Path $StatePath -ErrorAction SilentlyContinue }
    else { Write-Host '  Some changes could not be undone - the saved state is kept, so "Undo tweaks" can try again.' -ForegroundColor Yellow }
}

if ($RemoveOneDrive -and -not $Undo) {
    Write-Host "`nOneDrive:" -ForegroundColor Cyan
    Get-Process OneDrive -ErrorAction SilentlyContinue | Stop-Process -Force
    $code = $null
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if ($winget) {
        & winget uninstall --id Microsoft.OneDrive --silent --accept-source-agreements
        $code = $LASTEXITCODE
    } else {
        $exe = @("$env:LOCALAPPDATA\Microsoft\OneDrive\OneDrive.exe", "$env:ProgramFiles\Microsoft OneDrive\OneDrive.exe", "${env:ProgramFiles(x86)}\Microsoft OneDrive\OneDrive.exe") | Where-Object { Test-Path $_ }
        $setup = @("$env:SystemRoot\System32\OneDriveSetup.exe", "$env:SystemRoot\SysWOW64\OneDriveSetup.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
        if (-not $exe) { $code = -1978335212 }
        elseif ($setup) { $code = (Start-Process $setup -ArgumentList '/uninstall' -Wait -PassThru).ExitCode }
        else { Write-Host '  OneDrive installer not found - uninstall it from Settings > Apps.' -ForegroundColor Yellow }
    }
    if ($code -eq 0) { Write-Host '  OneDrive removed (reinstall any time from the Microsoft Store or winget).' -ForegroundColor Green }
    elseif ($code -eq -1978335212) { Write-Host '  OneDrive not installed - nothing to do.' -ForegroundColor Green }   # 0x8A150014 NO_APPLICATIONS_FOUND
    elseif ($null -ne $code) { Write-Fail 'OneDrive' "the uninstaller stopped with exit code $code" }
    # The scheduled tasks are per-user and survive the uninstall on some builds
    foreach ($t in @(Get-ScheduledTask -TaskName 'OneDrive*' -ErrorAction SilentlyContinue)) {
        if ("$($t.State)" -eq 'Disabled') { continue }
        try { Disable-ScheduledTask -InputObject $t -ErrorAction Stop | Out-Null }
        catch { Write-Fail "OneDrive task $($t.TaskName)" $_.Exception.Message }
    }
}

Write-Host "`nDone. Widgets: Windows blocks scripts from switching it off - use gpedit.msc >" -ForegroundColor Cyan
Write-Host "  Computer Configuration > Administrative Templates > Windows Components > Widgets > 'Allow widgets' = Disabled, then sign out." -ForegroundColor Cyan
