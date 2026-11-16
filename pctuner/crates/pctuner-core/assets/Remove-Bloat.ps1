#Requires -Version 5.1
#Requires -PSEdition Desktop
<#
  Remove-Bloat.ps1  (v2)
  Removes unneeded Windows apps IN DEPTH and stops Windows from putting them back.

  For every app it removes, it:
    1. Deprovisions it (Remove-AppxProvisionedPackage) - this is what stops feature updates and new
       user profiles from reinstalling it (Microsoft documents Windows respecting this since 2024).
    2. Uninstalls it for ALL users.
    3. Writes the AppxAllUserStore\Deprovisioned\<PackageFamilyName> marker (belt and braces).
    4. Deletes the Windows Update "UScheduler_Oobe" auto-install entry for that app, if it has one
       (Outlook, Dev Home, Cross Device, Teams). The EdgeUpdate / IA / LXP entries are never touched.
    5. Cleans leftovers like Revo Uninstaller: per-user data folders, alias stubs, app-specific folders,
       registry entries. Files are MOVED to a quarantine folder (not deleted) and registry keys are
       exported before deletion, so everything can be recovered.
    6. Registers a small SYSTEM task that re-checks at startup and daily, and removes anything a future
       update brings back.

  Never touched: Xbox app, Gaming Services, Game Bar, Xbox sign-in, Halo, Store, App Installer/winget,
  Windows Security, Claude, Edge / WebView2 / Edge Update, Photos, Paint, Snipping Tool, Notepad,
  Calculator, Terminal, Media Player, all codecs, all frameworks and all system packages.

  Run from an ELEVATED Windows PowerShell (not PowerShell 7):
      .\Remove-Bloat.ps1                     # menu
      .\Remove-Bloat.ps1 -WhatIf             # preview the Safe list
      .\Remove-Bloat.ps1 -Apply              # remove the Safe list
      .\Remove-Bloat.ps1 -Apply -Optional    # Safe + Optional
      .\Remove-Bloat.ps1 -Apply -Optional -Skip MSTeams,Microsoft.Todos
      .\Remove-Bloat.ps1 -Restore            # reinstall everything this script removed and undo its changes
      .\Remove-Bloat.ps1 -List               # show both lists with what you would lose
#>
[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$Optional,
    [switch]$WhatIf,
    [switch]$Restore,
    [switch]$List,
    [string[]]$Skip = @(),
    [switch]$NoTask,
    [switch]$Reapply,     # used by the scheduled task (runs as SYSTEM)
    [switch]$Yes          # skip the "type YES" prompts (PC Tuner asks in its own window)
)

$ErrorActionPreference = 'Stop'
# -Skip can arrive as one comma-separated string when called with -File (e.g. from PC Tuner)
$Skip = @($Skip | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Run this from an elevated (Administrator) Windows PowerShell.' -ForegroundColor Red
    exit 1
}
$IsSystem = ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -eq 'S-1-5-18')

# ======================================================================= paths
$DataDir   = Join-Path $env:ProgramData 'Debloat'
$StateFile = Join-Path $DataDir 'state.json'
$LogFile   = Join-Path $DataDir 'debloat.log'
$Stamp     = Get-Date -Format 'yyyyMMdd-HHmmss'
$RunDir    = Join-Path $DataDir "backup\$Stamp"
$QuarDir   = Join-Path $RunDir 'quarantine'
$RegDir    = Join-Path $RunDir 'registry'

$UschedKey   = 'HKLM:\SOFTWARE\Microsoft\WindowsUpdate\Orchestrator\UScheduler_Oobe'
$DeprovKey   = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Appx\AppxAllUserStore\Deprovisioned'
$UninstRoots = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'

# ======================================================================= protected (exact names, never removed)
$Protected = @(
    'Microsoft.GamingApp', 'Microsoft.GamingServices', 'Microsoft.Xbox.TCUI', 'Microsoft.XboxIdentityProvider',
    'Microsoft.XboxGamingOverlay', 'Microsoft.XboxGameCallableUI', 'Microsoft.254428597CFE2',
    'Microsoft.WindowsStore', 'Microsoft.StorePurchaseApp', 'Microsoft.DesktopAppInstaller', 'Microsoft.Winget.Source',
    'Microsoft.SecHealthUI', 'AnthropicPBC.Claude', 'Microsoft.MicrosoftEdge.Stable', 'Microsoft.MicrosoftEdgeDevToolsClient',
    'Microsoft.Windows.Photos', 'Microsoft.Paint', 'Microsoft.ScreenSketch', 'Microsoft.WindowsNotepad',
    'Microsoft.WindowsCalculator', 'Microsoft.WindowsTerminal', 'Microsoft.ZuneMusic', 'Microsoft.ApplicationCompatibilityEnhancements',
    'Microsoft.AV1VideoExtension', 'Microsoft.AVCEncoderVideoExtension', 'Microsoft.HEIFImageExtension', 'Microsoft.HEVCVideoExtension',
    'Microsoft.MPEG2VideoExtension', 'Microsoft.RawImageExtension', 'Microsoft.VP9VideoExtensions', 'Microsoft.WebMediaExtensions',
    'Microsoft.WebpImageExtension', 'MicrosoftWindows.Client.CBS', 'MicrosoftWindows.Client.Core', 'Microsoft.Windows.ContentDeliveryManager'
)
# Uninstall entries that must never be matched by a Win32 step
$ProtectedWin32 = 'Microsoft Edge', 'Microsoft Edge WebView2 Runtime', 'Microsoft EdgeWebView', 'Microsoft Edge Update', 'Xbox Service', 'Microsoft GameInput', 'Steam'

