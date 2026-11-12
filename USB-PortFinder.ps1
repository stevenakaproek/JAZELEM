#Requires -Version 5.1
<#
  USB-PortFinder.ps1
  Shows which USB controller every device is plugged into, ranked by how close that controller sits to
  the CPU, and has a plug-in test: plug a device into a port and it tells you that port's tier.

  Tier 1  CPU          - USB controller built into the CPU itself (shortest path)
  Tier 2  CPU lanes    - add-on controller (e.g. USB4 chip) wired to the CPU's own PCIe lanes
  Tier 3  Chipset      - controller inside the chipset (one extra hop through the chipset link)
  Tier 4  Chipset x2   - second, daisy-chained chipset (furthest from the CPU)

  No admin rights needed. Changes nothing.
      .\USB-PortFinder.ps1            # menu
      .\USB-PortFinder.ps1 -Map       # show every device and its tier
      .\USB-PortFinder.ps1 -Watch     # plug-in test
#>
[CmdletBinding()]
param([switch]$Map, [switch]$Watch)

$TierText = @{
    1 = 'Tier 1  CPU (built into the CPU)'
    2 = 'Tier 2  CPU lanes (via add-on USB4 chip)'
    3 = 'Tier 3  Chipset'
    4 = 'Tier 4  Second chipset (furthest)'
    9 = 'Unknown'
}
$TierColor = @{ 1 = 'Green'; 2 = 'Cyan'; 3 = 'Yellow'; 4 = 'DarkYellow'; 9 = 'Gray' }

# ------------------------------------------------------------------ device tree
function Get-Tree {
    $all = @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue)
    $byId = @{}; foreach ($d in $all) { $byId[$d.InstanceId] = $d }
    $props = @($all | Get-PnpDeviceProperty -KeyName DEVPKEY_Device_Parent, DEVPKEY_Device_LocationInfo, DEVPKEY_Device_LocationPaths, DEVPKEY_Device_BusReportedDeviceDesc -ErrorAction SilentlyContinue)
    $parent = @{}; $locInfo = @{}; $locPath = @{}; $busDesc = @{}
    foreach ($p in $props) {
        switch ($p.KeyName) {
            'DEVPKEY_Device_Parent'               { $parent[$p.InstanceId] = [string]$p.Data }
            'DEVPKEY_Device_LocationInfo'         { $locInfo[$p.InstanceId] = [string]$p.Data }
            'DEVPKEY_Device_LocationPaths'        { $locPath[$p.InstanceId] = @($p.Data)[0] }
            'DEVPKEY_Device_BusReportedDeviceDesc'{ $busDesc[$p.InstanceId] = [string]$p.Data }
        }
    }
    [pscustomobject]@{ All = $all; ById = $byId; Parent = $parent; LocInfo = $locInfo; LocPath = $locPath; BusDesc = $busDesc }
}

function Get-Ancestors($tree, [string]$id) {
    $chain = @(); $cur = $id; $guard = 0
    while ($cur -and $tree.Parent.ContainsKey($cur) -and $guard -lt 40) { $cur = $tree.Parent[$cur]; $chain += $cur; $guard++ }
    return $chain
}

# Classify a USB host controller (PCI device) by its place in the PCI tree.
function Get-ControllerTier($tree, [string]$ctlId) {
    $anc = Get-Ancestors $tree $ctlId
    # AMD Promontory chipset switch upstream ports (600/800-series chipsets). Each one = one chipset hop.
    $chipsetHops = @($anc | Where-Object { $_ -match 'VEN_1022&DEV_43F4' }).Count
    $path = $tree.LocPath[$ctlId]
    $depth = if ($path) { ([regex]::Matches($path, '#PCI\(')).Count } else { 99 }
    if ($chipsetHops -ge 2) { return 4 }
    if ($chipsetHops -eq 1) { return 3 }
    if ($depth -le 2) { return 1 }
    if ($depth -lt 99) { return 2 }
    return 9
}

function Get-Controllers($tree) {
    $ctl = @($tree.All | Where-Object {
        $_.InstanceId -like 'PCI\*' -and ($_.Class -eq 'USB' -or $_.FriendlyName -match 'USB4') -and $_.FriendlyName -match 'Host Controller|Host Router'
    })
    foreach ($c in $ctl) {
        [pscustomobject]@{ Id = $c.InstanceId; Name = $c.FriendlyName; Tier = (Get-ControllerTier $tree $c.InstanceId); Path = $tree.LocPath[$c.InstanceId] }
    }
}

