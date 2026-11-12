#Requires -Version 5.1
<#
  Service Manager - a small GUI to change Windows service startup types, with full undo.

  - Select one or more services, click Disable / Manual / Automatic.
  - Every change is logged to %LOCALAPPDATA%\ServiceManager\history.json (survives reboots).
  - "Undo last change" reverts the most recent batch; "History..." lets you undo any entry
    or import a backup CSV made by Optimize-Services.ps1.
  - Gaming dependencies (Xbox / Game Pass / Steam / EAC) are highlighted and ask for confirmation.
  - Core Windows services are locked so you can't make the PC unbootable.

  Launch with ServiceManager.cmd (double-click), or:
      powershell -ExecutionPolicy Bypass -File "<folder>\ServiceManager.ps1"
#>

# ---------------------------------------------------------------- elevation
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    try {
        Start-Process powershell.exe -Verb RunAs -ArgumentList "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSCommandPath`""
    } catch {
        Write-Host 'Administrator rights are required to change services.' -ForegroundColor Red
    }
    return
}

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

# The console is hidden, so surface any unexpected error in a dialog instead of failing silently.
trap {
    [System.Windows.Forms.MessageBox]::Show("Unexpected error:`n`n$($_.Exception.Message)`n`n$($_.InvocationInfo.PositionMessage)", 'Service Manager', 'OK', 'Error') | Out-Null
    continue
}

# ---------------------------------------------------------------- data
$DataDir  = Join-Path $env:LOCALAPPDATA 'ServiceManager'
$HistFile = Join-Path $DataDir 'history.json'
if (-not (Test-Path $DataDir)) { New-Item -ItemType Directory -Path $DataDir | Out-Null }

# Xbox / Game Pass / Steam / anti-cheat and everything they depend on
$Gaming = @(
    'GamingServices','GamingServicesNet','XblAuthManager','XblGameSave','XboxNetApiSvc','XboxGipSvc',
    'XboxService','GameInputSvc','GameInputRedistService','BcastDVRUserService','Steam Client Service',
    'EasyAntiCheat_EOS','EasyAntiCheat','BEService','InstallService','ClipSVC','LicenseManager','wlidsvc',
    'TokenBroker','AppXSvc','iphlpsvc','IKEEXT','KeyIso','WinHttpAutoProxySvc','DoSvc','BITS','UsoSvc',
    'wuauserv','hidserv','bthserv','Audiosrv','AudioEndpointBuilder','SSDPSRV','upnphost','QWAVE',
    'WpnService','NVDisplay.ContainerLocalSystem','amd3dvcacheSvc','CoworkVMServiceStore','HvHost','SharedAccess',
    'DusmSvc',      # Store / Xbox app downloads can hang at "Pending" without it
    'NcbService',   # Network Connection Broker - Xbox app notifications / invites
    'TimeBrokerSvc', # brokers background tasks for Store / Xbox apps
    'RmSvc'         # Radio Management - Bluetooth on/off and airplane mode (Bluetooth controllers / headsets)
)

# Changing these can stop Windows from booting or logging in - locked.
$Critical = @(
    'RpcSs','RpcEptMapper','DcomLaunch','LSM','PlugPlay','Power','ProfSvc','EventLog','BrokerInfrastructure',
    'SamSs','CryptSvc','Winmgmt','CoreMessagingRegistrar','StateRepository','UserManager','SystemEventsBroker',
    'gpsvc','Schedule','BFE','mpssvc','Dhcp','Dnscache','nsi','WinDefend','SecurityHealthService','wscsvc',
    'MDCoreSvc','WdNisSvc','EventSystem','TrustedInstaller','msiserver','AppIDSvc','Appinfo','camsvc',
    'TextInputManagementService','DispBrokerDesktopSvc','Themes','FontCache','NgcSvc','NgcCtnrSvc','VaultSvc',
    'KeyIso','LanmanWorkstation','Netman','netprofm','NlaSvc','Wcmsvc','sppsvc','WaaSMedicSvc','DeviceInstall',
    'DsmSvc','Audiosrv','AudioEndpointBuilder','SENS','ShellHWDetection'
)

