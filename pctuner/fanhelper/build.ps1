# Builds the fan helper (PCTunerFan.exe + LibreHardwareMonitorLib) into fanhelper\out.
# cargo build then embeds everything in fanhelper\out into PCTuner.exe.
# Needs the .NET SDK 8:  winget install Microsoft.DotNet.SDK.8
$ErrorActionPreference = 'Stop'
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_NOLOGO = '1'
Set-Location $PSScriptRoot
$dotnet = (Get-Command dotnet -ErrorAction SilentlyContinue).Source
if (-not $dotnet) { $dotnet = 'C:\Program Files\dotnet\dotnet.exe' }
if (Test-Path .\out) { Remove-Item .\out -Recurse -Force }
& $dotnet publish -c Release -o out
if ($LASTEXITCODE -ne 0) { throw "dotnet publish failed ($LASTEXITCODE)" }
& .\out\PCTunerFan.exe --selftest
if ($LASTEXITCODE -ne 0) { throw "fan helper selftest failed" }
Write-Host "Fan helper ready in $PSScriptRoot\out - now run: cargo build --release"