function Get-Kind([string]$text) {
    if ($text -match 'Xbox|Controller|Gamepad|XINPUT|DualSense|DualShock|8BitDo|Joystick|Wireless Adapter') { return 'GAME CONTROLLER' }
    if ($text -match 'Audio|Headset|Headphone|Arctis|DAC|Speaker|Sound|Microphone|Mic\b|HyperX|Astro|SteelSeries|Logitech G') { return 'AUDIO' }
    if ($text -match 'Mouse') { return 'mouse' }
    if ($text -match 'Keyboard') { return 'keyboard' }
    if ($text -match 'Mass Storage|Disk|Flash|Card Reader|UAS') { return 'storage' }
    if ($text -match 'Bluetooth') { return 'bluetooth' }
    if ($text -match 'Hub') { return 'hub' }
    return ''
}

# Every physical USB device (USB\VID_ node that is not a hub), with its controller, tier, port and hub count.
function Get-UsbDevices($tree, $controllers) {
    $ctlById = @{}; foreach ($c in $controllers) { $ctlById[$c.Id] = $c }
    # Only real plugs: a physical USB device always reports a "Port_#xxxx.Hub_#yyyy" location (virtual children do not).
    $devs = @($tree.All | Where-Object { $_.InstanceId -like 'USB\VID_*' -and $_.InstanceId -notmatch '&MI_' -and $_.FriendlyName -notmatch 'Root Hub' -and $tree.LocInfo[$_.InstanceId] -like 'Port_#*' })
    foreach ($d in $devs) {
        $anc = Get-Ancestors $tree $d.InstanceId
        $ctl = $null; foreach ($a in $anc) { if ($ctlById.ContainsKey($a)) { $ctl = $ctlById[$a]; break } }
        # External / onboard hubs between the device and the controller's root hub
        $hubs = @($anc | Where-Object { $_ -like 'USB\VID_*' -and $tree.ById[$_] -and $tree.ById[$_].FriendlyName -match 'Hub' }).Count
        # Child names (composite devices show their real function in the children)
        $children = @($tree.Parent.Keys | Where-Object { $tree.Parent[$_] -eq $d.InstanceId } | ForEach-Object { if ($tree.ById[$_]) { $tree.ById[$_].FriendlyName } })
        $name = if ($tree.BusDesc[$d.InstanceId]) { $tree.BusDesc[$d.InstanceId] } else { $d.FriendlyName }
        $kind = Get-Kind ("$name $($d.FriendlyName) " + ($children -join ' '))
        if ($kind -eq 'hub') { continue }
        # Built into the motherboard (wired to an internal USB port): MSI's own devices (VID 0DB0) such as onboard
        # audio and Mystic Light, and the onboard Wi-Fi/Bluetooth card. These cannot be moved to another port.
        $onboard = ($d.InstanceId -match 'VID_0DB0&') -or ($kind -eq 'bluetooth')
        [pscustomobject]@{
            Name = $name; Kind = $kind; Onboard = $onboard; Tier = $(if ($ctl) { $ctl.Tier } else { 9 }); Controller = $(if ($ctl) { $ctl.Name } else { '?' }); CtlId = $(if ($ctl) { $ctl.Id } else { '' })
            Port = $tree.LocInfo[$d.InstanceId]; Hubs = $hubs; Id = $d.InstanceId; Functions = ($children | Select-Object -Unique) -join ', '
        }
    }
}