# ======================================================================= targets
# Kind: appx | capability. Hook: extra cleanup by name (see Invoke-Hook).
$Targets = @(
    # ---------------------------------------------------------------- SAFE
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.MicrosoftOfficeHub'; Store = '9NHT9RB2F4HD'; Hook = 'Copilot'
       Loses = 'Copilot app (desktop + its app identity) and the Copilot key action. You use Claude.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Clipchamp.Clipchamp'; Store = '9P1J8S7CCWWT'; Loses = 'Clipchamp video editor.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.BingNews'; Store = '9WZDNCRFHVFW'; Loses = 'Microsoft News app.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.BingWeather'; Store = '9WZDNCRFJ3Q2'; Loses = 'Weather app.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.BingSearch'; Store = ''; Loses = 'Bing web results in Start search (local search still works).' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.MicrosoftSolitaireCollection'; Store = '9WZDNCRFHWD2'; Loses = 'Solitaire.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.PowerAutomateDesktop'; Store = '9NFTCH6J7FHV'; Loses = 'Power Automate.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.Windows.DevHome'; Store = ''; Hook = 'DevHome'; Loses = 'Nothing (Dev Home is retired; this is a stub).' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.WindowsCamera'; Store = '9WZDNCRFJBBG'; Loses = 'Camera app (no camera on this PC).' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.WindowsFeedbackHub'; Store = '9NBLGGH4R32N'; Hook = 'Feedback'; Loses = 'Feedback Hub.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.StartExperiencesApp'; Store = ''; Loses = 'MSN news/weather widgets.' }
    @{ Tier = 'Safe'; Kind = 'appx'; Name = 'Microsoft.Ink.Handwriting.Main.en-US.1.0.1'; Store = ''; Hook = 'Ink'; Loses = 'Pen handwriting recognition (no pen on this PC).' }
    @{ Tier = 'Safe'; Kind = 'capability'; Name = 'App.StepsRecorder~'; Loses = 'Steps Recorder (deprecated).' }
    @{ Tier = 'Safe'; Kind = 'capability'; Name = 'Hello.Face.'; Loses = 'Windows Hello face sign-in (no IR camera). PIN/password unaffected.' }
    @{ Tier = 'Safe'; Kind = 'capability'; Name = 'Print.Management.Console~'; Loses = 'Print Management console (no printer).' }
    @{ Tier = 'Safe'; Kind = 'capability'; Name = 'Language.Handwriting~'; Loses = 'Pen handwriting language data.' }
    @{ Tier = 'Safe'; Kind = 'system'; Name = 'Recall'; Hook = 'Recall'; Loses = 'Nothing (Recall is turned off by policy).' }
    @{ Tier = 'Safe'; Kind = 'system'; Name = 'OneDrive leftovers'; Hook = 'OneDrive'; Loses = 'Nothing (OneDrive is already uninstalled; stops it returning for new profiles).' }
    # ---------------------------------------------------------------- OPTIONAL
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'MicrosoftWindows.Client.WebExperience'; Store = '9MSSGKG348SP'
       Loses = 'Widgets board and lock-screen widgets. Also the way around the UCPD block on the Widgets policy.' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.WidgetsPlatformRuntime'; Store = ''; Loses = 'Third-party widgets (goes with Widgets).' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'MSTeams'; Store = 'XP8BT8DW290MPQ'; Hook = 'Teams'; Loses = 'Microsoft Teams (+ its Office meeting add-in).' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.OutlookForWindows'; Store = '9NRX63209R7B'; Hook = 'Outlook'; Loses = 'New Outlook mail/calendar (mailto: links need a new default).' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.YourPhone'; Store = '9NMPJ99VJBWV'; Hook = 'PhoneLink'; Loses = 'Phone Link.' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'MicrosoftWindows.CrossDevice'; Store = ''; Hook = 'CrossDevice'; Loses = 'Mobile devices page, phone as webcam, phone in Explorer.' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.Todos'; Store = '9NBLGGH5R558'; Loses = 'Microsoft To Do.' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.MicrosoftStickyNotes'; Store = '9NBLGGH4QGHW'; Loses = 'Sticky Notes (notes are backed up to quarantine).' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.WindowsAlarms'; Store = '9WZDNCRFJ3PR'; Loses = 'Clock: alarms, timers, stopwatch.' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.WindowsSoundRecorder'; Store = '9WZDNCRFHWKN'; Loses = 'Sound Recorder (your recordings in Documents stay).' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'MicrosoftCorporationII.QuickAssist'; Store = '9P7BP5VNWKX5'; Loses = 'Quick Assist remote help.' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.GetHelp'; Store = '9PKDZBMV1H3T'; Loses = 'Get Help - also the Settings > Troubleshoot runners.' }
    @{ Tier = 'Optional'; Kind = 'appx'; Name = 'Microsoft.XboxSpeechToTextOverlay'; Store = ''; Loses = 'Game Bar party-chat speech-to-text accessibility overlay.' }
    @{ Tier = 'Optional'; Kind = 'capability'; Name = 'Media.WindowsMediaPlayer~'; Loses = 'Windows Media Player Legacy (the new Media Player stays).' }
    @{ Tier = 'Optional'; Kind = 'capability'; Name = 'Browser.InternetExplorer~'; Loses = 'Old IE engine for apps that call iexplore.exe.' }
    @{ Tier = 'Optional'; Kind = 'capability'; Name = 'MathRecognizer~'; Loses = 'Ink math input.' }
    @{ Tier = 'Optional'; Kind = 'capability'; Name = 'Microsoft.Windows.PowerShell.ISE~'; Loses = 'PowerShell ISE editor (PowerShell itself stays).' }
    @{ Tier = 'Optional'; Kind = 'capability'; Name = 'Language.Speech~'; Loses = 'Voice typing, Voice Access, Game Bar speech-to-text.' }
    @{ Tier = 'Optional'; Kind = 'capability'; Name = 'Language.OCR~'; Loses = 'Copy text from images (Snipping Tool text actions).' }
)

# ======================================================================= helpers
$script:LogCount = 0
function Write-Log([string]$msg, [string]$color = 'Gray') {
    $script:LogCount++
    $line = "{0}  {1}" -f (Get-Date -Format 'HH:mm:ss'), $msg
    if (-not $IsSystem) { Write-Host $line -ForegroundColor $color }
    try { Add-Content -LiteralPath $LogFile -Value $line -Encoding UTF8 } catch { }
}

function Initialize-DataDir {
    if (-not (Test-Path $DataDir)) { New-Item -ItemType Directory -Path $DataDir | Out-Null }
    # Hardened ACL: a SYSTEM task runs a script from here, so ordinary users must not be able to write it.
    # icacls goes through cmd with all output discarded: its warnings (e.g. very long paths of quarantined
    # Outlook cache files) are harmless and would otherwise clutter the log.
    $q = '"' + $DataDir + '"'
    & cmd.exe /c "icacls $q /setowner *S-1-5-32-544 /T /C /Q >nul 2>&1"
    # Set the folder ACL once (folder only), then make everything inside inherit it.
    # (Granting (OI)(CI) flags with /T directly on files leaves them unreadable for Users.)
    & cmd.exe /c "icacls $q /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX /C /Q >nul 2>&1"
    if (Get-ChildItem -LiteralPath $DataDir -Force -ErrorAction SilentlyContinue) {
        $all = '"' + (Join-Path $DataDir '*') + '"'
        & cmd.exe /c "icacls $all /reset /T /C /Q >nul 2>&1"
    }
}

