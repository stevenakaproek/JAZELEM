<#
  Optimize-Services.ps1
  Trims Windows services on this PC (Ryzen 7 9800X3D, wired Ethernet, no printer/camera)
  while leaving every service that Xbox / Game Pass / Steam / EAC depend on untouched.

  Run from an ELEVATED PowerShell:
      powershell -ExecutionPolicy Bypass -File .\Optimize-Services.ps1            # apply SAFE tier
      powershell -ExecutionPolicy Bypass -File .\Optimize-Services.ps1 -Optional  # SAFE + OPTIONAL tiers
      powershell -ExecutionPolicy Bypass -File .\Optimize-Services.ps1 -WhatIf    # preview only
      powershell -ExecutionPolicy Bypass -File .\Optimize-Services.ps1 -Restore .\services-backup-XXXX.csv

  A backup CSV of the current start types is written before any change.
  Reboot afterwards.
#>
[CmdletBinding()]
param(
    [switch]$Optional,
    [switch]$WhatIf,
    [string]$Restore
)

$ErrorActionPreference = 'Stop'
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole('Administrators')) {
    throw 'Run this script from an elevated (Administrator) PowerShell.'
}

# ---- Never touched: gaming dependency closure (declared deps + known runtime needs) ----
$Protected = @(
    'GamingServices','GamingServicesNet','XblAuthManager','XblGameSave','XboxNetApiSvc','XboxGipSvc',
    'XboxService','GameInputSvc','GameInputRedistService','BcastDVRUserService','Steam Client Service',
    'EasyAntiCheat_EOS','InstallService','ClipSVC','LicenseManager','wlidsvc','TokenBroker','AppXSvc',
    'StateRepository','iphlpsvc','IKEEXT','BFE','mpssvc','KeyIso','WinHttpAutoProxySvc','nsi','Dhcp',
    'Dnscache','RpcSs','RpcEptMapper','DcomLaunch','UserManager','ProfSvc','BrokerInfrastructure',
    'DoSvc','UsoSvc','wuauserv','BITS','hidserv','bthserv','Audiosrv','AudioEndpointBuilder',
    'SSDPSRV','upnphost','WpnService','SharedAccess','HvHost','CoworkVMServiceStore',
    'NVDisplay.ContainerLocalSystem','AmdPpkgSvc','AmdAppCompatSvc','QWAVE','PlugPlay',
    'DusmSvc','NcbService','TimeBrokerSvc'
)

# ---- SAFE tier: no gaming impact, no hardware on this PC uses them ----
$Safe = [ordered]@{
    # Telemetry / ads / demo
    'DiagTrack'            = 'Disabled'   # Connected User Experiences and Telemetry
    'dmwappushservice'     = 'Disabled'   # WAP push (telemetry routing)
    'wuqisvc'              = 'Disabled'   # Usage and Quality Insights
    'RetailDemo'           = 'Disabled'
    'MapsBroker'           = 'Disabled'   # Downloaded Maps Manager
    'lfsvc'                = 'Disabled'   # Geolocation
    'wisvc'                = 'Disabled'   # Windows Insider
    'InventorySvc'         = 'Manual'     # Compatibility appraisal
    'PcaSvc'               = 'Manual'     # Program Compatibility Assistant
    'TrkWks'               = 'Disabled'   # Distributed Link Tracking (NTFS network links)
    'WerSvc'               = 'Manual'
    # Printing / scanning / fax / phone (no printer or camera present)
    'Spooler'              = 'Disabled'   # NOTE: also disables "Microsoft Print to PDF"
    'PrintNotify'          = 'Disabled'
    'PrintDeviceConfigurationService' = 'Disabled'
    'PrintScanBrokerService' = 'Disabled'
    'StiSvc'               = 'Disabled'   # WIA scanners/cameras
    'WiaRpc'               = 'Disabled'
    'TapiSrv'              = 'Disabled'   # Telephony
    'PhoneSvc'             = 'Disabled'
    'SmsRouter'            = 'Disabled'
    # Remote access you don't host
    'TermService'          = 'Disabled'   # Remote Desktop *host* (RDP client still works)
    'SessionEnv'           = 'Disabled'
    'UmRdpService'         = 'Disabled'
    'WinRM'                = 'Disabled'
    'RemoteRegistry'       = 'Disabled'
    'WebClient'            = 'Disabled'   # WebDAV
    'RasAuto'              = 'Disabled'
    # Networking roles not used (wired Ethernet, not domain-joined)
    'icssvc'               = 'Disabled'   # Mobile hotspot
    'WwanSvc'              = 'Disabled'   # Cellular
    'autotimesvc'          = 'Disabled'   # Cellular time
    'WlanSvc'              = 'Manual'     # Wi-Fi (you're on Ethernet)
    'PeerDistSvc'          = 'Disabled'   # BranchCache
    'MSiSCSI'              = 'Disabled'
    'lltdsvc'              = 'Disabled'
    'NcdAutoSetup'         = 'Disabled'
    'wcncsvc'              = 'Disabled'
    'Netlogon'             = 'Disabled'   # Domain only
    'CscService'           = 'Disabled'   # Offline Files
    # Misc consumer features
    'SEMgrSvc'             = 'Disabled'   # Payments / NFC
    'WalletService'        = 'Disabled'
    'WpcMonSvc'            = 'Disabled'   # Parental controls
    'WMPNetworkSvc'        = 'Disabled'
    'fhsvc'                = 'Disabled'   # File History
    'SensorService'        = 'Disabled'
    'SensorDataService'    = 'Disabled'
    'SensrSvc'             = 'Disabled'
    'edgeupdate'           = 'Manual'
    'MicrosoftCopilotElevationService' = 'Disabled'
    'StorSvc'              = 'Manual'     # trigger-starts on demand; never Disabled (Store installs need it)
    'amd3dvcacheSvc'       = 'Manual'     # Only matters on dual-CCD X3D (7950X3D/9950X3D); 9800X3D is single-CCD
    # Hyper-V *guest* integration (this PC is a host, not a guest)
    'vmicguestinterface'   = 'Disabled'
    'vmicheartbeat'        = 'Disabled'
    'vmickvpexchange'      = 'Disabled'
    'vmicrdv'              = 'Disabled'
    'vmicshutdown'         = 'Disabled'
    'vmictimesync'         = 'Disabled'
    'vmicvmsession'        = 'Disabled'
    'vmicvss'              = 'Disabled'
}