# ------------------------------------------------------------------ views
function Show-Map {
    Write-Host 'Reading the USB tree...' -ForegroundColor DarkGray
    $tree = Get-Tree; $controllers = @(Get-Controllers $tree); $devices = @(Get-UsbDevices $tree $controllers)
    Write-Host "`nUSB CONTROLLERS (closest to the CPU first)" -ForegroundColor White
    foreach ($c in $controllers | Sort-Object Tier, Name) {
        $n = @($devices | Where-Object CtlId -eq $c.Id).Count
        Write-Host ("  {0,-42} {1}   ({2} device(s))" -f $TierText[$c.Tier], $c.Name, $n) -ForegroundColor $TierColor[$c.Tier]
    }
    Write-Host "`nCONNECTED DEVICES" -ForegroundColor White
    foreach ($d in $devices | Sort-Object Tier, Kind, Name) {
        $tag = if ($d.Kind) { "[$($d.Kind)]" } else { '' }
        $hubNote = if ($d.Hubs -gt 0) { "  via $($d.Hubs) hub(s)" } else { '' }
        if ($d.Onboard) { $hubNote += '  (built into the motherboard - cannot be moved)' }
        Write-Host ("  {0,-12} {1,-18} {2}" -f "Tier $($d.Tier)", $tag, $d.Name) -ForegroundColor $TierColor[$d.Tier]
        Write-Host ("               {0}  {1}{2}" -f $d.Controller, $d.Port, $hubNote) -ForegroundColor DarkGray
    }
    $key = @($devices | Where-Object { $_.Kind -in 'GAME CONTROLLER', 'AUDIO' -and -not $_.Onboard })
    Write-Host "`nYOUR GAME CONTROLLER / AUDIO" -ForegroundColor White
    if (-not $key) { Write-Host '  None found on USB. (A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.)' -ForegroundColor DarkGray }
    foreach ($d in $key) {
        $verdict = switch ($d.Tier) { 1 { 'already on a CPU port - nothing to do' } 2 { 'on CPU lanes - fine' } default { 'on a chipset port - move it to a Tier 1 port if one is free' } }
        if ($d.Hubs -gt 0) { $verdict += '; it is going through a hub - plug it into the PC directly' }
        Write-Host ("  {0}: {1}  ->  {2}" -f $d.Kind, $d.Name, $verdict) -ForegroundColor $TierColor[$d.Tier]
    }
    Write-Host "`nTo find which physical socket is which tier, run the plug-in test (option 2)." -ForegroundColor DarkGray
    Write-Host 'Good to know: the latency difference between tiers is a few microseconds. What matters more is plugging' -ForegroundColor DarkGray
    Write-Host 'straight into the PC (no hub, front panel or monitor port) and not sharing a port group with a busy webcam or drive.' -ForegroundColor DarkGray
}

function Start-Watch {
    Write-Host "`nPLUG-IN TEST" -ForegroundColor White
    Write-Host 'Plug any USB device (a flash drive or your controller) into a port, wait 2 seconds, then move it to the next port.'
    Write-Host 'Each time, this shows that port''s tier. Press Ctrl+C to stop.' -ForegroundColor DarkGray
    $seen = @{}; foreach ($d in Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | Where-Object InstanceId -like 'USB\VID_*') { $seen[$d.InstanceId] = $true }
    $n = 0
    while ($true) {
        Start-Sleep -Milliseconds 1500
        $now = @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | Where-Object { $_.InstanceId -like 'USB\VID_*' -and $_.InstanceId -notmatch '&MI_' })
        $new = @($now | Where-Object { -not $seen.ContainsKey($_.InstanceId) })
        $nowIds = @{}; foreach ($d in $now) { $nowIds[$d.InstanceId] = $true }
        foreach ($k in @($seen.Keys)) { if (-not $nowIds.ContainsKey($k)) { $seen.Remove($k) } }   # unplugged -> forget, so re-plugging is detected
        if (-not $new) { continue }
        Start-Sleep -Milliseconds 800   # let Windows finish enumerating the device
        $tree = Get-Tree; $controllers = @(Get-Controllers $tree)
        $info = @(Get-UsbDevices $tree $controllers) | Where-Object { $new.InstanceId -contains $_.Id }
        foreach ($d in $info) {
            $n++
            $hubNote = if ($d.Hubs -gt 0) { "  (through $($d.Hubs) hub(s) - a front-panel or monitor port)" } else { '' }
            Write-Host ''
            Write-Host ("  #{0}  {1}" -f $n, $d.Name) -ForegroundColor White
            Write-Host ("      THIS PORT = {0}{1}" -f $TierText[$d.Tier], $hubNote) -ForegroundColor $TierColor[$d.Tier]
            Write-Host ("      {0}  {1}" -f $d.Controller, $d.Port) -ForegroundColor DarkGray
        }
        foreach ($d in $new) { $seen[$d.InstanceId] = $true }
    }
}

# ------------------------------------------------------------------ main
if ($Map) { Show-Map; return }
if ($Watch) { Start-Watch; return }
Write-Host 'USB Port Finder' -ForegroundColor White
Write-Host '  1  Show every USB device and how close its port is to the CPU'
Write-Host '  2  Plug-in test: find out which physical port is which tier'
Write-Host '  Q  Quit'
switch ((Read-Host 'Choose').Trim().ToUpper()) {
    '1' { Show-Map }
    '2' { Start-Watch }
    default { }
}