function Load-State {
    if (Test-Path $StateFile) {
        try {
            $s = ConvertFrom-Json (Get-Content -LiteralPath $StateFile -Raw)
            return [pscustomobject]@{
                Removed  = [System.Collections.ArrayList]@(@($s.Removed) | Where-Object { $_ })
                Policies = [System.Collections.ArrayList]@(@($s.Policies) | Where-Object { $_ })
                Usched   = [System.Collections.ArrayList]@(@($s.Usched) | Where-Object { $_ })
                Backups  = [System.Collections.ArrayList]@(@($s.Backups) | Where-Object { $_ })
                Tasks      = [System.Collections.ArrayList]@(@($s.Tasks) | Where-Object { $_ })
                Features   = [System.Collections.ArrayList]@(@($s.Features) | Where-Object { $_ })
                Frameworks = [System.Collections.ArrayList]@(@($s.Frameworks) | Where-Object { $_ })
            }
        } catch { Write-Log "State file unreadable: $($_.Exception.Message)" 'Yellow' }
    }
    [pscustomobject]@{ Removed = New-Object System.Collections.ArrayList; Policies = New-Object System.Collections.ArrayList
                       Usched = New-Object System.Collections.ArrayList; Backups = New-Object System.Collections.ArrayList
                       Tasks = New-Object System.Collections.ArrayList; Features = New-Object System.Collections.ArrayList
                       Frameworks = New-Object System.Collections.ArrayList }
}
function Save-State { ConvertTo-Json -InputObject $script:State -Depth 6 | Set-Content -LiteralPath $StateFile -Encoding UTF8 }

# Export a registry key before changing it. Returns $true if exported, $false if the key does not exist.
function Backup-RegKey([string]$regPath, [string]$label) {
    $ps = 'Registry::' + $regPath
    if (-not (Test-Path -LiteralPath $ps)) { return $false }
    if (-not (Test-Path $RegDir)) { New-Item -ItemType Directory -Path $RegDir -Force | Out-Null }
    $file = Join-Path $RegDir (($label -replace '[^\w\.-]', '_') + '.reg')
    & reg.exe export $regPath $file /y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Could not back up $regPath - not changing it." }
    [void]$script:State.Backups.Add([pscustomobject]@{ Time = $Stamp; Key = $regPath; File = $file })
    return $true
}

function Remove-RegKeySafe([string]$regPath, [string]$label) {
    if ($WhatIf) { if (Test-Path -LiteralPath ('Registry::' + $regPath)) { Write-Log "  would back up + delete $regPath" } ; return }
    if (Backup-RegKey $regPath $label) {
        Remove-Item -LiteralPath ('Registry::' + $regPath) -Recurse -Force
        Write-Log "  deleted $regPath (backed up)" 'DarkGray'
    }
}

# Set a policy/registry value, remembering what was there so -Restore can put it back.
function Set-TrackedValue([string]$path, [string]$name, $value, [string]$type = 'DWord') {
    if ($IsSystem -and $path -like 'HKCU:*') { return }
    $k = Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
    $prev = if ($k) { $k.GetValue($name, $null) } else { $null }
    if ($null -ne $prev -and "$prev" -eq "$value") { return }
    if ($WhatIf) { Write-Log "  would set $path\$name = $value"; return }
    if (-not $k) { New-Item -Path $path -Force | Out-Null }
    New-ItemProperty -LiteralPath $path -Name $name -Value $value -PropertyType $type -Force | Out-Null
    if (-not ($script:State.Policies | Where-Object { $_.Path -eq $path -and $_.Name -eq $name })) {
        [void]$script:State.Policies.Add([pscustomobject]@{ Path = $path; Name = $name; Existed = ($null -ne $prev); Prev = $prev; Type = $type })
    }
    Write-Log "  set $path\$name = $value" 'DarkGray'
}