# ---- Presets (same lists as Optimize-Services.ps1). None of these affect Xbox / Steam.
$PresetSafe = [ordered]@{
    # telemetry / ads / demo
    'DiagTrack' = 'Disabled'; 'dmwappushservice' = 'Disabled'; 'wuqisvc' = 'Disabled'; 'RetailDemo' = 'Disabled'
    'MapsBroker' = 'Disabled'; 'lfsvc' = 'Disabled'; 'wisvc' = 'Disabled'; 'InventorySvc' = 'Manual'
    'PcaSvc' = 'Manual'; 'TrkWks' = 'Disabled'; 'WerSvc' = 'Manual'
    # printing / scanning / phone (no printer or camera on this PC)
    'Spooler' = 'Disabled'; 'PrintNotify' = 'Disabled'; 'PrintDeviceConfigurationService' = 'Disabled'
    'PrintScanBrokerService' = 'Disabled'; 'StiSvc' = 'Disabled'; 'WiaRpc' = 'Disabled'; 'TapiSrv' = 'Disabled'
    'PhoneSvc' = 'Disabled'; 'SmsRouter' = 'Disabled'
    # remote access you don't host
    'TermService' = 'Disabled'; 'SessionEnv' = 'Disabled'; 'UmRdpService' = 'Disabled'; 'WinRM' = 'Disabled'
    'RemoteRegistry' = 'Disabled'; 'WebClient' = 'Disabled'; 'RasAuto' = 'Disabled'
    # networking roles not used (wired Ethernet, not domain-joined)
    'icssvc' = 'Disabled'; 'WwanSvc' = 'Disabled'; 'autotimesvc' = 'Disabled'; 'WlanSvc' = 'Manual'
    'PeerDistSvc' = 'Disabled'; 'MSiSCSI' = 'Disabled'; 'lltdsvc' = 'Disabled'; 'NcdAutoSetup' = 'Disabled'
    'wcncsvc' = 'Disabled'; 'Netlogon' = 'Disabled'; 'CscService' = 'Disabled'
    # misc consumer features
    'SEMgrSvc' = 'Disabled'; 'WalletService' = 'Disabled'; 'WpcMonSvc' = 'Disabled'; 'WMPNetworkSvc' = 'Disabled'
    'fhsvc' = 'Disabled'; 'SensorService' = 'Disabled'; 'SensorDataService' = 'Disabled'; 'SensrSvc' = 'Disabled'
    'edgeupdate' = 'Manual'; 'MicrosoftEdgeElevationService' = 'Disabled'; 'MicrosoftCopilotElevationService' = 'Disabled'
    'StorSvc' = 'Manual'   # Storage Service: trigger-starts on demand (Store / Game Pass installs still start it) - never Disabled
    # Hyper-V *guest* integration (this PC is a host)
    'vmicguestinterface' = 'Disabled'; 'vmicheartbeat' = 'Disabled'; 'vmickvpexchange' = 'Disabled'
    'vmicrdv' = 'Disabled'; 'vmicshutdown' = 'Disabled'; 'vmictimesync' = 'Disabled'
    'vmicvmsession' = 'Disabled'; 'vmicvss' = 'Disabled'
}
$PresetOptional = [ordered]@{
    'WSearch' = 'Disabled'; 'SysMain' = 'Manual'; 'LanmanServer' = 'Manual'
    # Diagnostics trio: troubleshooters, problem detection and Task Manager "App history" stop working
    'DPS' = 'Disabled'; 'WdiServiceHost' = 'Disabled'; 'WdiSystemHost' = 'Disabled'
    'WSAIFabricSvc' = 'Manual'; 'CDPSvc' = 'Manual'
    'lmhosts' = 'Disabled'  # NetBIOS name lookup of old LAN devices (DNS keeps working); trigger restarts it if Manual
    'PcaSvc' = 'Disabled'   # Program Compatibility Assistant prompts (overrides the Safe preset's Manual)
    'logi_lamparray_service' = 'Manual'; 'WbioSrvc' = 'Disabled'; 'SCardSvr' = 'Disabled'
    'ScDeviceEnum' = 'Disabled'; 'SCPolicySvc' = 'Disabled'; 'CertPropSvc' = 'Disabled'
    'whesvc' = 'Manual'   # Windows Health and Optimized Experiences: slowness traces + feedback toasts
}

$StartNames = @{ 0 = 'Boot'; 1 = 'System'; 2 = 'Automatic'; 3 = 'Manual'; 4 = 'Disabled' }
$SvcRoot    = 'HKLM:\SYSTEM\CurrentControlSet\Services'

$script:History = New-Object System.Collections.ArrayList

# ---------------------------------------------------------------- helpers
function Get-BaseName([string]$name) {
    # Per-user services look like CDPUserSvc_70560 -> CDPUserSvc
    if ($name -match '^(.+)_[0-9a-fA-F]{4,8}$') { return $Matches[1] }
    return $name
}

function Get-Template([string]$name) {
    $base = Get-BaseName $name
    if ($base -ne $name -and (Test-Path -LiteralPath "$SvcRoot\$base")) { return $base }
    return $null
}

