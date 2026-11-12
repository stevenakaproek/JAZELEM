# PC Tuner

A gaming-safe Windows 11 tuning toolkit, built for a Ryzen 7 9800X3D + RTX 5080 gaming PC
that plays Halo Infinite (Xbox app / Game Pass) and Steam games.

Every change is previewed first, applied with live progress, and can be undone. Anything Xbox,
Game Pass, Steam, Easy Anti-Cheat, the Store, Windows Security or Edge depends on is left alone.

![RTX 5080 tab](docs/gpu.png)

## What is in here

| Folder / file | What it is |
| --- | --- |
| `pctuner/` | **PC Tuner** - one Rust (egui) program with every tool below as a tab |
| `ServiceManager.ps1` | Disable / set to Manual unneeded services, with presets and undo |
| `StartupCleaner.ps1` | Switch off startup junk (Run keys, tasks, Explorer add-ons), never deletes |
| `Remove-Bloat.ps1` | In-depth removal of bundled Store apps, with reinstall blocks and restore |
| `PostInstall-Tweaks.ps1` | Telemetry tasks, Edge preloading, Teredo for Xbox online play |
| `TweakScheduler.ps1` | Win32PrioritySeparation presets (CPU scheduler) |
| `USB-PortFinder.ps1` | Finds which USB ports sit closest to the CPU |
| `RTX5080/` | MSI Afterburner undervolt presets (Stock / Efficiency / Balanced / Performance) |
| `README.txt` | Full usage notes for every tool |

## PC Tuner tabs

- **Services, Startup Cleaner, Remove Bloat, Post-install Tweaks, CPU Scheduler, USB Port Finder** -
  same rules and safety checks as the scripts.
- **Network** - live ping / jitter / packet-loss test, adapter power saving and latency settings,
  with an honest note on what each one does (most "ping tweaks" are TCP-only; Halo's match traffic is UDP).
- **RTX 5080 Profiles** - live clock / temperature / power / load graphs, presets, an always-visible
  **Reset GPU to stock**, and a safety trial that puts the GPU back on Stock at the next sign-in
  if Windows logs a crash while a new tune is being tested.
- **Fan Tuner** - live fan RPM / duty and CPU / GPU / board temperatures; per fan: BIOS, Manual
  (slider) or Curve (draggable points, follows CPU, GPU or the hotter of the two); Silent / Balanced /
  Performance profiles plus your own, the last one applied at startup. Safety: never below 20 %
  (pumps 50 %), 100 % at 90 °C, fans back to the BIOS when PC Tuner closes or crashes.

![Fan Tuner tab](docs/fan.png)
![Network tab](docs/network.png)
![Preview before a change](docs/services-preview.png)

## Build

Needs Rust (rustup, MSVC toolchain), Visual Studio 2022 Build Tools (C++) and, for the fan helper,
the .NET 8 SDK (`winget install Microsoft.DotNet.SDK.8`). The fan helper is built first and embedded
into PCTuner.exe:

```
cd pctuner
powershell -ExecutionPolicy Bypass -File fanhelper\build.ps1
cargo build --release
```

Fan control at run time needs the [PawnIO](https://pawnio.eu) driver installed (not WinRing0).

The release build asks for administrator rights (it changes system settings). The debug build runs
unelevated in view-only mode. `cargo test` runs the unit tests (use the debug profile).

## Third-party components

The Fan Tuner's helper (`pctuner/fanhelper`) uses, unmodified from NuGet,
[LibreHardwareMonitorLib](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) 0.9.6
(MPL-2.0; source code at that link) and its dependencies (HidSharp - Apache-2.0, and others listed in
its package). These DLLs are embedded in PCTuner.exe and unpacked to `%LOCALAPPDATA%\PCTuner\fan`.
Hardware access goes through the separately installed [PawnIO](https://pawnio.eu) driver.

## Disclaimer

These tools change Windows services, the registry, scheduled tasks, network adapter settings,
fan speeds and GPU tuning. They were written for one specific PC. Read the preview before applying anything,
and use at your own risk.
