#Requires -Version 5.1
<#
  Startup Cleaner - a small GUI to switch off startup junk, with full undo.

  Covers what Autoruns shows on its main tabs:
    - Logon:  Run keys (user / machine / 32-bit) and Startup folders
    - Scheduled tasks: all third-party tasks + known Windows telemetry/tips tasks
                       (tick "Show all Windows tasks" to see the rest)
    - Store app startup tasks (Teams, Office Hub, Xbox app, ...)
    - Explorer extensions: third-party icon overlays and right-click menu handlers
    - Windows settings: policies that stop background features (Widgets, suggested apps,
      Bing in Start, telemetry level, activity history, Spotlight, ...)

  Nothing is ever deleted. Items are switched off the same way Task Manager / Autoruns do it,
  so every change can be undone. History: %LOCALAPPDATA%\StartupCleaner\history.json

  Launch with StartupCleaner.cmd (double-click), or:
      powershell -ExecutionPolicy Bypass -File "<folder>\StartupCleaner.ps1"
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
    [System.Windows.Forms.MessageBox]::Show("Unexpected error:`n`n$($_.Exception.Message)`n`n$($_.InvocationInfo.PositionMessage)", 'Startup Cleaner', 'OK', 'Error') | Out-Null
    continue
}

# ---------------------------------------------------------------- data
$DataDir  = Join-Path $env:LOCALAPPDATA 'StartupCleaner'
$HistFile = Join-Path $DataDir 'history.json'
if (-not (Test-Path $DataDir)) { New-Item -ItemType Directory -Path $DataDir | Out-Null }

$ApprovedBase = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved'
$StoreBase    = 'HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData'
$BlockedKey   = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Shell Extensions\Blocked'