# ---- OPTIONAL tier: no gaming impact, but you lose a convenience ----
$Opt = [ordered]@{
    'WSearch'              = 'Disabled'   # Start-menu/Explorer file search becomes slow (unindexed)
    'SysMain'              = 'Manual'     # Superfetch; little benefit on NVMe
    'LanmanServer'         = 'Manual'     # Can't share folders/printers FROM this PC
    'DPS'                  = 'Disabled'   # Troubleshooters / Task Manager App history stop working
    'WdiServiceHost'       = 'Disabled'
    'WdiSystemHost'        = 'Disabled'
    'lmhosts'              = 'Disabled'   # NetBIOS lookups of old LAN devices (DNS still works)
    'PcaSvc'               = 'Disabled'   # Program Compatibility Assistant prompts
    'WSAIFabricSvc'        = 'Manual'     # Windows AI components (Recall/Click to Do)
    'CDPSvc'               = 'Manual'     # Phone Link / Nearby Sharing / cross-device
    'logi_lamparray_service' = 'Manual'   # Logitech RGB via Windows Dynamic Lighting; keep if you use it
    'WbioSrvc'             = 'Disabled'   # Windows Hello fingerprint/face (none detected)
    'SCardSvr'             = 'Disabled'   # Smart cards
    'ScDeviceEnum'         = 'Disabled'
    'SCPolicySvc'          = 'Disabled'
    'CertPropSvc'          = 'Disabled'
    'whesvc'               = 'Manual'     # Windows Health and Optimized Experiences (slowness traces, feedback toasts)
}

function Set-StartType($name, $type) {
    $svc = Get-Service -Name $name -ErrorAction SilentlyContinue
    if (-not $svc) { Write-Host "  skip  $name (not installed)" -ForegroundColor DarkGray; return }
    if ($Protected -contains $name) { Write-Host "  KEEP  $name (gaming dependency)" -ForegroundColor Yellow; return }
    if ($WhatIf) { Write-Host ("  would {0,-8} {1}  (now {2})" -f $type, $name, $svc.StartType); return }
    try {
        Set-Service -Name $name -StartupType $type
        if ($type -eq 'Disabled' -and $svc.Status -eq 'Running') { Stop-Service -Name $name -Force -ErrorAction SilentlyContinue }
        Write-Host ("  set   {0,-8} {1}" -f $type, $name) -ForegroundColor Green
    } catch {
        # Some services are ACL-protected from Set-Service; fall back to the registry
        try {
            $map = @{ Automatic = 2; Manual = 3; Disabled = 4 }
            Set-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Services\$name" -Name Start -Value $map[$type]
            Write-Host ("  set   {0,-8} {1} (via registry)" -f $type, $name) -ForegroundColor Green
        } catch { Write-Host "  FAIL  $name : $($_.Exception.Message)" -ForegroundColor Red }
    }
}

if ($Restore) {
    Import-Csv $Restore | ForEach-Object {
        $t = switch ($_.StartMode) { 'Auto' {'Automatic'} 'Manual' {'Manual'} 'Disabled' {'Disabled'} default {$null} }
        if ($t) { try { Set-Service -Name $_.Name -StartupType $t; Write-Host "  restored $($_.Name) -> $t" } catch { Write-Host "  FAIL $($_.Name)" -ForegroundColor Red } }
    }
    Write-Host 'Restore complete. Reboot.'; return
}

$targets = $Safe
if ($Optional) { foreach ($k in $Opt.Keys) { $targets[$k] = $Opt[$k] } }

if (-not $WhatIf) {
    $backup = Join-Path $PSScriptRoot ("services-backup-{0:yyyyMMdd-HHmmss}.csv" -f (Get-Date))
    Get-CimInstance Win32_Service | Select-Object Name, StartMode | Export-Csv $backup -NoTypeInformation
    Write-Host "Backup written: $backup`n"
}

foreach ($k in $targets.Keys) { Set-StartType $k $targets[$k] }
Write-Host "`nDone. Reboot to apply. Undo with:  .\Optimize-Services.ps1 -Restore <backup.csv>"