function Get-Note([string]$name) {
    $base = Get-BaseName $name
    # $script: so a caller's local variable with the same name can never shadow these lists
    if ($script:Critical -contains $name -or $script:Critical -contains $base) { return 'Core Windows (locked)' }
    if ($script:Gaming   -contains $name -or $script:Gaming   -contains $base) { return 'Gaming dependency' }
    if (Get-Template $name) { return 'Per-user service' }
    return ''
}

function Get-StartType([string]$name) {
    $p = Get-ItemProperty -LiteralPath "$SvcRoot\$name" -ErrorAction SilentlyContinue
    if ((-not $p -or $null -eq $p.Start) -and ($template = Get-Template $name)) {
        $p = Get-ItemProperty -LiteralPath "$SvcRoot\$template" -ErrorAction SilentlyContinue
    }
    if (-not $p -or $null -eq $p.Start) { return 'Unknown' }
    $t = $StartNames[[int]$p.Start]
    if (-not $t) { return 'Unknown' }
    if ($t -eq 'Automatic' -and $p.DelayedAutostart -eq 1) { $t = 'Automatic (Delayed)' }
    return $t
}

function Set-RegistryStart([string]$name, [string]$type) {
    $num = @{ 'Disabled' = 4; 'Manual' = 3; 'Automatic' = 2; 'Automatic (Delayed)' = 2 }[$type]
    $key = "$SvcRoot\$name"
    Set-ItemProperty -LiteralPath $key -Name Start -Value $num -Type DWord -ErrorAction Stop
    if ($type -like 'Automatic*') {
        Set-ItemProperty -LiteralPath $key -Name DelayedAutostart -Value ([int]($type -eq 'Automatic (Delayed)')) -Type DWord -ErrorAction Stop
    }
}

# Returns a short note on how it was applied; throws on failure.
function Set-StartType([string]$name, [string]$type) {
    $scArg = switch ($type) {
        'Disabled'            { 'disabled' }
        'Manual'              { 'demand' }
        'Automatic'           { 'auto' }
        'Automatic (Delayed)' { 'delayed-auto' }
        default               { throw "Cannot set start type '$type'." }
    }

    # Per-user services: Windows recreates the instance from its template at each logon,
    # so change the template too (registry only - sc.exe refuses templates).
    $template = Get-Template $name
    if ($template) {
        Set-RegistryStart $template $type
        Set-RegistryStart $name $type
        return 'per-user, applies after sign-out/reboot'
    }

    $out = & sc.exe config "$name" start= $scArg
    if ($LASTEXITCODE -eq 0) { return 'ok' }

    # Some services are ACL-protected from sc.exe; the registry usually still works.
    try {
        Set-RegistryStart $name $type
        return 'via registry, applies after reboot'
    } catch {
        throw "sc.exe: $(($out | Where-Object { $_ }) -join ' ') / registry: $($_.Exception.Message)"
    }
}

function Load-History {
    $script:History.Clear()
    if (Test-Path $HistFile) {
        try {
            $raw  = Get-Content -LiteralPath $HistFile -Raw
            $data = ConvertFrom-Json $raw
            foreach ($h in @($data)) { if ($h) { [void]$script:History.Add($h) } }
        } catch {
            [System.Windows.Forms.MessageBox]::Show("Could not read history file:`n$HistFile`n`n$($_.Exception.Message)", 'Service Manager', 'OK', 'Warning') | Out-Null
        }
    }
}