# Windows scheduled tasks that are telemetry / tips / unused features - safe to switch off.
$JunkTasks = @(
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
    # added after the full autoruns audit
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
# Windows tasks that are safe to switch off but cost a small convenience.
$OptionalTasks = @{
    '\Microsoft\Windows\WlanSvc\CDSSync'                    = 'Optional - Wi-Fi sync (you use Ethernet; switch back on if you use Wi-Fi)'
    '\Microsoft\Windows\AppListBackup\Backup'               = 'Optional - Windows Backup app list to your Microsoft account'
    '\Microsoft\Windows\AppListBackup\BackupNonMaintenance' = 'Optional - Windows Backup app list to your Microsoft account'
    '\Microsoft\Windows\CloudRestore\Backup'                = 'Optional - settings backup to your Microsoft account'
    '\Microsoft\Windows\Shell\ThemesSyncedImageDownload'    = 'Optional - downloads synced theme wallpapers'
}
$KeepTasks = @{
    '\Microsoft\XblGameSave\XblGameSaveTask' = 'Keep - syncs Xbox / Game Pass cloud saves'
}

# Windows settings / policies that stop background features. "On" = Windows default.
# Switching one off writes the values below; switching it back on restores what was there before.
$Tweaks = @(
    @{ Name = 'Widgets (news feed board)'; Cat = 'Junk'
       Advice = 'Junk - stops WidgetBoard / WidgetService / Start feed running in the background (sign out to apply)'
       Values = @( @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Dsh'; N = 'AllowNewsAndInterests'; V = 0 }
                   @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Dsh'; N = 'DisableWidgetsOnLockScreen'; V = 1 }
                   @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lock Screen'; N = 'LockScreenWidgetsEnabled'; V = 0 } ) }
    @{ Name = 'Suggested apps, silent app installs and tips'; Cat = 'Junk'
       Advice = 'Junk - stops Windows installing promoted apps and showing tips / Start recommendations'
       Values = @( foreach ($n in 'SilentInstalledAppsEnabled', 'SystemPaneSuggestionsEnabled', 'SoftLandingEnabled', 'OemPreInstalledAppsEnabled',
                                 'PreInstalledAppsEnabled', 'SubscribedContent-338389Enabled', 'SubscribedContent-338388Enabled',
                                 'SubscribedContent-310093Enabled', 'SubscribedContent-338393Enabled', 'SubscribedContent-353694Enabled',
                                 'SubscribedContent-353696Enabled') {
                       @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager'; N = $n; V = 0 } }
                   @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced'; N = 'Start_IrisRecommendations'; V = 0 } ) }
    @{ Name = 'Bing web results in Start search'; Cat = 'Junk'
       Advice = 'Junk - Start search stays local (apps, files, settings); restart Explorer to apply'
       Values = @( @{ P = 'HKCU:\Software\Policies\Microsoft\Windows\Explorer'; N = 'DisableSearchBoxSuggestions'; V = 1 }
                   @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Search'; N = 'BingSearchEnabled'; V = 0 } ) }
    @{ Name = 'Diagnostic data: lock to Required (minimum)'; Cat = 'Junk'
       Advice = 'Junk - stops Windows raising telemetry again after updates; no feedback pop-ups'
       Values = @( @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection'; N = 'AllowTelemetry'; V = 1 }
                   @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection'; N = 'DoNotShowFeedbackNotifications'; V = 1 }
                   @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection'; N = 'LimitDiagnosticLogCollection'; V = 1 } ) }
    @{ Name = 'Activity history collection / upload'; Cat = 'Junk'
       Advice = 'Junk - Timeline is gone in Windows 11; stops activity database writes and uploads'
       Values = @( @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System'; N = 'EnableActivityFeed'; V = 0 }
                   @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System'; N = 'PublishUserActivities'; V = 0 }
                   @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System'; N = 'UploadUserActivities'; V = 0 } ) }
    @{ Name = '"Finish setting up your device" nag'; Cat = 'Junk'
       Advice = 'Junk - stops the setup/upsell screen (UserOOBEBroker)'
       Values = @( @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement'; N = 'ScoobeSystemSettingEnabled'; V = 0 } ) }
    @{ Name = 'Delivery Optimization: share updates with LAN PCs'; Cat = 'Junk'
       Advice = 'Junk - HTTP-only downloads (single PC, nothing to share with); Store / Game Pass downloads unaffected'
       Values = @( @{ P = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\DeliveryOptimization'; N = 'DODownloadMode'; V = 0 } ) }
    @{ Name = 'Cross-device Resume (CrossDeviceResume.exe)'; Cat = 'Optional'
       Advice = 'Optional - resume phone apps on the PC; sign out to apply'
       Values = @( @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration'; N = 'IsResumeAllowed'; V = 0 }
                   @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration'; N = 'IsOneDriveResumeAllowed'; V = 0 } ) }
    @{ Name = 'Windows Spotlight on the lock screen'; Cat = 'Optional'
       Advice = 'Optional - rotating Bing lock-screen pictures and tips; also pick "Picture" in Settings > Lock screen'
       Values = @( @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager'; N = 'RotatingLockScreenEnabled'; V = 0 }
                   @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager'; N = 'RotatingLockScreenOverlayEnabled'; V = 0 }
                   @{ P = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager'; N = 'SubscribedContent-338387Enabled'; V = 0 } ) }
    @{ Name = 'Windows Spotlight on the desktop'; Cat = 'Optional'
       Advice = 'Optional - rotating Bing wallpaper; also pick "Picture" in Settings > Personalization > Background'
       Values = @( @{ P = 'HKCU:\Software\Policies\Microsoft\Windows\CloudContent'; N = 'DisableSpotlightCollectionOnDesktop'; V = 1 } ) }
)

# First matching rule wins. Matched against "name command location".
$Rules = @(
    @{ P = 'MicrosoftEdgeUpdate|EdgeUpdate';                                         C = 'Keep';     A = 'Keep - also keeps WebView2 (used by many apps) patched' }
    @{ P = 'nv3dappshext';                                                           C = 'Junk';     A = 'Junk - NVIDIA "Run with graphics processor" menu; does nothing on a single-GPU PC' }
    @{ P = 'SecurityHealthSystray';                                                  C = 'Optional'; A = 'Optional - Windows Security tray icon only; Defender keeps running and still alerts' }
    @{ P = '\bsteam|xbox|xbl|gamingapp|gamingservices|gameinput|easyanticheat|battleye|vanguard|nvidia|nvcpl|nvcontainer|nvdisplay|\\amd\\|radeon|realtek|rtkaud|logitech|lghub|logi_|razer|corsair|steelseries|securityhealth';
                                                                                     C = 'Keep';     A = 'Keep - gaming / hardware / security' }
    @{ P = 'onedrive|MicrosoftEdgeAutoLaunch|softlanding|MSTeams|TeamsTfwStartupTask|MicrosoftOfficeHub|WebViewHostStartupId|copilot|cortana';
                                                                                     C = 'Junk';     A = 'Junk - safe to switch off' }
    @{ P = 'Windows Defender\\shellext';                                             C = 'Optional'; A = 'Optional - "Scan with Microsoft Defender" right-click item' }
    @{ P = 'update|updater'; NameOnly = $true;                                       C = 'Review';   A = 'Review - updater (most apps also update when you open them)' }
)

$KindNames = @{
    Run           = 'Logon (Run key)'
    StartupFolder = 'Logon (Startup folder)'
    Task          = 'Scheduled task'
    StoreApp      = 'Store app startup'
    ShellExt      = 'Explorer extension'
    Tweak         = 'Windows setting'
}

$script:History  = New-Object System.Collections.ArrayList
$script:Entries  = @{}
$script:PubCache = @{}

# ---------------------------------------------------------------- helpers
function Get-ExePath([string]$cmd) {
    if (-not $cmd -or $cmd.StartsWith('(')) { return $null }
    $c = [Environment]::ExpandEnvironmentVariables($cmd.Trim())
    if ($c.StartsWith('"')) {
        $end = $c.IndexOf('"', 1)
        $p = if ($end -gt 1) { $c.Substring(1, $end - 1) } else { $c.Trim('"') }
    } elseif ($c -match '^(.+?\.(exe|dll|cmd|bat|lnk|ps1|vbs))(\s|,|$)') {
        $p = $Matches[1]
    } else {
        $p = ($c -split '\s+')[0]
    }
    if ($p -and -not [IO.Path]::IsPathRooted($p)) {
        $leaf = if ([IO.Path]::HasExtension($p)) { $p } else { "$p.exe" }
        foreach ($dir in "$env:SystemRoot\System32", $env:SystemRoot) {
            if (Test-Path -LiteralPath (Join-Path $dir $leaf)) { return (Join-Path $dir $leaf) }
        }
        return $null   # bare command name we can't resolve - don't call it "missing"
    }
    return $p
}

function Get-Publisher([string]$cmd) {
    $p = Get-ExePath $cmd
    if (-not $p) { return '' }
    if ($script:PubCache.ContainsKey($p)) { return $script:PubCache[$p] }
    $co = ''
    try {
        if (Test-Path -LiteralPath $p -PathType Leaf) { $co = [string](Get-Item -LiteralPath $p).VersionInfo.CompanyName }
        else { $co = '(file missing)' }
    } catch { }
    $script:PubCache[$p] = $co
    return $co
}

function Get-Advice([string]$kind, [string]$name, [string]$command, [string]$location, [string]$taskFull) {
    if ($kind -eq 'Task') {
        if ($KeepTasks.ContainsKey($taskFull))     { return 'Keep', $KeepTasks[$taskFull] }
        if ($JunkTasks -contains $taskFull)        { return 'Junk', 'Junk - Windows telemetry / tips / unused feature' }
        if ($OptionalTasks.ContainsKey($taskFull)) { return 'Optional', $OptionalTasks[$taskFull] }
        if ($taskFull -like '\Microsoft\*')    { return 'Windows', 'Windows task - leave unless you know it' }
    }
    $hay = "$name $command $location"
    foreach ($r in $Rules) {
        $target = if ($r.NameOnly) { $name } else { $hay }
        if ($target -match $r.P) { return $r.C, $r.A }
    }
    if ($kind -eq 'ShellExt') { return 'Review', 'Review - third-party Explorer add-on; switch off if you never use its menu item / icon' }
    return 'Review', 'Review - starts with Windows; switch off if you do not need it at login'
}

function New-Entry([string]$kind, [string]$name, [string]$location, [string]$command, [bool]$enabled, $loc, $publisher = $null) {
    $cat, $adv = Get-Advice $kind $name $command $location $loc.TaskFull
    if ($null -eq $publisher) { $publisher = Get-Publisher $command }
    if ($publisher -eq '(file missing)' -and $cat -ne 'Keep' -and $kind -in 'Run', 'StartupFolder', 'Task') {
        $cat = 'Junk'; $adv = 'Junk - points to a file that no longer exists'
    }
    [pscustomobject]@{
        Id = "$kind|$location|$name"; Kind = $kind; Name = $name; Location = $location; Command = $command
        Publisher = $publisher; Enabled = $enabled; Category = $cat; Advice = $adv; Loc = $loc
    }
}

function Get-ApprovedEnabled([string]$approvedKey, [string]$valueName) {
    $item = Get-Item -LiteralPath $approvedKey -ErrorAction SilentlyContinue
    if (-not $item) { return $true }
    $v = $item.GetValue($valueName)
    if ($v -is [byte[]] -and $v.Length -gt 0) { return -not ($v[0] -band 1) }
    return $true
}

function Resolve-Clsid([string]$clsid) {
    foreach ($root in 'Registry::HKEY_CURRENT_USER\Software\Classes\CLSID', 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID', 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Classes\CLSID') {
        $k = Get-Item -LiteralPath "$root\$clsid\InprocServer32" -ErrorAction SilentlyContinue
        if ($k) { return [Environment]::ExpandEnvironmentVariables([string]$k.GetValue('')) }
    }
    return ''
}

# ---------------------------------------------------------------- scanners
function Get-RunEntries {
    $sources = @(
        @{ Key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run';             Approved = "HKCU:\$ApprovedBase\Run";   Loc = 'HKCU Run' }
        @{ Key = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run';             Approved = "HKLM:\$ApprovedBase\Run";   Loc = 'HKLM Run' }
        @{ Key = 'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run'; Approved = "HKLM:\$ApprovedBase\Run32"; Loc = 'HKLM Run (32-bit)' }
    )
    foreach ($s in $sources) {
        $item = Get-Item -LiteralPath $s.Key -ErrorAction SilentlyContinue
        if (-not $item) { continue }
        foreach ($vn in $item.GetValueNames()) {
            if (-not $vn) { continue }
            New-Entry 'Run' $vn $s.Loc ([string]$item.GetValue($vn)) (Get-ApprovedEnabled $s.Approved $vn) @{ Approved = $s.Approved; Value = $vn }
        }
    }
}

function Get-StartupFolderEntries {
    $shell = New-Object -ComObject WScript.Shell
    $sources = @(
        @{ Dir = [Environment]::GetFolderPath('Startup');       Approved = "HKCU:\$ApprovedBase\StartupFolder"; Loc = 'Startup folder (you)' }
        @{ Dir = [Environment]::GetFolderPath('CommonStartup'); Approved = "HKLM:\$ApprovedBase\StartupFolder"; Loc = 'Startup folder (all users)' }
    )
    foreach ($s in $sources) {
        if (-not $s.Dir -or -not (Test-Path -LiteralPath $s.Dir)) { continue }
        foreach ($f in Get-ChildItem -LiteralPath $s.Dir -File -Force -ErrorAction SilentlyContinue) {
            if ($f.Name -ieq 'desktop.ini') { continue }
            $cmd = $f.FullName
            if ($f.Extension -ieq '.lnk') {
                try { $lnk = $shell.CreateShortcut($f.FullName); if ($lnk.TargetPath) { $cmd = "`"$($lnk.TargetPath)`" $($lnk.Arguments)".Trim() } } catch { }
            }
            New-Entry 'StartupFolder' $f.Name $s.Loc $cmd (Get-ApprovedEnabled $s.Approved $f.Name) @{ Approved = $s.Approved; Value = $f.Name }
        }
    }
}

function Get-TaskEntries {
    foreach ($t in Get-ScheduledTask -ErrorAction SilentlyContinue) {
        $full = $t.TaskPath + $t.TaskName
        $a = @($t.Actions)[0]
        $cmd = if ($a -and $a.Execute) { "$($a.Execute) $($a.Arguments)".Trim() } else { '(COM handler)' }
        New-Entry 'Task' $t.TaskName $t.TaskPath $cmd ([string]$t.State -ne 'Disabled') @{ TaskPath = $t.TaskPath; TaskName = $t.TaskName; TaskFull = $full }
    }
}

function Get-StoreEntries {
    foreach ($pkg in Get-AppxPackage -ErrorAction SilentlyContinue) {
        if (-not $pkg.InstallLocation) { continue }
        $m = Join-Path $pkg.InstallLocation 'AppxManifest.xml'
        if (-not (Test-Path -LiteralPath $m)) { continue }
        try { $raw = [IO.File]::ReadAllText($m) } catch { continue }
        if ($raw -notmatch 'StartupTask') { continue }
        try { $x = [xml]$raw } catch { continue }
        $pubNode = $x.SelectSingleNode("//*[local-name()='Properties']/*[local-name()='PublisherDisplayName']")
        $pub = if ($pubNode) { $pubNode.InnerText } else { '' }
        foreach ($st in $x.SelectNodes("//*[local-name()='StartupTask']")) {
            $tid = $st.GetAttribute('TaskId')
            if (-not $tid) { continue }
            $k = "$StoreBase\$($pkg.PackageFamilyName)\$tid"
            $state = $null
            $ki = Get-Item -LiteralPath $k -ErrorAction SilentlyContinue
            if ($ki) { $state = $ki.GetValue('State') }
            $enabled = if ($null -ne $state) { [int]$state -in 2, 4 } else { $st.GetAttribute('Enabled') -eq 'true' }
            New-Entry 'StoreApp' $pkg.Name 'Store app' $tid $enabled @{ Pkg = $pkg.PackageFamilyName; TaskId = $tid } $pub
        }
    }
}

function Get-ShellExtEntries {
    $blockedItem = Get-Item -LiteralPath $BlockedKey -ErrorAction SilentlyContinue
    $blocked = if ($blockedItem) { @($blockedItem.GetValueNames()) } else { @() }
    $sources = @(
        @{ Path = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers'; Loc = 'Icon overlay' }
        @{ Path = 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers'; Loc = 'Icon overlay' }
    )
    $classes = [ordered]@{
        '*' = 'Right-click (files)'; 'AllFilesystemObjects' = 'Right-click (files)'; 'Directory' = 'Right-click (folders)'
        'Directory\Background' = 'Right-click (background)'; 'Folder' = 'Right-click (folders)'; 'Drive' = 'Right-click (drives)'
        'exefile' = 'Right-click (.exe)'; 'lnkfile' = 'Right-click (shortcuts)'; 'DesktopBackground' = 'Right-click (desktop)'
    }
    foreach ($root in 'HKLM:\SOFTWARE\Classes', 'HKCU:\SOFTWARE\Classes') {
        foreach ($c in $classes.Keys) { $sources += @{ Path = "$root\$c\shellex\ContextMenuHandlers"; Loc = $classes[$c] } }
    }
    $seen = @{}
    foreach ($s in $sources) {
        foreach ($k in Get-ChildItem -LiteralPath $s.Path -ErrorAction SilentlyContinue) {
            $clsid = [string]$k.GetValue('')
            if ($clsid -notmatch '^\{') { $clsid = $k.PSChildName }
            if ($clsid -notmatch '^\{[0-9A-Fa-f-]+\}$') { continue }
            $dll = Resolve-Clsid $clsid
            # Skip Windows' own handlers - but vendor DLLs installed into the DriverStore (e.g. NVIDIA) are third-party
            if (-not $dll -or $dll -match '\\Windows\\(?!System32\\DriverStore\\)|^%SystemRoot%|^%windir%') { continue }
            $key = $clsid.ToUpperInvariant()
            if ($seen.ContainsKey($key)) {
                if ($seen[$key].Location -notmatch [regex]::Escape($s.Loc)) { $seen[$key].Location += ", $($s.Loc)" }
                continue
            }
            $e = New-Entry 'ShellExt' $k.PSChildName.Trim() $s.Loc $dll ($blocked -notcontains $clsid) @{ Clsid = $clsid }
            $e.Id = "ShellExt|$key"
            $seen[$key] = $e
        }
    }
    $seen.Values
}

function Get-TweakValue([string]$path, [string]$name) {
    $k = Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
    if ($k) { return $k.GetValue($name) }
    return $null
}

function Get-TweakEntries {
    foreach ($t in $Tweaks) {
        $vals = @($t.Values)
        # "On" (Windows default) unless every value already matches the switched-off value
        $off = @($vals | Where-Object { $cur = Get-TweakValue $_.P $_.N; $null -ne $cur -and "$cur" -eq "$($_.V)" }).Count -eq $vals.Count
        $cmd = ($vals | ForEach-Object { "$(($_.P -replace '^HK(LM|CU):\\', 'HK$1\'))\$($_.N)=$($_.V)" }) -join ' ; '
        [pscustomobject]@{
            Id = "Tweak|$($t.Name)"; Kind = 'Tweak'; Name = $t.Name; Location = 'Registry policy / setting'; Command = $cmd
            Publisher = 'Windows'; Enabled = -not $off; Category = $t.Cat; Advice = $t.Advice
            Loc = @{ Values = $vals; Prev = @() }
        }
    }
}

function Get-TweakSnapshot($values) {
    foreach ($v in @($values)) {
        $d = Get-TweakValue $v.P $v.N
        [pscustomobject]@{ P = $v.P; N = $v.N; Exists = ($null -ne $d); Data = $d }
    }
}

# ---------------------------------------------------------------- state changes
function Set-EntryEnabled([string]$kind, $loc, [bool]$on) {
    switch ($kind) {
        { $_ -in 'Run', 'StartupFolder' } {
            if (-not (Test-Path -LiteralPath $loc.Approved)) { New-Item -Path $loc.Approved -Force | Out-Null }
            # Same 12-byte format Task Manager uses: 02 = enabled, 03 + timestamp = disabled
            $bytes = if ($on) { [byte[]](2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) }
                     else     { [byte[]](@(3, 0, 0, 0) + [BitConverter]::GetBytes([DateTime]::Now.ToFileTime())) }
            Set-ItemProperty -LiteralPath $loc.Approved -Name $loc.Value -Value $bytes -Type Binary -ErrorAction Stop
        }
        'Task' {
            if ($on) { Enable-ScheduledTask  -TaskPath $loc.TaskPath -TaskName $loc.TaskName -ErrorAction Stop | Out-Null }
            else     { Disable-ScheduledTask -TaskPath $loc.TaskPath -TaskName $loc.TaskName -ErrorAction Stop | Out-Null }
        }
        'StoreApp' {
            $k = "$StoreBase\$($loc.Pkg)\$($loc.TaskId)"
            if (-not (Test-Path -LiteralPath $k)) { New-Item -Path $k -Force | Out-Null }
            Set-ItemProperty -LiteralPath $k -Name State -Value $(if ($on) { 2 } else { 1 }) -Type DWord -ErrorAction Stop
        }
        'ShellExt' {
            if ($on) {
                Remove-ItemProperty -LiteralPath $BlockedKey -Name $loc.Clsid -ErrorAction SilentlyContinue
            } else {
                if (-not (Test-Path -LiteralPath $BlockedKey)) { New-Item -Path $BlockedKey -Force | Out-Null }
                New-ItemProperty -LiteralPath $BlockedKey -Name $loc.Clsid -Value '' -PropertyType String -Force -ErrorAction Stop | Out-Null
            }
        }
        'Tweak' {
            foreach ($v in @($loc.Values)) {
                if ($on) {
                    # Restore what was there before we switched it off; otherwise remove our value (= Windows default)
                    $prev = @(@($loc.Prev) | Where-Object { $_ -and $_.P -eq $v.P -and $_.N -eq $v.N })
                    if ($prev.Count -and $prev[0].Exists) {
                        if (-not (Test-Path -LiteralPath $v.P)) { New-Item -Path $v.P -Force | Out-Null }
                        Set-ItemProperty -LiteralPath $v.P -Name $v.N -Value $prev[0].Data -Type DWord -ErrorAction Stop
                    } else {
                        Remove-ItemProperty -LiteralPath $v.P -Name $v.N -ErrorAction SilentlyContinue
                    }
                } else {
                    if (-not (Test-Path -LiteralPath $v.P)) { New-Item -Path $v.P -Force | Out-Null }
                    Set-ItemProperty -LiteralPath $v.P -Name $v.N -Value ([int]$v.V) -Type DWord -ErrorAction Stop
                }
            }
        }
        default { throw "Unknown entry type '$kind'." }
    }
}

function Load-History {
    $script:History.Clear()
    if (Test-Path $HistFile) {
        try {
            $data = ConvertFrom-Json (Get-Content -LiteralPath $HistFile -Raw)
            foreach ($h in @($data)) { if ($h) { [void]$script:History.Add($h) } }
        } catch {
            [System.Windows.Forms.MessageBox]::Show("Could not read history file:`n$HistFile`n`n$($_.Exception.Message)", 'Startup Cleaner', 'OK', 'Warning') | Out-Null
        }
    }
}

function Save-History {
    ConvertTo-Json -InputObject @($script:History.ToArray()) -Depth 5 | Set-Content -LiteralPath $HistFile -Encoding UTF8
}

function Get-NextBatch {
    if ($script:History.Count -eq 0) { return 1 }
    return [int](($script:History | Measure-Object -Property Batch -Maximum).Maximum) + 1
}

function Escape-Like([string]$s) {
    $s = $s -replace "'", "''"
    return (($s.ToCharArray() | ForEach-Object { if ('[', ']', '*', '%' -contains [string]$_) { "[$_]" } else { [string]$_ } }) -join '')
}

# ---------------------------------------------------------------- main window
$form = New-Object System.Windows.Forms.Form
$form.Text = 'Startup Cleaner'
$form.Size = New-Object System.Drawing.Size(1250, 740)
$form.MinimumSize = New-Object System.Drawing.Size(900, 450)
$form.StartPosition = 'CenterScreen'
$form.Font = New-Object System.Drawing.Font('Segoe UI', 9)

$top = New-Object System.Windows.Forms.FlowLayoutPanel
$top.Dock = 'Top'; $top.Height = 38; $top.Padding = New-Object System.Windows.Forms.Padding(6, 6, 6, 0)

$lblSearch = New-Object System.Windows.Forms.Label
$lblSearch.Text = 'Search:'; $lblSearch.AutoSize = $true; $lblSearch.Margin = New-Object System.Windows.Forms.Padding(0, 6, 4, 0)
$txtSearch = New-Object System.Windows.Forms.TextBox; $txtSearch.Width = 220

$cmbKind = New-Object System.Windows.Forms.ComboBox
$cmbKind.DropDownStyle = 'DropDownList'; $cmbKind.Width = 170; $cmbKind.Margin = New-Object System.Windows.Forms.Padding(12, 3, 0, 0)
[void]$cmbKind.Items.Add('All types'); foreach ($v in 'Logon (Run key)', 'Logon (Startup folder)', 'Scheduled task', 'Store app startup', 'Explorer extension', 'Windows setting') { [void]$cmbKind.Items.Add($v) }
$cmbKind.SelectedIndex = 0

function New-Check([string]$text, [bool]$checked = $false) {
    $c = New-Object System.Windows.Forms.CheckBox
    $c.Text = $text; $c.AutoSize = $true; $c.Checked = $checked; $c.Margin = New-Object System.Windows.Forms.Padding(14, 4, 0, 0)
    return $c
}
$chkEnabled = New-Check 'Enabled only'
$chkWinTasks = New-Check 'Show all Windows tasks'

$btnClean = New-Object System.Windows.Forms.Button
$btnClean.Text = 'Clean junk (one click)'; $btnClean.Width = 170; $btnClean.Height = 26
$btnClean.Margin = New-Object System.Windows.Forms.Padding(24, 0, 0, 0)
$btnClean.Font = New-Object System.Drawing.Font('Segoe UI', 9, [System.Drawing.FontStyle]::Bold)
$btnCleanOpt = New-Object System.Windows.Forms.Button
$btnCleanOpt.Text = 'Clean junk + optional'; $btnCleanOpt.Width = 170; $btnCleanOpt.Height = 26
$btnCleanOpt.Margin = New-Object System.Windows.Forms.Padding(6, 0, 0, 0)

$top.Controls.AddRange(@($lblSearch, $txtSearch, $cmbKind, $chkEnabled, $chkWinTasks, $btnClean, $btnCleanOpt))

$table = New-Object System.Data.DataTable
foreach ($c in 'Id', 'On', 'Name', 'Type', 'Location', 'Publisher', 'Advice', 'Command', 'Category') { [void]$table.Columns.Add($c) }
$table.DefaultView.Sort = 'Type ASC, Name ASC'

$grid = New-Object System.Windows.Forms.DataGridView
$grid.Dock = 'Fill'; $grid.ReadOnly = $true
$grid.AllowUserToAddRows = $false; $grid.AllowUserToDeleteRows = $false; $grid.AllowUserToResizeRows = $false
$grid.SelectionMode = 'FullRowSelect'; $grid.MultiSelect = $true; $grid.RowHeadersVisible = $false
$grid.AutoSizeColumnsMode = 'Fill'; $grid.BackgroundColor = [System.Drawing.SystemColors]::Window; $grid.BorderStyle = 'None'
$grid.DataSource = $table

$bottom = New-Object System.Windows.Forms.FlowLayoutPanel
$bottom.Dock = 'Bottom'; $bottom.Height = 44; $bottom.Padding = New-Object System.Windows.Forms.Padding(6, 6, 6, 0)
function New-Button([string]$text, [int]$width = 110) {
    $b = New-Object System.Windows.Forms.Button
    $b.Text = $text; $b.Width = $width; $b.Height = 30
    return $b
}
$btnOff      = New-Button 'Switch off'
$btnOn       = New-Button 'Switch on'
$btnUndo     = New-Button 'Undo last change' 130
$btnHistory  = New-Button 'History...'
$btnRefresh  = New-Button 'Rescan' 90
$btnExplorer = New-Button 'Restart Explorer' 130
$btnOff.Margin = New-Object System.Windows.Forms.Padding(3, 3, 3, 3)
$btnUndo.Margin = New-Object System.Windows.Forms.Padding(24, 3, 3, 3)
$bottom.Controls.AddRange(@($btnOff, $btnOn, $btnUndo, $btnHistory, $btnRefresh, $btnExplorer))

$status = New-Object System.Windows.Forms.StatusStrip
$statusLabel = New-Object System.Windows.Forms.ToolStripStatusLabel
$statusLabel.Spring = $true; $statusLabel.TextAlign = 'MiddleLeft'
$legend = New-Object System.Windows.Forms.ToolStripStatusLabel
$legend.Text = 'Orange = junk   Blue = optional   Yellow = keep (gaming / hardware)   Grey = switched off'
$legend.ForeColor = [System.Drawing.Color]::DimGray
[void]$status.Items.Add($statusLabel); [void]$status.Items.Add($legend)

$form.Controls.Add($grid); $form.Controls.Add($top); $form.Controls.Add($bottom); $form.Controls.Add($status)

function Set-Status([string]$text) { $statusLabel.Text = $text }

# ---------------------------------------------------------------- grid logic
function Apply-Filter {
    $parts = @()
    $q = $txtSearch.Text.Trim()
    if ($q) { $e = Escape-Like $q; $parts += "(Name LIKE '*$e*' OR Command LIKE '*$e*' OR Publisher LIKE '*$e*' OR Location LIKE '*$e*')" }
    if ($cmbKind.SelectedIndex -gt 0) { $parts += "Type = '$(([string]$cmbKind.SelectedItem) -replace "'", "''")'" }
    if ($chkEnabled.Checked)       { $parts += "On = 'Yes'" }
    if (-not $chkWinTasks.Checked) { $parts += "Category <> 'Windows'" }
    $table.DefaultView.RowFilter = ($parts -join ' AND ')
}

function Get-SelectedEntries {
    return @($grid.SelectedRows | ForEach-Object { $script:Entries[[string]$_.Cells['Id'].Value] } | Where-Object { $_ })
}

function Update-StatusCounts {
    $all = @($script:Entries.Values | Where-Object { $_.Category -ne 'Windows' })
    $on = @($all | Where-Object { $_.Enabled }).Count
    $junk = @($all | Where-Object { $_.Enabled -and $_.Category -eq 'Junk' }).Count
    $pending = @($script:History | Where-Object { -not $_.Undone }).Count
    Set-Status ("{0} items  |  {1} switched on  |  {2} junk still on  |  {3} change(s) can be undone" -f $all.Count, $on, $junk, $pending)
}

function Load-Entries {
    $form.Cursor = 'WaitCursor'
    Set-Status 'Scanning startup locations...'
    [System.Windows.Forms.Application]::DoEvents()

    $selected = @($grid.SelectedRows | ForEach-Object { [string]$_.Cells['Id'].Value })
    $scroll = $grid.FirstDisplayedScrollingRowIndex

    $script:Entries = @{}
    $list = @()
    $list += @(Get-RunEntries)
    $list += @(Get-StartupFolderEntries)
    $list += @(Get-StoreEntries)
    $list += @(Get-ShellExtEntries)
    $list += @(Get-TweakEntries)
    $list += @(Get-TaskEntries)

    $table.BeginLoadData()
    $table.Rows.Clear()
    foreach ($e in $list) {
        if (-not $e -or $script:Entries.ContainsKey($e.Id)) { continue }
        $script:Entries[$e.Id] = $e
        [void]$table.Rows.Add($e.Id, $(if ($e.Enabled) { 'Yes' } else { 'No' }), $e.Name, $KindNames[$e.Kind], $e.Location, $e.Publisher, $e.Advice, $e.Command, $e.Category)
    }
    $table.EndLoadData()

    $grid.ClearSelection()
    if ($selected.Count) { foreach ($r in $grid.Rows) { if ($selected -contains [string]$r.Cells['Id'].Value) { $r.Selected = $true } } }
    if ($scroll -ge 0 -and $scroll -lt $grid.Rows.Count) { $grid.FirstDisplayedScrollingRowIndex = $scroll }
    $form.Cursor = 'Default'
    Update-StatusCounts
}

$grid.Add_DataBindingComplete({
    if ($grid.Columns.Contains('Id')) {
        $grid.Columns['Id'].Visible = $false
        $grid.Columns['Category'].Visible = $false
        $grid.Columns['On'].FillWeight = 28
        $grid.Columns['Name'].FillWeight = 140
        $grid.Columns['Type'].FillWeight = 90
        $grid.Columns['Location'].FillWeight = 110
        $grid.Columns['Publisher'].FillWeight = 90
        $grid.Columns['Advice'].FillWeight = 170
        $grid.Columns['Command'].FillWeight = 180
    }
})

$colorJunk = [System.Drawing.Color]::FromArgb(255, 224, 189)
$colorKeep = [System.Drawing.Color]::FromArgb(255, 248, 205)
$colorOpt  = [System.Drawing.Color]::FromArgb(222, 235, 255)
$grid.Add_CellFormatting({
    param($src, $e)
    if ($e.RowIndex -lt 0) { return }
    $row = $grid.Rows[$e.RowIndex]
    $cat = [string]$row.Cells['Category'].Value
    if ($cat -eq 'Junk')     { $e.CellStyle.BackColor = $colorJunk }
    elseif ($cat -eq 'Keep') { $e.CellStyle.BackColor = $colorKeep }
    elseif ($cat -eq 'Optional') { $e.CellStyle.BackColor = $colorOpt }
    if ([string]$row.Cells['On'].Value -eq 'No') { $e.CellStyle.ForeColor = [System.Drawing.Color]::Gray }
})

# ---------------------------------------------------------------- actions
# Switches a set of entries on/off as ONE history batch.
function Invoke-Toggle($entries, [bool]$on, [string]$verb) {
    $work = @($entries | Where-Object { $_ -and $_.Enabled -ne $on })
    $skipped = @($entries).Count - $work.Count
    if (-not $work.Count) { Set-Status "Nothing to change - already switched $(if ($on) { 'on' } else { 'off' })."; return 0 }

    $form.Cursor = 'WaitCursor'
    $batch = Get-NextBatch
    $ok = 0; $errors = @(); $needExplorer = $false
    foreach ($e in $work) {
        try {
            if ($e.Kind -eq 'Tweak' -and -not $on) { $e.Loc.Prev = @(Get-TweakSnapshot $e.Loc.Values) }   # so undo restores exactly
            Set-EntryEnabled $e.Kind $e.Loc $on
            [void]$script:History.Add([pscustomobject]@{
                Batch = $batch; Time = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
                Kind = $e.Kind; Name = $e.Name; Location = $e.Location; Loc = $e.Loc
                From = $e.Enabled; To = $on; Undone = $false
            })
            if ($e.Kind -eq 'ShellExt') { $needExplorer = $true }
            $ok++
        } catch {
            $why = $_.Exception.Message
            if ($why -match 'Access is denied' -and $e.Kind -eq 'Task' -and $e.Loc.TaskPath -like '\Microsoft\*') {
                $why = 'protected by Windows, so it cannot be changed (this is normal and harmless)'
            }
            if ($e.Kind -eq 'Tweak') {
                # Don't leave a half-applied setting behind: put back whatever was written before the failure
                if (-not $on) { try { Set-EntryEnabled 'Tweak' $e.Loc $true } catch { } }
                if ($why -match 'unauthorized|denied') {
                    $why = "Windows' UCPD protection blocks this value from scripts. Set it with Group Policy instead (gpedit.msc)."
                    if ($e.Name -like 'Widgets*') { $why += "`n    Computer Configuration > Administrative Templates > Windows Components > Widgets > 'Allow widgets' = Disabled, then sign out." }
                }
            }
            $errors += "$($e.Name): $why"
        }
    }
    Save-History
    Load-Entries
    $form.Cursor = 'Default'

    $summary = "$ok $verb"
    if ($skipped) { $summary += ", $skipped already in that state" }
    if ($errors.Count) { $summary += ", $($errors.Count) failed" }
    Set-Status ($summary + '.  ' + $statusLabel.Text)
    if ($errors.Count) {
        [System.Windows.Forms.MessageBox]::Show("Failed:`n$($errors -join "`n")", 'Startup Cleaner', 'OK', 'Warning') | Out-Null
    }
    if ($needExplorer) { Ask-RestartExplorer }
    return $ok
}

function Invoke-Switch([bool]$on) {
    $sel = Get-SelectedEntries
    if (-not $sel.Count) { Set-Status 'Select one or more items first.'; return }
    if (-not $on) {
        $keep = @($sel | Where-Object { $_.Category -in 'Keep', 'Windows' -and $_.Enabled })
        if ($keep.Count) {
            $msg = "These are marked KEEP (gaming / hardware / security / Windows):`n`n$(($keep | ForEach-Object { $_.Name }) -join "`n")`n`nSwitch them off anyway?"
            if ([System.Windows.Forms.MessageBox]::Show($msg, 'Startup Cleaner', 'YesNo', 'Warning') -ne 'Yes') { return }
        }
    }
    [void](Invoke-Toggle $sel $on $(if ($on) { 'switched on' } else { 'switched off' }))
}

function Invoke-CleanJunk([bool]$includeOptional) {
    $cats = if ($includeOptional) { 'Junk', 'Optional' } else { @('Junk') }
    $title = if ($includeOptional) { 'Clean junk + optional' } else { 'Clean junk' }
    $junk = @($script:Entries.Values | Where-Object { $_.Category -in $cats -and $_.Enabled } | Sort-Object Category, Kind, Name)
    if (-not $junk.Count) {
        [System.Windows.Forms.MessageBox]::Show('Nothing left to clean - all of these are already switched off.', $title, 'OK', 'Information') | Out-Null
        return
    }
    $preview = ($junk | Select-Object -First 35 | ForEach-Object { "{0} [{1}]  {2}" -f $(if ($_.Category -eq 'Optional') { '(opt)' } else { '' }), $KindNames[$_.Kind], $_.Name }) -join "`n"
    if ($junk.Count -gt 35) { $preview += "`n... and $($junk.Count - 35) more" }
    $msg = "Switch off $($junk.Count) item(s)?`n`n$preview`n`nNothing is deleted. Gaming / hardware items are never included.`n'Undo last change' reverts all of it."
    if ([System.Windows.Forms.MessageBox]::Show($msg, $title, 'YesNo', 'Question') -ne 'Yes') { return }
    if ((Invoke-Toggle $junk $false 'items switched off') -gt 0) {
        $extra = ''
        if ($junk | Where-Object { $_.Name -like 'Windows Spotlight*' }) { $extra = "`n`nSpotlight: also choose 'Picture' in Settings > Personalization > Background / Lock screen." }
        [System.Windows.Forms.MessageBox]::Show("Done. Sign out or reboot for everything to take effect.`nUse 'Undo last change' to revert.$extra", $title, 'OK', 'Information') | Out-Null
    }
}

function Undo-Entries($entries) {
    $entries = @($entries | Where-Object { $_ -and -not $_.Undone })
    if (-not $entries.Count) { Set-Status 'Nothing to undo.'; return 0 }
    $form.Cursor = 'WaitCursor'
    $errors = @(); $done = 0; $needExplorer = $false
    foreach ($h in ($entries | Sort-Object Batch, Time -Descending)) {
        try {
            Set-EntryEnabled $h.Kind $h.Loc ([bool]$h.From)
            $h.Undone = $true
            $done++
            if ($h.Kind -eq 'ShellExt') { $needExplorer = $true }
        } catch {
            $errors += "$($h.Name): $($_.Exception.Message)"
        }
    }
    Save-History
    Load-Entries
    $form.Cursor = 'Default'
    Set-Status ("Undid $done change(s).  " + $statusLabel.Text)
    if ($errors.Count) {
        [System.Windows.Forms.MessageBox]::Show("Could not undo:`n$($errors -join "`n")", 'Startup Cleaner', 'OK', 'Warning') | Out-Null
    }
    if ($needExplorer) { Ask-RestartExplorer }
    return $done
}

function Undo-LastBatch {
    $pending = @($script:History | Where-Object { -not $_.Undone })
    if (-not $pending.Count) { Set-Status 'Nothing to undo.'; return }
    $last = ($pending | Measure-Object -Property Batch -Maximum).Maximum
    $entries = @($pending | Where-Object { $_.Batch -eq $last })
    $list = ($entries | Select-Object -First 30 | ForEach-Object { "$($_.Name): switch $(if ($_.From) { 'back on' } else { 'back off' })" }) -join "`n"
    if ($entries.Count -gt 30) { $list += "`n... and $($entries.Count - 30) more" }
    if ([System.Windows.Forms.MessageBox]::Show("Undo the last change?`n`n$list", 'Undo', 'YesNo', 'Question') -eq 'Yes') {
        [void](Undo-Entries $entries)
    }
}

function Restart-Explorer {
    Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 2
    if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }
    Set-Status 'Explorer restarted.'
}

function Ask-RestartExplorer {
    if ([System.Windows.Forms.MessageBox]::Show('Explorer extensions change after Explorer restarts. Restart it now? (taskbar will blink)', 'Startup Cleaner', 'YesNo', 'Question') -eq 'Yes') {
        Restart-Explorer
    }
}

function Show-History {
    $hf = New-Object System.Windows.Forms.Form
    $hf.Text = 'Change history'; $hf.Size = New-Object System.Drawing.Size(950, 520)
    $hf.StartPosition = 'CenterParent'; $hf.Font = $form.Font

    $ht = New-Object System.Data.DataTable
    foreach ($c in 'Idx', 'Time', 'Name', 'Type', 'Location', 'Change', 'State') { [void]$ht.Columns.Add($c) }
    $ht.Columns['Idx'].DataType = [int]

    $hg = New-Object System.Windows.Forms.DataGridView
    $hg.Dock = 'Fill'; $hg.ReadOnly = $true; $hg.AllowUserToAddRows = $false; $hg.RowHeadersVisible = $false
    $hg.SelectionMode = 'FullRowSelect'; $hg.MultiSelect = $true; $hg.AutoSizeColumnsMode = 'Fill'
    $hg.BackgroundColor = [System.Drawing.SystemColors]::Window; $hg.BorderStyle = 'None'
    $hg.DataSource = $ht
    $hg.Add_DataBindingComplete({ if ($hg.Columns.Contains('Idx')) { $hg.Columns['Idx'].Visible = $false } })

    $fill = {
        $ht.Rows.Clear()
        for ($i = $script:History.Count - 1; $i -ge 0; $i--) {
            $h = $script:History[$i]
            [void]$ht.Rows.Add($i, $h.Time, $h.Name, $KindNames[[string]$h.Kind], $h.Location,
                $(if ($h.To) { 'Switched on' } else { 'Switched off' }), $(if ($h.Undone) { 'Undone' } else { 'Active' }))
        }
        $hg.ClearSelection()
    }

    $hb = New-Object System.Windows.Forms.FlowLayoutPanel
    $hb.Dock = 'Bottom'; $hb.Height = 44; $hb.Padding = New-Object System.Windows.Forms.Padding(6, 6, 6, 0)
    $bUndoSel = New-Button 'Undo selected' 120
    $bUndoAll = New-Button 'Undo everything' 130
    $bClear   = New-Button 'Clear undone entries' 150
    $bClose   = New-Button 'Close' 90
    $hb.Controls.AddRange(@($bUndoSel, $bUndoAll, $bClear, $bClose))

    $bUndoSel.Add_Click({
        $sel = @($hg.SelectedRows | ForEach-Object { $script:History[[int]$_.Cells['Idx'].Value] })
        if (-not $sel.Count) { return }
        [void](Undo-Entries $sel); & $fill
    })
    $bUndoAll.Add_Click({
        $pending = @($script:History | Where-Object { -not $_.Undone })
        if (-not $pending.Count) { return }
        if ([System.Windows.Forms.MessageBox]::Show("Undo all $($pending.Count) active change(s)?", 'Undo everything', 'YesNo', 'Question') -eq 'Yes') {
            [void](Undo-Entries $pending); & $fill
        }
    })
    $bClear.Add_Click({
        $keep = @($script:History | Where-Object { -not $_.Undone })
        $script:History.Clear(); foreach ($k in $keep) { [void]$script:History.Add($k) }
        Save-History; & $fill
    })
    $bClose.Add_Click({ $hf.Close() })

    $hf.Controls.Add($hg); $hf.Controls.Add($hb)
    & $fill
    [void]$hf.ShowDialog($form)
}

# ---------------------------------------------------------------- wiring
$btnOff.Add_Click({ Invoke-Switch $false })
$btnOn.Add_Click({ Invoke-Switch $true })
$btnClean.Add_Click({ Invoke-CleanJunk $false })
$btnCleanOpt.Add_Click({ Invoke-CleanJunk $true })
$btnUndo.Add_Click({ Undo-LastBatch })
$btnHistory.Add_Click({ Show-History })
$btnRefresh.Add_Click({ Load-Entries; Apply-Filter })
$btnExplorer.Add_Click({ Restart-Explorer })
$txtSearch.Add_TextChanged({ Apply-Filter })
$cmbKind.Add_SelectedIndexChanged({ Apply-Filter })
$chkEnabled.Add_CheckedChanged({ Apply-Filter })
$chkWinTasks.Add_CheckedChanged({ Apply-Filter })

$tips = New-Object System.Windows.Forms.ToolTip
$tips.SetToolTip($btnClean, 'Switches off everything marked junk (orange). Shows a preview first. Never touches gaming items.')
$tips.SetToolTip($btnCleanOpt, 'Junk (orange) plus optional (blue) items - each optional one costs a small convenience; see the Advice column.')
$tips.SetToolTip($btnOff, 'Switch off the selected items (nothing is deleted).')
$tips.SetToolTip($btnOn, 'Switch the selected items back on.')
$tips.SetToolTip($btnExplorer, 'Needed for Explorer extension changes to take effect.')

$form.Add_Shown({
    Load-History
    Load-Entries
    Apply-Filter
    $grid.ClearSelection()
    $txtSearch.Focus()
})

[void]$form.ShowDialog()
