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

  Run from an ELEVATED PowerShell:
      powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1
      powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1 -RemoveOneDrive
      powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1 -Undo
#>
[CmdletBinding()]
param(
    [switch]$RemoveOneDrive,
    [switch]$Undo
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

Write-Host "Scheduled tasks:" -ForegroundColor Cyan
foreach ($p in $Tasks) {
    $folder = (Split-Path $p) + '\'
    $name   = Split-Path $p -Leaf
    $t = Get-ScheduledTask -TaskPath $folder -TaskName $name -ErrorAction SilentlyContinue
    if (-not $t) { Write-Host "  skip      $p (not present)" -ForegroundColor DarkGray; continue }
    try {
        if ($Undo) { Enable-ScheduledTask -InputObject $t | Out-Null; Write-Host "  enabled   $p" -ForegroundColor Green }
        else       { Disable-ScheduledTask -InputObject $t | Out-Null; Write-Host "  disabled  $p" -ForegroundColor Green }
    } catch { Write-Host "  FAIL      $p : $($_.Exception.Message)" -ForegroundColor Red }
}
# Windows "tips" / promotional content tasks (per-user folder names vary)
Get-ScheduledTask -TaskPath '\SoftLanding\*' -ErrorAction SilentlyContinue | ForEach-Object {
    if ($Undo) { Enable-ScheduledTask -InputObject $_ | Out-Null } else { Disable-ScheduledTask -InputObject $_ | Out-Null }
    Write-Host ("  {0,-9} {1}{2}" -f $(if ($Undo) { 'enabled' } else { 'disabled' }), $_.TaskPath, $_.TaskName) -ForegroundColor Green
}

Write-Host "`nEdge background launching:" -ForegroundColor Cyan
$edgeKey = 'HKLM:\SOFTWARE\Policies\Microsoft\Edge'
if ($Undo) {
    foreach ($v in 'StartupBoostEnabled', 'BackgroundModeEnabled') { Remove-ItemProperty -Path $edgeKey -Name $v -ErrorAction SilentlyContinue }
    Write-Host '  Edge policies removed (Edge back to defaults)' -ForegroundColor Green
} else {
    New-Item -Path $edgeKey -Force | Out-Null
    Set-ItemProperty -Path $edgeKey -Name StartupBoostEnabled  -Value 0 -Type DWord
    Set-ItemProperty -Path $edgeKey -Name BackgroundModeEnabled -Value 0 -Type DWord
    Write-Host '  Startup Boost and background mode turned off (Edge shows "Managed by your organization" - expected)' -ForegroundColor Green
}

Write-Host "`nXbox networking (Teredo):" -ForegroundColor Cyan
$v6Key = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\TCPIP\v6Transition'
$TeredoServer = 'win1910.ipv6.microsoft.com'
if ($Undo) {
    foreach ($v in 'Teredo_State', 'Teredo_ServerName') { Remove-ItemProperty -Path $v6Key -Name $v -ErrorAction SilentlyContinue }
    & netsh interface teredo set state type=default | Out-Null
    Write-Host '  Teredo policy removed and Teredo set back to the Windows default' -ForegroundColor Green
} else {
    & netsh interface teredo set state type=enterpriseclient servername=$TeredoServer | Out-Null
    # Same values Group Policy writes for: Network > TCPIP Settings > IPv6 Transition Technologies
    New-Item -Path $v6Key -Force | Out-Null
    Set-ItemProperty -Path $v6Key -Name Teredo_State -Value 'Enterprise Client' -Type String
    Set-ItemProperty -Path $v6Key -Name Teredo_ServerName -Value $TeredoServer -Type String
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

if ($RemoveOneDrive -and -not $Undo) {
    Write-Host "`nOneDrive:" -ForegroundColor Cyan
    Get-Process OneDrive -ErrorAction SilentlyContinue | Stop-Process -Force
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if ($winget) {
        & winget uninstall --id Microsoft.OneDrive --silent --accept-source-agreements
    } else {
        $setup = @("$env:SystemRoot\System32\OneDriveSetup.exe", "$env:SystemRoot\SysWOW64\OneDriveSetup.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($setup) { Start-Process $setup -ArgumentList '/uninstall' -Wait } else { Write-Host '  OneDrive installer not found - uninstall it from Settings > Apps.' -ForegroundColor Yellow }
    }
    # The scheduled tasks are per-user and survive the uninstall on some builds
    Get-ScheduledTask -TaskName 'OneDrive*' -ErrorAction SilentlyContinue | Disable-ScheduledTask | Out-Null
    Write-Host '  OneDrive removed (reinstall any time from the Microsoft Store or winget).' -ForegroundColor Green
}

Write-Host "`nDone. Widgets: Windows blocks scripts from switching it off - use gpedit.msc >" -ForegroundColor Cyan
Write-Host "  Computer Configuration > Administrative Templates > Windows Components > Widgets > 'Allow widgets' = Disabled, then sign out." -ForegroundColor Cyan