function Save-History {
    ConvertTo-Json -InputObject @($script:History.ToArray()) -Depth 3 | Set-Content -LiteralPath $HistFile -Encoding UTF8
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
$form.Text = 'Service Manager'
$form.Size = New-Object System.Drawing.Size(1100, 720)
$form.MinimumSize = New-Object System.Drawing.Size(800, 450)
$form.StartPosition = 'CenterScreen'
$form.Font = New-Object System.Drawing.Font('Segoe UI', 9)

# top bar
$top = New-Object System.Windows.Forms.FlowLayoutPanel
$top.Dock = 'Top'; $top.Height = 38; $top.Padding = New-Object System.Windows.Forms.Padding(6, 6, 6, 0)

$lblSearch = New-Object System.Windows.Forms.Label
$lblSearch.Text = 'Search:'; $lblSearch.AutoSize = $true; $lblSearch.Margin = New-Object System.Windows.Forms.Padding(0, 6, 4, 0)
$txtSearch = New-Object System.Windows.Forms.TextBox
$txtSearch.Width = 260

$chkRunning = New-Object System.Windows.Forms.CheckBox
$chkRunning.Text = 'Running only'; $chkRunning.AutoSize = $true; $chkRunning.Margin = New-Object System.Windows.Forms.Padding(16, 4, 0, 0)
$chkHideLocked = New-Object System.Windows.Forms.CheckBox
$chkHideLocked.Text = 'Hide core Windows services'; $chkHideLocked.AutoSize = $true; $chkHideLocked.Margin = New-Object System.Windows.Forms.Padding(16, 4, 0, 0)

$btnPresetSafe = New-Object System.Windows.Forms.Button
$btnPresetSafe.Text = 'Apply Safe preset'; $btnPresetSafe.Width = 140; $btnPresetSafe.Height = 26
$btnPresetSafe.Margin = New-Object System.Windows.Forms.Padding(24, 0, 0, 0)
$btnPresetOpt = New-Object System.Windows.Forms.Button
$btnPresetOpt.Text = 'Apply Safe + Optional preset'; $btnPresetOpt.Width = 200; $btnPresetOpt.Height = 26
$btnPresetOpt.Margin = New-Object System.Windows.Forms.Padding(6, 0, 0, 0)

$top.Controls.AddRange(@($lblSearch, $txtSearch, $chkRunning, $chkHideLocked, $btnPresetSafe, $btnPresetOpt))

# grid
$table = New-Object System.Data.DataTable
foreach ($c in 'Name', 'DisplayName', 'Startup', 'Status', 'Note') { [void]$table.Columns.Add($c) }
$table.DefaultView.Sort = 'DisplayName ASC'

$grid = New-Object System.Windows.Forms.DataGridView
$grid.Dock = 'Fill'
$grid.ReadOnly = $true
$grid.AllowUserToAddRows = $false
$grid.AllowUserToDeleteRows = $false
$grid.AllowUserToResizeRows = $false
$grid.SelectionMode = 'FullRowSelect'
$grid.MultiSelect = $true
$grid.RowHeadersVisible = $false
$grid.AutoSizeColumnsMode = 'Fill'
$grid.BackgroundColor = [System.Drawing.SystemColors]::Window
$grid.BorderStyle = 'None'
$grid.DataSource = $table

# bottom bar
$bottom = New-Object System.Windows.Forms.FlowLayoutPanel
$bottom.Dock = 'Bottom'; $bottom.Height = 44; $bottom.Padding = New-Object System.Windows.Forms.Padding(6, 6, 6, 0)

function New-Button([string]$text, [int]$width = 110) {
    $b = New-Object System.Windows.Forms.Button
    $b.Text = $text; $b.Width = $width; $b.Height = 30
    return $b
}
$btnDisable = New-Button 'Disable'
$btnManual  = New-Button 'Manual'
$btnAuto    = New-Button 'Automatic'
$chkStop = New-Object System.Windows.Forms.CheckBox
$chkStop.Text = 'Also stop service when disabling'; $chkStop.Checked = $true; $chkStop.AutoSize = $true
$chkStop.Margin = New-Object System.Windows.Forms.Padding(12, 8, 24, 0)
$btnUndo    = New-Button 'Undo last change' 130
$btnHistory = New-Button 'History...'
$btnRefresh = New-Button 'Refresh' 90
$bottom.Controls.AddRange(@($btnDisable, $btnManual, $btnAuto, $chkStop, $btnUndo, $btnHistory, $btnRefresh))

$status = New-Object System.Windows.Forms.StatusStrip
$statusLabel = New-Object System.Windows.Forms.ToolStripStatusLabel
$statusLabel.Spring = $true; $statusLabel.TextAlign = 'MiddleLeft'
[void]$status.Items.Add($statusLabel)
$legend = New-Object System.Windows.Forms.ToolStripStatusLabel
$legend.Text = 'Yellow = gaming dependency   Red = locked   Grey = disabled'
$legend.ForeColor = [System.Drawing.Color]::DimGray
[void]$status.Items.Add($legend)

$form.Controls.Add($grid)
$form.Controls.Add($top)
$form.Controls.Add($bottom)
$form.Controls.Add($status)

function Set-Status([string]$text) { $statusLabel.Text = $text }

# ---------------------------------------------------------------- grid logic
function Apply-Filter {
    $parts = @()
    $q = $txtSearch.Text.Trim()
    if ($q) { $e = Escape-Like $q; $parts += "(Name LIKE '*$e*' OR DisplayName LIKE '*$e*')" }
    if ($chkRunning.Checked)    { $parts += "Status = 'Running'" }
    if ($chkHideLocked.Checked) { $parts += "Note <> 'Core Windows (locked)'" }
    $table.DefaultView.RowFilter = ($parts -join ' AND ')
}

function Get-SelectedNames {
    return @($grid.SelectedRows | ForEach-Object { [string]$_.Cells['Name'].Value })
}

function Load-Services {
    $selected = Get-SelectedNames
    $scroll = $grid.FirstDisplayedScrollingRowIndex

    $table.BeginLoadData()
    $table.Rows.Clear()
    foreach ($s in Get-CimInstance Win32_Service) {
        [void]$table.Rows.Add($s.Name, $s.DisplayName, (Get-StartType $s.Name), $s.State, (Get-Note $s.Name))
    }
    $table.EndLoadData()

    $grid.ClearSelection()
    if ($selected.Count) {
        foreach ($r in $grid.Rows) { if ($selected -contains [string]$r.Cells['Name'].Value) { $r.Selected = $true } }
    }
    if ($scroll -ge 0 -and $scroll -lt $grid.Rows.Count) { $grid.FirstDisplayedScrollingRowIndex = $scroll }

    $disabled = @($table.Select("Startup = 'Disabled'")).Count
    $running  = @($table.Select("Status = 'Running'")).Count
    $pending  = @($script:History | Where-Object { -not $_.Undone }).Count
    Set-Status ("{0} services  |  {1} running  |  {2} disabled  |  {3} change(s) can be undone" -f $table.Rows.Count, $running, $disabled, $pending)
}

$grid.Add_DataBindingComplete({
    if ($grid.Columns.Contains('DisplayName')) {
        $grid.Columns['DisplayName'].HeaderText = 'Display name'
        $grid.Columns['DisplayName'].FillWeight = 220
        $grid.Columns['Name'].FillWeight = 120
        $grid.Columns['Startup'].FillWeight = 80
        $grid.Columns['Status'].FillWeight = 55
        $grid.Columns['Note'].FillWeight = 90
    }
})

$colorGaming   = [System.Drawing.Color]::FromArgb(255, 248, 205)
$colorCritical = [System.Drawing.Color]::FromArgb(253, 222, 222)
$grid.Add_CellFormatting({
    param($src, $e)
    if ($e.RowIndex -lt 0) { return }
    $row = $grid.Rows[$e.RowIndex]
    $note = [string]$row.Cells['Note'].Value
    if ($note -eq 'Gaming dependency')         { $e.CellStyle.BackColor = $colorGaming }
    elseif ($note -eq 'Core Windows (locked)') { $e.CellStyle.BackColor = $colorCritical }
    if ([string]$row.Cells['Startup'].Value -eq 'Disabled') { $e.CellStyle.ForeColor = [System.Drawing.Color]::Gray }
})

# ---------------------------------------------------------------- actions
function Invoke-Change([string[]]$names, [string]$type) {
    $names = @($names | Where-Object { $_ })
    if (-not $names.Count) { Set-Status 'Select one or more services first.'; return }

    $locked = @($names | Where-Object { (Get-Note $_) -eq 'Core Windows (locked)' })
    if ($locked.Count) {
        [System.Windows.Forms.MessageBox]::Show("These are core Windows services and are locked:`n`n$($locked -join "`n")", 'Service Manager', 'OK', 'Information') | Out-Null
        $names = @($names | Where-Object { $locked -notcontains $_ })
        if (-not $names.Count) { return }
    }

    $selGaming = @($names | Where-Object { (Get-Note $_) -eq 'Gaming dependency' })
    if ($selGaming.Count -and $type -ne 'Automatic') {
        $msg = "These are needed by Xbox / Game Pass / Steam / anti-cheat:`n`n$($selGaming -join "`n")`n`nSet them to $type anyway?"
        if ([System.Windows.Forms.MessageBox]::Show($msg, 'Gaming dependency', 'YesNo', 'Warning') -ne 'Yes') { return }
    }

    $plan = @($names | ForEach-Object { [pscustomobject]@{ Name = $_; To = $type } })
    [void](Invoke-Plan $plan "set to $type")
}

# Keeps only entries for installed services whose startup type would actually change.
function Get-PendingPlan($plan) {
    $work = @()
    foreach ($p in $plan) {
        if (-not (Test-Path -LiteralPath "$SvcRoot\$($p.Name)")) { continue }
        $from = Get-StartType $p.Name
        if ($from -eq $p.To -or ($p.To -eq 'Automatic' -and $from -eq 'Automatic (Delayed)')) { continue }
        $work += [pscustomobject]@{ Name = $p.Name; From = $from; To = $p.To }
    }
    return $work
}

# Applies a list of {Name, To} as ONE history batch, so "Undo last change" reverts all of it.
function Invoke-Plan($plan, [string]$verb) {
    $work = @(Get-PendingPlan $plan)
    $skipped = @($plan).Count - $work.Count
    if (-not $work.Count) { Set-Status "Nothing to change - all already set.  "; return 0 }

    $form.Cursor = 'WaitCursor'
    $batch = Get-NextBatch
    $ok = 0; $errors = @(); $notes = @()
    foreach ($w in $work) {
        $n = $w.Name
        $svc = Get-Service -Name $n -ErrorAction SilentlyContinue
        $wasRunning = [bool]($svc -and $svc.Status -eq 'Running')
        try {
            $how = Set-StartType $n $w.To
        } catch {
            $errors += "${n}: $($_.Exception.Message)"; continue
        }
        if ($how -ne 'ok') { $notes += "${n}: $how" }
        [void]$script:History.Add([pscustomobject]@{
            Batch       = $batch
            Time        = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
            Name        = $n
            DisplayName = if ($svc) { $svc.DisplayName } else { $n }
            From        = $w.From
            To          = $w.To
            WasRunning  = $wasRunning
            Undone      = $false
        })
        $ok++
        if ($w.To -eq 'Disabled' -and $chkStop.Checked -and $wasRunning) {
            try { Stop-Service -Name $n -Force -ErrorAction Stop } catch { $notes += "${n}: changed, but could not be stopped now ($($_.Exception.Message))" }
        }
    }
    Save-History
    Load-Services
    $form.Cursor = 'Default'

    $summary = "$ok $verb"
    if ($skipped) { $summary += ", $skipped already set" }
    if ($errors.Count) { $summary += ", $($errors.Count) failed" }
    Set-Status ($summary + '.  ' + $statusLabel.Text)
    if ($errors.Count -or $notes.Count) {
        $text = ''
        if ($errors.Count) { $text += "Failed:`n$($errors -join "`n")`n`n" }
        if ($notes.Count)  { $text += "Notes:`n$($notes -join "`n")" }
        [System.Windows.Forms.MessageBox]::Show($text.Trim(), 'Service Manager', 'OK', $(if ($errors.Count) { 'Warning' } else { 'Information' })) | Out-Null
    }
    return $ok
}

function Get-PresetPlan([bool]$includeOptional) {
    $all = [ordered]@{}
    foreach ($k in $PresetSafe.Keys) { $all[$k] = $PresetSafe[$k] }
    if ($includeOptional) { foreach ($k in $PresetOptional.Keys) { $all[$k] = $PresetOptional[$k] } }
    # Belt and braces: never let a preset touch a gaming or core service.
    return @($all.Keys | Where-Object { (Get-Note $_) -notin 'Gaming dependency', 'Core Windows (locked)' } |
        ForEach-Object { [pscustomobject]@{ Name = $_; To = $all[$_] } })
}

function Invoke-Preset([bool]$includeOptional) {
    $name = if ($includeOptional) { 'Safe + Optional' } else { 'Safe' }
    $plan = Get-PresetPlan $includeOptional
    $work = @(Get-PendingPlan $plan)
    if (-not $work.Count) {
        [System.Windows.Forms.MessageBox]::Show("The $name preset is already fully applied.", 'Preset', 'OK', 'Information') | Out-Null
        return
    }
    $preview = ($work | Select-Object -First 30 | ForEach-Object { "{0}:  {1} -> {2}" -f $_.Name, $_.From, $_.To }) -join "`n"
    if ($work.Count -gt 30) { $preview += "`n... and $($work.Count - 30) more" }
    $msg = "Apply the $name preset? $($work.Count) service(s) will change:`n`n$preview`n`nNo Xbox / Steam services are included. 'Undo last change' reverts the whole preset."
    if ([System.Windows.Forms.MessageBox]::Show($msg, "$name preset", 'YesNo', 'Question') -ne 'Yes') { return }
    if ((Invoke-Plan $plan "changed by $name preset") -gt 0) {
        [System.Windows.Forms.MessageBox]::Show("$name preset applied. Reboot to finish.`nUse 'Undo last change' to revert it.", 'Preset', 'OK', 'Information') | Out-Null
    }
}

function Undo-Entries($entries) {
    $entries = @($entries | Where-Object { $_ -and -not $_.Undone })
    if (-not $entries.Count) { Set-Status 'Nothing to undo.'; return 0 }
    $form.Cursor = 'WaitCursor'
    $errors = @(); $done = 0
    # newest first, so stacked changes to the same service unwind in order
    foreach ($h in ($entries | Sort-Object Batch, Time -Descending)) {
        try {
            [void](Set-StartType $h.Name $h.From)
            $h.Undone = $true
            $done++
            if ($h.WasRunning) { Start-Service -Name $h.Name -ErrorAction SilentlyContinue }
        } catch {
            $errors += "$($h.Name): $($_.Exception.Message)"
        }
    }
    Save-History
    Load-Services
    $form.Cursor = 'Default'
    Set-Status ("Undid $done change(s).  " + $statusLabel.Text)
    if ($errors.Count) {
        [System.Windows.Forms.MessageBox]::Show("Could not undo:`n$($errors -join "`n")", 'Service Manager', 'OK', 'Warning') | Out-Null
    }
    return $done
}

function Undo-LastBatch {
    $pending = @($script:History | Where-Object { -not $_.Undone })
    if (-not $pending.Count) { Set-Status 'Nothing to undo.'; return }
    $last = ($pending | Measure-Object -Property Batch -Maximum).Maximum
    $entries = @($pending | Where-Object { $_.Batch -eq $last })
    $list = ($entries | ForEach-Object { "$($_.Name): $($_.To) -> $($_.From)" }) -join "`n"
    if ([System.Windows.Forms.MessageBox]::Show("Undo the last change?`n`n$list", 'Undo', 'YesNo', 'Question') -eq 'Yes') {
        [void](Undo-Entries $entries)
    }
}

function Import-BackupCsv {
    $dlg = New-Object System.Windows.Forms.OpenFileDialog
    $dlg.Filter = 'Service backup (*.csv)|*.csv'
    $dlg.InitialDirectory = $PSScriptRoot
    if ($dlg.ShowDialog() -ne 'OK') { return }

    $map = @{ 'Auto' = 'Automatic'; 'Automatic' = 'Automatic'; 'Manual' = 'Manual'; 'Disabled' = 'Disabled' }
    $changes = @()
    foreach ($row in Import-Csv -LiteralPath $dlg.FileName) {
        $want = $map[[string]$row.StartMode]
        if (-not $want -or -not $row.Name) { continue }
        if (-not (Test-Path -LiteralPath "$SvcRoot\$($row.Name)")) { continue }
        $now = Get-StartType $row.Name
        if ($now -eq $want -or ($want -eq 'Automatic' -and $now -eq 'Automatic (Delayed)')) { continue }
        $changes += [pscustomobject]@{ Name = $row.Name; From = $now; To = $want }
    }
    if (-not $changes.Count) {
        [System.Windows.Forms.MessageBox]::Show('Every service already matches that backup.', 'Restore', 'OK', 'Information') | Out-Null
        return
    }
    $preview = ($changes | Select-Object -First 25 | ForEach-Object { "$($_.Name): $($_.From) -> $($_.To)" }) -join "`n"
    if ($changes.Count -gt 25) { $preview += "`n... and $($changes.Count - 25) more" }
    if ([System.Windows.Forms.MessageBox]::Show("Restore $($changes.Count) service(s) from the backup?`n`n$preview", 'Restore', 'YesNo', 'Question') -ne 'Yes') { return }

    # Group by target type so each lands in the history as one undoable batch.
    $batch = Get-NextBatch; $errors = @(); $ok = 0
    foreach ($c in $changes) {
        try {
            [void](Set-StartType $c.Name $c.To)
            $svc = Get-Service -Name $c.Name -ErrorAction SilentlyContinue
            [void]$script:History.Add([pscustomobject]@{
                Batch = $batch; Time = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss'); Name = $c.Name
                DisplayName = if ($svc) { $svc.DisplayName } else { $c.Name }
                From = $c.From; To = $c.To; WasRunning = [bool]($svc -and $svc.Status -eq 'Running'); Undone = $false
            })
            $ok++
        } catch { $errors += "$($c.Name): $($_.Exception.Message)" }
    }
    Save-History
    Load-Services
    $msg = "Restored $ok service(s). Reboot to apply fully."
    if ($errors.Count) { $msg += "`n`nFailed:`n$($errors -join "`n")" }
    [System.Windows.Forms.MessageBox]::Show($msg, 'Restore', 'OK', 'Information') | Out-Null
}

function Show-History {
    $hf = New-Object System.Windows.Forms.Form
    $hf.Text = 'Change history'
    $hf.Size = New-Object System.Drawing.Size(900, 520)
    $hf.StartPosition = 'CenterParent'
    $hf.Font = $form.Font

    $ht = New-Object System.Data.DataTable
    foreach ($c in 'Idx', 'Time', 'Name', 'DisplayName', 'From', 'To', 'State') { [void]$ht.Columns.Add($c) }
    $ht.Columns['Idx'].DataType = [int]

    $hg = New-Object System.Windows.Forms.DataGridView
    $hg.Dock = 'Fill'; $hg.ReadOnly = $true; $hg.AllowUserToAddRows = $false; $hg.RowHeadersVisible = $false
    $hg.SelectionMode = 'FullRowSelect'; $hg.MultiSelect = $true; $hg.AutoSizeColumnsMode = 'Fill'
    $hg.BackgroundColor = [System.Drawing.SystemColors]::Window; $hg.BorderStyle = 'None'
    $hg.DataSource = $ht
    $hg.Add_DataBindingComplete({ if ($hg.Columns.Contains('Idx')) { $hg.Columns['Idx'].Visible = $false; $hg.Columns['DisplayName'].HeaderText = 'Display name' } })

    $fill = {
        $ht.Rows.Clear()
        for ($i = $script:History.Count - 1; $i -ge 0; $i--) {
            $h = $script:History[$i]
            [void]$ht.Rows.Add($i, $h.Time, $h.Name, $h.DisplayName, $h.From, $h.To, $(if ($h.Undone) { 'Undone' } else { 'Active' }))
        }
        $hg.ClearSelection()
    }

    $hb = New-Object System.Windows.Forms.FlowLayoutPanel
    $hb.Dock = 'Bottom'; $hb.Height = 44; $hb.Padding = New-Object System.Windows.Forms.Padding(6, 6, 6, 0)
    $bUndoSel = New-Button 'Undo selected' 120
    $bImport  = New-Button 'Restore from backup CSV...' 190
    $bClear   = New-Button 'Clear undone entries' 150
    $bClose   = New-Button 'Close' 90
    $hb.Controls.AddRange(@($bUndoSel, $bImport, $bClear, $bClose))

    $bUndoSel.Add_Click({
        $sel = @($hg.SelectedRows | ForEach-Object { $script:History[[int]$_.Cells['Idx'].Value] })
        if (-not $sel.Count) { return }
        [void](Undo-Entries $sel)
        & $fill
    })
    $bImport.Add_Click({ Import-BackupCsv; & $fill })
    $bClear.Add_Click({
        $keep = @($script:History | Where-Object { -not $_.Undone })
        $script:History.Clear()
        foreach ($k in $keep) { [void]$script:History.Add($k) }
        Save-History
        & $fill
    })
    $bClose.Add_Click({ $hf.Close() })

    $hf.Controls.Add($hg)
    $hf.Controls.Add($hb)
    & $fill
    [void]$hf.ShowDialog($form)
}

# ---------------------------------------------------------------- wiring
$btnDisable.Add_Click({ Invoke-Change (Get-SelectedNames) 'Disabled' })
$btnManual.Add_Click({  Invoke-Change (Get-SelectedNames) 'Manual' })
$btnAuto.Add_Click({    Invoke-Change (Get-SelectedNames) 'Automatic' })
$btnUndo.Add_Click({ Undo-LastBatch })
$btnHistory.Add_Click({ Show-History })
$btnRefresh.Add_Click({ Load-Services })
$btnPresetSafe.Add_Click({ Invoke-Preset $false })
$btnPresetOpt.Add_Click({ Invoke-Preset $true })
$txtSearch.Add_TextChanged({ Apply-Filter })
$chkRunning.Add_CheckedChanged({ Apply-Filter })
$chkHideLocked.Add_CheckedChanged({ Apply-Filter })

$tips = New-Object System.Windows.Forms.ToolTip
$tips.SetToolTip($btnDisable, 'Set selected services to Disabled (they cannot start at all).')
$tips.SetToolTip($btnManual, 'Set selected services to Manual (start only when something needs them).')
$tips.SetToolTip($btnAuto, 'Set selected services to start automatically at boot.')
$tips.SetToolTip($btnUndo, 'Revert the most recent batch of changes.')
$tips.SetToolTip($btnPresetSafe, 'Telemetry, printing, remote desktop host, unused networking and consumer extras. No gaming impact.')
$tips.SetToolTip($btnPresetOpt, 'Safe preset plus: Windows Search, SysMain, file sharing, diagnostics, Windows AI, Phone Link, Logitech RGB, smart card / biometrics.')

$form.Add_Shown({
    Load-History
    Load-Services
    $grid.ClearSelection()
    $txtSearch.Focus()
})

[void]$form.ShowDialog()