# Move a leftover file/folder into the quarantine instead of deleting it.
$NeverQuarantine = @(
    $env:LOCALAPPDATA, "$env:LOCALAPPDATA\Packages", "$env:LOCALAPPDATA\Microsoft", "$env:LOCALAPPDATA\Microsoft\WindowsApps",
    $env:APPDATA, "$env:APPDATA\Microsoft", $env:ProgramData, "$env:ProgramData\Microsoft", ${env:ProgramFiles}, ${env:ProgramFiles(x86)},
    "${env:ProgramFiles(x86)}\Microsoft", "$env:ProgramFiles\WindowsApps", $env:SystemRoot, 'C:\Users'
) | ForEach-Object { $_.TrimEnd('\').ToLowerInvariant() }
function Move-ToQuarantine([string]$path) {
    if (-not $path -or -not (Test-Path -LiteralPath $path)) { return }
    $full = (Resolve-Path -LiteralPath $path).ProviderPath.TrimEnd('\')
    $low = $full.ToLowerInvariant()
    $isProtectedRoot = $NeverQuarantine -contains $low
    $inProgramWindowsApps = $low.StartsWith(("$env:ProgramFiles\WindowsApps").ToLowerInvariant())
    $inWindowsDir = $low.StartsWith($env:SystemRoot.ToLowerInvariant() + '\')
    if ($isProtectedRoot -or $inProgramWindowsApps -or $inWindowsDir) {
        Write-Log "  refused to touch protected path $full" 'Yellow'; return
    }
    if ($WhatIf) { Write-Log "  would quarantine $full"; return }
    $dest = Join-Path $QuarDir ($full -replace ':', '' -replace '^\\+', '')
    New-Item -ItemType Directory -Path (Split-Path $dest) -Force | Out-Null
    try { Move-Item -LiteralPath $full -Destination $dest -Force; Write-Log "  quarantined $full" 'DarkGray' }
    catch { Write-Log "  could not move $full (in use?): $($_.Exception.Message)" 'Yellow' }
}

function Get-ProfileDirs {
    Get-ChildItem 'C:\Users' -Directory -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notin 'Public', 'All Users', 'Default User', 'defaultuser0' -and -not ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) } |
        ForEach-Object { $_.FullName }
}

function Get-UninstallEntry([string]$displayName) {
    foreach ($r in $UninstRoots) {
        Get-ChildItem -LiteralPath $r -ErrorAction SilentlyContinue | ForEach-Object {
            $p = Get-ItemProperty -LiteralPath $_.PSPath -ErrorAction SilentlyContinue
            if ($p.DisplayName -eq $displayName) { [pscustomobject]@{ Key = $_.Name; Guid = $_.PSChildName; DisplayName = $p.DisplayName; Uninstall = $p.UninstallString } }
        }
    }
}

function Remove-UschedEntry([string]$entry) {
    $path = "HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\WindowsUpdate\Orchestrator\UScheduler_Oobe\$entry"
    if (Test-Path -LiteralPath "Registry::$path") {
        Remove-RegKeySafe $path "UScheduler_Oobe-$entry"
    }
    if (-not $WhatIf -and -not ($script:State.Usched -contains $entry)) { [void]$script:State.Usched.Add($entry) }
}

# ======================================================================= app-specific deep clean
function Invoke-Hook([string]$hook, [string]$phase, $t) {
    switch ("$hook/$phase") {
        'Copilot/before' {
            # The desktop Copilot and OfficeHub are one product: uninstall the desktop part first.
            $e = Get-UninstallEntry 'Copilot' | Select-Object -First 1
            if ($e -and $e.Uninstall -match '^"([^"]+copilot_setup\.exe)"\s*(.*)$') {
                $exe = $Matches[1]; $argList = "$($Matches[2]) --force-uninstall"
                if ($WhatIf) { Write-Log "  would run Copilot uninstaller ($exe)"; break }
                Write-Log '  uninstalling desktop Copilot...'
                $p = Start-Process -FilePath $exe -ArgumentList $argList -Wait -PassThru -WindowStyle Hidden
                if (Get-UninstallEntry 'Copilot') { throw "Copilot uninstaller exited $($p.ExitCode) and Copilot is still installed - leaving OfficeHub in place." }
                Move-ToQuarantine "${env:ProgramFiles(x86)}\Microsoft\Copilot"
                [void]$script:State.Removed.Add([pscustomobject]@{ Name = 'Copilot (desktop)'; Kind = 'win32'; Pfn = ''; Store = '9NHT9RB2F4HD'; Time = $Stamp; Restored = $false })
            }
        }
        'Copilot/after' {
            Set-TrackedValue 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsCopilot' 'TurnOffWindowsCopilot' 1
            Set-TrackedValue 'HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot' 'TurnOffWindowsCopilot' 1
            # Edge Update per-app policy for Copilot ONLY (never Edge/WebView2). Likely ignored on a non-managed PC; harmless.
            Set-TrackedValue 'HKLM:\SOFTWARE\Policies\Microsoft\EdgeUpdate' 'Install{C50565E9-CCCF-44B4-BA15-5AC5C6569197}' 0
        }
        'DevHome/after'  { Remove-UschedEntry 'DevHomeUpdate' }
        'Outlook/after'  {
            Remove-UschedEntry 'OutlookUpdate'
            Set-TrackedValue 'HKLM:\SOFTWARE\Microsoft\WindowsUpdate\Orchestrator\UScheduler_Oobe' 'BlockedOobeUpdaters' '["MS_Outlook"]' 'String'
            if (-not $IsSystem) { Move-ToQuarantine "$env:LOCALAPPDATA\Microsoft\Olk" }
        }
        'CrossDevice/after' { Remove-UschedEntry 'CrossDeviceUpdate' }
        'PhoneLink/after'   { Set-TrackedValue 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System' 'EnableMmx' 0 }
        'Feedback/after' {
            $task = Get-ScheduledTask -TaskPath '\Microsoft\Windows\PerformanceTrace\' -TaskName 'ShowFeedbackToast' -ErrorAction SilentlyContinue
            if ($task -and $task.State -ne 'Disabled') { if ($WhatIf) { Write-Log '  would disable ShowFeedbackToast task' } else { Disable-ScheduledTask -InputObject $task | Out-Null; Write-Log '  disabled ShowFeedbackToast task' 'DarkGray'
                if (-not ($script:State.Tasks | Where-Object { $_.Name -eq 'ShowFeedbackToast' })) { [void]$script:State.Tasks.Add([pscustomobject]@{ Path = '\Microsoft\Windows\PerformanceTrace\'; Name = 'ShowFeedbackToast' }) } } }
        }
        'Ink/after' {
            # The handwriting framework has no other users once Main is gone.
            foreach ($fw in @(Get-AppxPackage -AllUsers -Name 'Microsoft.Ink.Handwriting.en-US.1.0' -ErrorAction SilentlyContinue)) {
                $users = Get-AppxPackage -AllUsers | Where-Object { $_.Dependencies.Name -contains $fw.Name -and $_.Name -ne $fw.Name }
                if ($users) { Write-Log "  keeping $($fw.Name): still used by $($users.Name -join ', ')"; continue }
                if ($WhatIf) { Write-Log "  would remove framework $($fw.PackageFullName)"; continue }
                try {
                    Remove-AppxPackage -Package $fw.PackageFullName -AllUsers; Write-Log "  removed framework $($fw.PackageFullName)" 'DarkGray'
                    if (-not ($script:State.Frameworks | Where-Object { $_.Pfn -eq $fw.PackageFamilyName })) { [void]$script:State.Frameworks.Add([pscustomobject]@{ Name = $fw.Name; Pfn = $fw.PackageFamilyName; Time = $Stamp }) }
                } catch { Write-Log "  framework not removed: $($_.Exception.Message)" 'Yellow' }
            }
        }
        'Teams/after' {
            Remove-UschedEntry 'TFLUpdate'
            $addin = Get-UninstallEntry 'Microsoft Teams Meeting Add-in for Microsoft Office' | Select-Object -First 1
            if ($addin -and $addin.Guid -match '^\{[0-9A-Fa-f-]+\}$') {
                if ($WhatIf) { Write-Log "  would uninstall Teams Meeting Add-in ($($addin.Guid))" }
                else {
                    $p = Start-Process msiexec.exe -ArgumentList "/x $($addin.Guid) /qn /norestart" -Wait -PassThru
                    Write-Log "  Teams Meeting Add-in uninstall exit code $($p.ExitCode)" 'DarkGray'
                    if ($p.ExitCode -in 0, 3010) { [void]$script:State.Removed.Add([pscustomobject]@{ Name = 'Teams Meeting Add-in'; Kind = 'win32'; Pfn = ''; Store = ''; Time = $Stamp; Restored = $false }) }
                }
            }
            if (-not $IsSystem) {
                Move-ToQuarantine "$env:LOCALAPPDATA\Microsoft\TeamsMeetingAdd-in"
                Move-ToQuarantine "$env:LOCALAPPDATA\Microsoft\TeamsMeetingAddin"
                Remove-RegKeySafe 'HKEY_CURRENT_USER\Software\Microsoft\Office\Outlook\Addins\TeamsAddin.FastConnect' 'HKCU-TeamsAddin'
                foreach ($proto in 'ms-teams', 'msteams') {
                    $cmd = (Get-ItemProperty -LiteralPath "HKCU:\Software\Classes\$proto\shell\open\command" -ErrorAction SilentlyContinue).'(default)'
                    if ($cmd -match 'ms-teams\.exe' -and -not (Get-Command ms-teams.exe -ErrorAction SilentlyContinue)) { Remove-RegKeySafe "HKEY_CURRENT_USER\Software\Classes\$proto" "HKCU-Classes-$proto" }
                }
            }
        }
        'Recall/run' {
            Set-TrackedValue 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsAI' 'AllowRecallEnablement' 0
            Set-TrackedValue 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsAI' 'DisableAIDataAnalysis' 1
            Set-TrackedValue 'HKCU:\Software\Policies\Microsoft\Windows\WindowsAI' 'DisableAIDataAnalysis' 1
            try {
                $f = Get-WindowsOptionalFeature -Online -FeatureName 'Recall' -ErrorAction Stop
                if ($f.State -eq 'Enabled') {
                    if ($WhatIf) { Write-Log '  would disable optional feature Recall' }
                    else {
                        Disable-WindowsOptionalFeature -Online -FeatureName 'Recall' -NoRestart | Out-Null; Write-Log '  disabled optional feature Recall' 'DarkGray'
                        if (-not ($script:State.Features -contains 'Recall')) { [void]$script:State.Features.Add('Recall') }
                    }
                }
            } catch { }
        }
        'OneDrive/run' {
            Set-TrackedValue 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OneDrive' 'DisableFileSyncNGSC' 1
            if (-not $IsSystem) { Move-ToQuarantine "$env:ProgramData\Microsoft OneDrive" }
            # New user profiles get OneDrive from a Run entry in the Default user hive.
            $hive = 'C:\Users\Default\NTUSER.DAT'
            # A preview never mounts the Default user hive (a failed unload would leave it locked).
            if ((Test-Path $hive) -and -not $IsSystem -and $WhatIf) {
                Write-Log '  would remove OneDriveSetup from the Default user Run key if it is there, backed up (a preview does not open that profile)'
            } elseif ((Test-Path $hive) -and -not $IsSystem) {
                $loaded = $false
                try {
                    & reg.exe load 'HKU\DebloatDefault' $hive 2>$null | Out-Null
                    $loaded = ($LASTEXITCODE -eq 0)
                    if ($loaded) {
                        $run = Get-ItemProperty -LiteralPath 'Registry::HKEY_USERS\DebloatDefault\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue
                        if ($run -and $run.OneDriveSetup) {
                            [void](Backup-RegKey 'HKEY_USERS\DebloatDefault\Software\Microsoft\Windows\CurrentVersion\Run' 'DefaultUser-Run')
                            Remove-ItemProperty -LiteralPath 'Registry::HKEY_USERS\DebloatDefault\Software\Microsoft\Windows\CurrentVersion\Run' -Name OneDriveSetup
                            Write-Log '  removed OneDriveSetup from the Default user Run key (backed up)' 'DarkGray'
                        }
                        $run = $null
                    }
                } finally {
                    if ($loaded) { [gc]::Collect(); [gc]::WaitForPendingFinalizers(); Start-Sleep -Milliseconds 500; & reg.exe unload 'HKU\DebloatDefault' | Out-Null }
                }
            }
        }
        default { }
    }
}

# ======================================================================= removal
function Remove-AppxTarget($t) {
    $name = $t.Name
    if ($Protected -contains $name) { Write-Log "  KEEP $name (protected)" 'Yellow'; return }
    $installed = @(Get-AppxPackage -AllUsers -Name $name -ErrorAction SilentlyContinue)
    $prov = @(Get-Provisioned | Where-Object DisplayName -eq $name)
    foreach ($p in $installed) { if ($p.NonRemovable -or $p.SignatureKind -eq 'System' -or $p.IsFramework) { Write-Log "  KEEP $name (Windows system/framework package)" 'Yellow'; return } }
    # PackageFamilyName = <Name>_<PublisherId>; a provisioned PackageName is <Name>_<Version>_<Arch>_<Resource>_<PublisherId>
    $pfn = if ($installed) { $installed[0].PackageFamilyName } elseif ($prov) { $parts = $prov[0].PackageName -split '_'; "$($parts[0])_$($parts[-1])" } else { $null }
    if (-not $installed -and -not $prov) {
        if (-not $Reapply) { Write-Log "  not installed: $name" 'DarkGray' }
        return
    }
    Write-Log "Removing $name  -  $($t.Loses)" 'Cyan'
    if ($t.Hook) { Invoke-Hook $t.Hook 'before' $t }
    if ($WhatIf) {
        Write-Log "  would deprovision ($($prov.Count)) and uninstall for all users ($($installed.Count))"
    } else {
        foreach ($p in $prov) { try { Remove-AppxProvisionedPackage -Online -PackageName $p.PackageName -AllUsers | Out-Null; Write-Log "  deprovisioned $($p.PackageName)" 'DarkGray' } catch { Write-Log "  deprovision failed: $($_.Exception.Message)" 'Yellow' } }
        foreach ($p in $installed) { try { Remove-AppxPackage -Package $p.PackageFullName -AllUsers; Write-Log "  uninstalled $($p.PackageFullName)" 'DarkGray' } catch { Write-Log "  uninstall failed: $($_.Exception.Message)" 'Red' } }
        if ($pfn) { New-Item -Path (Join-Path $DeprovKey $pfn) -Force | Out-Null }
    }
    # Revo-style leftovers: per-user app data and alias folders for every profile (package is gone for all users).
    if ($pfn -and -not $IsSystem) {
        foreach ($prof in Get-ProfileDirs) {
            Move-ToQuarantine (Join-Path $prof "AppData\Local\Packages\$pfn")
            Move-ToQuarantine (Join-Path $prof "AppData\Local\Microsoft\WindowsApps\$pfn")
        }
    }
    if ($t.Hook) { Invoke-Hook $t.Hook 'after' $t }
    if (-not $WhatIf) {
        $still = @(Get-AppxPackage -AllUsers -Name $name -ErrorAction SilentlyContinue)
        $ok = ($still.Count -eq 0)
        Write-Log $(if ($ok) { "  done: $name removed" } else { "  WARNING: $name still registered for some user" }) $(if ($ok) { 'Green' } else { 'Yellow' })
        # removing an app again that a Restore only unblocked blocks it again
        $open = @($script:State.Removed | Where-Object { $_.Name -eq $name -and -not $_.Restored })
        foreach ($o in $open) { $o | Add-Member -NotePropertyName Unblocked -NotePropertyValue $false -Force }
        if (-not $open) {
            [void]$script:State.Removed.Add([pscustomobject]@{ Name = $name; Kind = 'appx'; Pfn = $pfn; Store = $t.Store; Time = $Stamp; Restored = $false })
        }
    }
}

$script:CapCache = $null
$script:ProvCache = $null
function Get-Provisioned {
    if ($null -eq $script:ProvCache) { $script:ProvCache = @(Get-AppxProvisionedPackage -Online) }
    return $script:ProvCache
}
function Remove-CapabilityTarget($t) {
    try {
        if ($null -eq $script:CapCache) { Write-Log 'Reading Windows optional capabilities (takes a moment)...' 'DarkGray'; $script:CapCache = @(Get-WindowsCapability -Online -ErrorAction Stop) }
        $caps = @($script:CapCache | Where-Object { $_.Name -like "$($t.Name)*" -and $_.State -eq 'Installed' })
    }
    catch { Write-Log "  ERROR on $($t.Name): cannot query capabilities: $($_.Exception.Message)" 'Red'; return }
    if (-not $caps) { if (-not $Reapply) { Write-Log "  not installed: $($t.Name)" 'DarkGray' }; return }
    foreach ($c in $caps) {
        Write-Log "Removing capability $($c.Name)  -  $($t.Loses)" 'Cyan'
        if ($WhatIf) { Write-Log '  would remove'; continue }
        try {
            Remove-WindowsCapability -Online -Name $c.Name | Out-Null
            Write-Log "  done: $($c.Name)" 'Green'
            [void]$script:State.Removed.Add([pscustomobject]@{ Name = $c.Name; Kind = 'capability'; Pfn = ''; Store = ''; Time = $Stamp; Restored = $false })
        } catch { Write-Log "  failed: $($_.Exception.Message)" 'Red' }
    }
}

function Register-ReapplyTask {
    if ($WhatIf -or $NoTask) { if ($WhatIf) { Write-Log 'Would register the re-check task \Debloat\ReapplyDebloat' }; return }
    $copy = Join-Path $DataDir 'Remove-Bloat.ps1'
    Copy-Item -LiteralPath $PSCommandPath -Destination $copy -Force
    Initialize-DataDir
    $a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$copy`" -Reapply"
    $tr = @((New-ScheduledTaskTrigger -AtStartup), (New-ScheduledTaskTrigger -Daily -At 3am -RandomDelay (New-TimeSpan -Hours 2)))
    $set = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Minutes 30) -AllowStartIfOnBatteries
    $pr = New-ScheduledTaskPrincipal -UserId 'S-1-5-18' -RunLevel Highest
    Register-ScheduledTask -TaskName 'ReapplyDebloat' -TaskPath '\Debloat\' -Action $a -Trigger $tr -Settings $set -Principal $pr -Force | Out-Null
    Write-Log 'Registered re-check task \Debloat\ReapplyDebloat (startup + daily) - removes anything updates bring back.' 'Green'
}

# ======================================================================= restore
function Invoke-Restore {
    $winget = Get-Command winget.exe -ErrorAction SilentlyContinue
    foreach ($r in @($script:State.Removed | Where-Object { -not $_.Restored })) {
        # Already settled: reinstalled by hand since, or unblocked by an earlier Restore (reported then, not again).
        if ($r.Kind -eq 'appx' -and @(Get-AppxPackage -AllUsers -Name $r.Name -ErrorAction SilentlyContinue).Count) {
            if ($r.Pfn) { Remove-Item -LiteralPath (Join-Path $DeprovKey $r.Pfn) -Force -ErrorAction SilentlyContinue }
            $r.Restored = $true; Write-Log "$($r.Name): installed again - nothing to restore" 'DarkGray'; continue
        }
        if ($r.Unblocked) { Write-Log "$($r.Name): still unblocked from an earlier Restore, reinstall it by hand" 'DarkGray'; continue }
        switch ($r.Kind) {
            'appx' {
                if ($r.Pfn) { Remove-Item -LiteralPath (Join-Path $DeprovKey $r.Pfn) -Force -ErrorAction SilentlyContinue }
                if ($r.Store -and $winget) {
                    Write-Log "Reinstalling $($r.Name) from the Store ($($r.Store))..." 'Cyan'
                    & winget.exe install --source msstore --id $r.Store --accept-package-agreements --accept-source-agreements --silent | Out-Null
                    if ($LASTEXITCODE -eq 0) { $r.Restored = $true; Write-Log '  reinstalled' 'Green' } else { Write-Log "  winget exit code $LASTEXITCODE - reinstall it from the Store by name" 'Yellow' }
                } else {
                    # Unblocked (no Deprovisioned marker, the re-check task leaves it alone) but NOT reinstalled.
                    $r | Add-Member -NotePropertyName Unblocked -NotePropertyValue $true -Force
                    $why = if ($r.Store) { 'winget not found' } else { 'no Store id' }
                    Write-Log "NOT restored: $($r.Name) - $why; unblocked, reinstall it from the Microsoft Store by name" 'Yellow'
                }
            }
            'capability' {
                Write-Log "Re-adding capability $($r.Name) (needs internet)..." 'Cyan'
                try { Add-WindowsCapability -Online -Name $r.Name | Out-Null; $r.Restored = $true; Write-Log '  done' 'Green' } catch { Write-Log "  failed: $($_.Exception.Message)" 'Yellow' }
            }
            'win32' {
                if ($r.Store -and $winget) {
                    Write-Log "Reinstalling $($r.Name) from the Store ($($r.Store))..." 'Cyan'
                    & winget.exe install --source msstore --id $r.Store --accept-package-agreements --accept-source-agreements --silent | Out-Null
                    if ($LASTEXITCODE -eq 0) { $r.Restored = $true; Write-Log '  reinstalled' 'Green' } else { Write-Log "  winget exit code $LASTEXITCODE - reinstall it from the Store by name" 'Yellow' }
                } else {
                    $r | Add-Member -NotePropertyName Unblocked -NotePropertyValue $true -Force
                    # nothing blocks a win32 app without a Store id (Teams add-in): reported once, then settled
                    if (-not $r.Store) { $r.Restored = $true }
                    Write-Log "NOT restored: $($r.Name) - reinstall it by hand (the Teams add-in comes back with Teams)" 'Yellow'
                }
            }
        }
    }
    foreach ($tk in @($script:State.Tasks)) {
        Write-Log "Re-enabling task $($tk.Name)..." 'Cyan'
        try { Enable-ScheduledTask -TaskPath $tk.Path -TaskName $tk.Name -ErrorAction Stop | Out-Null; [void]$script:State.Tasks.Remove($tk); Write-Log '  done' 'Green' }
        catch { Write-Log "  failed: $($_.Exception.Message)" 'Yellow' }
    }
    foreach ($fe in @($script:State.Features)) {
        Write-Log "Re-enabling optional feature $fe..." 'Cyan'
        try { Enable-WindowsOptionalFeature -Online -FeatureName $fe -NoRestart -ErrorAction Stop | Out-Null; [void]$script:State.Features.Remove($fe); Write-Log '  done' 'Green' }
        catch { Write-Log "  failed: $($_.Exception.Message)" 'Yellow' }
    }
    foreach ($fw in @($script:State.Frameworks)) {
        if (Get-AppxPackage -AllUsers -Name $fw.Name -ErrorAction SilentlyContinue) { [void]$script:State.Frameworks.Remove($fw); continue }
        Write-Log "Re-registering framework $($fw.Name)..." 'Cyan'
        try { Add-AppxPackage -RegisterByFamilyName -MainPackage $fw.Pfn -ErrorAction Stop; [void]$script:State.Frameworks.Remove($fw); Write-Log '  done' 'Green' }
        catch { Write-Log "  failed: $($_.Exception.Message) - it comes back with an app that needs it" 'Yellow' }
    }
    foreach ($p in @($script:State.Policies)) {
        try {
            if ($p.Existed) { New-ItemProperty -LiteralPath $p.Path -Name $p.Name -Value $p.Prev -PropertyType $p.Type -Force | Out-Null }
            else { Remove-ItemProperty -LiteralPath $p.Path -Name $p.Name -ErrorAction SilentlyContinue }
            Write-Log "  restored $($p.Path)\$($p.Name)" 'DarkGray'
        } catch { }
    }
    $script:State.Policies.Clear()
    # Registry exports. The Default user Run key lives in a hive that is mounted only while it is needed.
    # reg.exe runs through cmd so its stderr (it reports success there too) cannot trip ErrorActionPreference=Stop.
    $keep = New-Object System.Collections.ArrayList
    $hive = 'C:\Users\Default\NTUSER.DAT'; $hiveTried = $false; $hiveLoaded = $false
    try {
        foreach ($b in @($script:State.Backups | Sort-Object Time -Descending)) {
            if (-not (Test-Path -LiteralPath $b.File)) { Write-Log "  backup file missing for $($b.Key): $($b.File)" 'Yellow'; continue }
            if ($b.Key -like 'HKEY_USERS\DebloatDefault*' -and -not $hiveTried) {
                $hiveTried = $true
                $out = & cmd.exe /c "reg.exe load HKU\DebloatDefault `"$hive`" 2>&1"
                $hiveLoaded = ($LASTEXITCODE -eq 0)
                if (-not $hiveLoaded) { Write-Log "  could not load the Default user hive (reg.exe exit $LASTEXITCODE): $(($out | Out-String).Trim())" 'Yellow' }
            }
            $out = & cmd.exe /c "reg.exe import `"$($b.File)`" 2>&1"
            if ($LASTEXITCODE -eq 0) { Write-Log "  re-imported $($b.Key)" 'DarkGray' }
            else { Write-Log "  could not re-import $($b.Key) (reg.exe exit $LASTEXITCODE): $(($out | Out-String).Trim())" 'Yellow'; [void]$keep.Add($b) }
        }
    } finally {
        if ($hiveLoaded) { [gc]::Collect(); [gc]::WaitForPendingFinalizers(); Start-Sleep -Milliseconds 500; & cmd.exe /c "reg.exe unload HKU\DebloatDefault >nul 2>&1" }
    }
    # failed imports stay recorded so a later Restore can retry them
    $script:State.Backups = $keep; $script:State.Usched.Clear()
    Unregister-ScheduledTask -TaskName 'ReapplyDebloat' -TaskPath '\Debloat\' -Confirm:$false -ErrorAction SilentlyContinue
    Write-Log 'Restore finished. Quarantined files are kept under C:\ProgramData\Debloat\backup\<date>\quarantine.' 'Green'
}

# ======================================================================= main
function Show-List {
    foreach ($tier in 'Safe', 'Optional') {
        Write-Host "`n$tier list:" -ForegroundColor Cyan
        foreach ($t in $Targets | Where-Object Tier -eq $tier) { Write-Host ("  {0,-46} {1}" -f $t.Name.TrimEnd('~', '.'), $t.Loses) }
    }
    Write-Host "`nNever touched: Xbox/Game Pass/Game Bar/Halo, Store, winget, Security, Claude, Edge/WebView2, Photos, Paint," -ForegroundColor DarkGray
    Write-Host 'Snipping Tool, Notepad, Calculator, Terminal, Media Player, codecs, frameworks, system packages.' -ForegroundColor DarkGray
}

if (-not ($Apply -or $WhatIf -or $Restore -or $List -or $Reapply)) {
    Show-List
    Write-Host "`n  1  Preview Safe list (no changes)`n  2  Remove Safe list`n  3  Preview Safe + Optional`n  4  Remove Safe + Optional`n  5  Restore everything this script removed`n  Q  Quit" -ForegroundColor White
    switch ((Read-Host 'Choose').Trim().ToUpper()) {
        '1' { $WhatIf = $true }
        '2' { $Apply = $true }
        '3' { $WhatIf = $true; $Optional = $true }
        '4' { $Apply = $true; $Optional = $true }
        '5' { $Restore = $true }
        default { return }
    }
}
if ($List) { Show-List; return }

# A preview changes nothing on disk (not even the log folder).
if (-not $WhatIf) {
    Initialize-DataDir
    Add-Content -LiteralPath $LogFile -Value "`r`n===== $(Get-Date -Format s)  Apply=$Apply Optional=$Optional Restore=$Restore Reapply=$Reapply" -Encoding UTF8
} else {
    # every preview overwrites its log instead of letting it grow without end
    $LogFile = Join-Path $env:TEMP 'debloat-preview.log'
    try { Set-Content -LiteralPath $LogFile -Value '' -Encoding UTF8 } catch { }
}
$script:State = Load-State

if ($Restore) {
    if (-not $Yes) {
        $confirm = Read-Host 'Reinstall every app this script removed and undo its registry changes? Type YES'
        if ($confirm -ne 'YES') { return }
    }
    Invoke-Restore; Save-State; return
}

if ($Reapply) {
    # Only re-remove what was removed before, if an update brought it back. No per-user changes.
    # an app that a Restore unblocked (but could not reinstall) stays unblocked
    $names = @($script:State.Removed | Where-Object { -not $_.Restored -and -not $_.Unblocked -and $_.Kind -in 'appx', 'capability' } | ForEach-Object Name)
    $work = @($Targets | Where-Object { $_.Kind -eq 'appx' -and $names -contains $_.Name })
    $work += @($script:State.Removed | Where-Object { -not $_.Restored -and $_.Kind -eq 'capability' } | ForEach-Object { @{ Kind = 'capability'; Name = $_.Name; Loses = '(re-check)' } })
    foreach ($e in @($script:State.Usched)) { Remove-UschedEntry $e }
} else {
    $tiers = if ($Optional) { 'Safe', 'Optional' } else { @('Safe') }
    $skipNorm = @($Skip | ForEach-Object { $_.Trim().TrimEnd('~', '.').ToLowerInvariant() })
    $work = @($Targets | Where-Object { $tiers -contains $_.Tier -and $skipNorm -notcontains $_.Name.TrimEnd('~', '.').ToLowerInvariant() })
    $skipped = @($Targets | Where-Object { $tiers -contains $_.Tier -and $skipNorm -contains $_.Name.TrimEnd('~', '.').ToLowerInvariant() })
    if ($skipped) { Write-Log "Skipping: $(($skipped | ForEach-Object Name) -join ', ')" 'DarkGray' }
    if ($Apply -and -not $WhatIf) {
        Write-Host "`nAbout to remove the $($tiers -join ' + ') list ($($work.Count) items). A restore point is created first." -ForegroundColor Yellow
        if (-not $Yes -and (Read-Host 'Type YES to continue') -ne 'YES') { return }
        # Windows silently skips a new restore point if one was made in the last 24 h; lift that limit for this one call.
        $srKey = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore'
        $prevFreq = (Get-ItemProperty -LiteralPath $srKey -Name SystemRestorePointCreationFrequency -ErrorAction SilentlyContinue).SystemRestorePointCreationFrequency
        try {
            Enable-ComputerRestore -Drive "$env:SystemDrive\" -ErrorAction SilentlyContinue
            Set-ItemProperty -LiteralPath $srKey -Name SystemRestorePointCreationFrequency -Value 0 -Type DWord
            Checkpoint-Computer -Description 'Before Remove-Bloat' -RestorePointType MODIFY_SETTINGS -ErrorAction Stop -WarningAction SilentlyContinue
            $made = Get-ComputerRestorePoint -ErrorAction SilentlyContinue | Where-Object Description -eq 'Before Remove-Bloat' | Select-Object -Last 1
            if ($made) { Write-Log 'Restore point created.' 'Green' } else { Write-Log 'Windows did not create a restore point. Continuing - every change is still backed up.' 'Yellow' }
        }
        catch { Write-Log "Restore point not created ($($_.Exception.Message)). Continuing - every change is still backed up." 'Yellow' }
        finally {
            if ($null -eq $prevFreq) { Remove-ItemProperty -LiteralPath $srKey -Name SystemRestorePointCreationFrequency -ErrorAction SilentlyContinue }
            else { Set-ItemProperty -LiteralPath $srKey -Name SystemRestorePointCreationFrequency -Value $prevFreq -Type DWord }
        }
    }
}

foreach ($t in $work) {
    try {
        switch ($t.Kind) {
            'appx'       { Remove-AppxTarget $t }
            'capability' { Remove-CapabilityTarget $t }
            'system'     {
                Write-Log "Applying: $($t.Name)  -  $($t.Loses)" 'Cyan'
                $before = $script:LogCount
                Invoke-Hook $t.Hook 'run' $t
                if ($script:LogCount -eq $before) { Write-Log '  already set' 'DarkGray' }
            }
        }
    } catch { Write-Log "  ERROR on $($t.Name): $($_.Exception.Message)" 'Red' }
    if (-not $WhatIf) { Save-State }
}

if (-not $WhatIf) {
    Save-State
    if (-not $Reapply) { Register-ReapplyTask }
}
if (-not $IsSystem) {
    Write-Host ''
    if ($WhatIf) { Write-Host 'Preview only - nothing was changed. Run with -Apply to do it.' -ForegroundColor Yellow }
    else {
        Write-Host "Finished. Log: $LogFile" -ForegroundColor Green
        Write-Host "Backups (registry exports + quarantined files): $RunDir" -ForegroundColor Green
        Write-Host 'Restart the PC. Undo any time with:  .\Remove-Bloat.ps1 -Restore' -ForegroundColor Green
        Write-Host 'Tip: Settings > Personalization > Text input > Copilot key - point it at Search or an app of your choice.' -ForegroundColor DarkGray
    }
}
