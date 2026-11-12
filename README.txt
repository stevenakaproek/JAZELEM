PC TUNER - after-reinstall gaming toolkit
=========================================

Trims Windows services and startup items while keeping everything that
Xbox / Game Pass / Steam / Easy Anti-Cheat need. The presets are tailored
to this PC: wired Ethernet, no printer, no camera, Bluetooth controller.

Files
-----
  ServiceManager.cmd     Double-click: Service Manager (services, presets, undo).
  ServiceManager.ps1     The Service Manager app.
  StartupCleaner.cmd     Double-click: Startup Cleaner (autoruns junk, one-click clean, undo).
  StartupCleaner.ps1     The Startup Cleaner app.
  PostInstall-Tweaks.ps1 Script alternative: scheduled tasks, Edge background, OneDrive.
  Remove-Bloat.cmd/.ps1  Deep-removes bloat apps and blocks Windows from reinstalling them (menu, -Restore).
  Optimize-Services.ps1  Script version of the presets (no window), if you prefer.
  Run-Recheck.cmd        Runs the Remove-Bloat re-check task now and shows the result.
  TweakScheduler.cmd/.ps1 CPU scheduler (Win32PrioritySeparation) presets with undo.
  USB-PortFinder.cmd/.ps1 Finds which USB ports sit closest to the CPU (no admin).
  RTX5080\               RTX 5080 Profile Manager (MSI Afterburner presets).
  PCTuner.exe            Everything above in one program (download from GitHub
                         Releases, or build from pctuner\).
  README.txt             This file.


AFTER REINSTALLING WINDOWS
--------------------------
1. Install your games/launchers FIRST (Xbox app, Gaming Services, Steam),
   and let Windows Update finish. Some services only exist after that.

2. Unzip this folder somewhere permanent, e.g. C:\Tools\ServiceManager.
   If Windows blocks it: right-click the zip > Properties > tick "Unblock"
   before unzipping.

3. Double-click ServiceManager.cmd > Yes to the admin prompt.
   Click "Apply Safe preset"  (or "Apply Safe + Optional preset").
   Check the preview list > Yes. Reboot.

4. Double-click StartupCleaner.cmd > Yes to the admin prompt.
   Click "Clean junk (one click)" (or "Clean junk + optional") > check the preview > Yes.
   Sign out / reboot afterwards (Widgets, search and Explorer changes need it).
   Remove bloat apps: double-click Remove-Bloat.cmd > Yes to the admin prompt >
   pick 1 (preview) first, then 2 (Safe) or 4 (Safe + Optional). Restart after.
   Then, if you don't use OneDrive, uninstall it (Settings > Apps), and stop Edge
   preloading - in an Administrator PowerShell in this folder run:
       powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1 -RemoveOneDrive
   (leave off -RemoveOneDrive to keep OneDrive)

5. Manual settings:
   - Settings > Personalization > Taskbar > Widgets: Off
   - Settings > Apps > Default apps: pick your browser
   - Settings > Apps > Installed apps: uninstall Copilot if unused


USING THE APP
-------------
  Search box            filter by name
  Disable/Manual/Auto   change the selected services (Ctrl/Shift-click for many)
  Undo last change      reverts the last click or the whole last preset
  History...            undo any individual change, or restore from a backup CSV
  Yellow rows           gaming dependencies - asks before changing
  Red rows              core Windows services - locked

Change history is kept in %LOCALAPPDATA%\ServiceManager\history.json,
so Undo still works after rebooting.


PRESETS
-------
Safe:     telemetry, printing/scanning, phone/SMS, Remote Desktop HOST,
          WinRM, hotspot/cellular, BranchCache, domain logon, Offline Files,
          wallet/NFC, parental controls, sensors, File History,
          Hyper-V guest services, Edge Update -> Manual, Edge Elevation off.
          NOTE: disables the Print Spooler (incl. "Print to PDF").
          If you buy a printer: select "Print Spooler" > Automatic.

          Storage Service -> Manual, Copilot elevation helper off.
Optional: Safe + Windows Search (file search gets slow), SysMain, whesvc,
          diagnostics (troubleshooters + Task Manager App history), NetBIOS
          helper, Program Compatibility Assistant,
          file sharing from this PC, troubleshooters, Windows AI,
          Phone Link / Nearby Share, Logitech LampArray RGB,
          smart card / biometric (Windows Hello fingerprint/face).


STARTUP CLEANER
---------------
Shows what Autoruns shows on its main tabs: Run keys, Startup folders,
scheduled tasks, Store app startup tasks, Explorer icon overlays and
right-click handlers (incl. .exe / shortcut menus), plus "Windows setting"
policies that stop background features. Nothing is ever deleted - items are
switched off the same way Task Manager does it, settings are restored exactly.

  Clean junk (one click)  switches off everything orange: telemetry / unused-
                          feature tasks, OneDrive, Edge autolaunch, Teams,
                          Office Hub, NVIDIA "Run with graphics processor" menu,
                          Widgets, suggested/silent app installs + tips, Bing in
                          Start search, telemetry locked to Required, activity
                          history, setup nag, Delivery Optimization LAN sharing.
  Clean junk + optional   also blue items: Security tray icon, Defender right-
                          click, Microsoft-account backups, Wi-Fi sync, theme
                          sync, Cross-device Resume, Spotlight (desktop + lock
                          screen - also pick "Picture" in Settings).
  Orange = junk   Blue = optional   Yellow = keep (gaming/hardware)
  Switch off / on         selected rows (asks before switching off a "keep")
  Show all Windows tasks  lists every Windows scheduled task (leave these alone
                          unless you know what they do)
  Restart Explorer        needed after changing Explorer extensions

History: %LOCALAPPDATA%\StartupCleaner\history.json


UNDO EVERYTHING
---------------
  Apps:     History... > "Undo everything" (Startup Cleaner)
            History... > select all > Undo selected (Service Manager)
  Apps:     powershell -ExecutionPolicy Bypass -File .\Remove-Bloat.ps1 -Restore
  Tweaks:   powershell -ExecutionPolicy Bypass -File .\PostInstall-Tweaks.ps1 -Undo
            (OneDrive: reinstall from the Microsoft Store)
            The first Apply saves the state of every task, the Edge / Teredo
            policies and the Teredo type to %LOCALAPPDATA%\PCTuner\
            tweaks-state.json; -Undo puts back exactly that (tasks that were
            already off stay off, tasks the Startup Cleaner switched off stay
            off) and deletes the file. Without that file (tweaks applied by an
            older version) -Undo resets the tasks and policies to the Windows
            defaults instead.

KNOWN ISSUE: WIDGETS
--------------------
Windows' UCPD protection driver blocks scripts (even as admin) from writing
the "Allow widgets" policy, so Startup Cleaner can't switch Widgets off.
Use Group Policy instead:
  Win+R > gpedit.msc > Computer Configuration > Administrative Templates >
  Windows Components > Widgets > "Allow widgets" > Disabled > OK, then sign out.
Undo: set it back to "Not Configured".

XBOX ONLINE PLAY: TEREDO
------------------------
This Windows build ships with Teredo disabled, and plain "client" mode turns
itself off because the router hands out a "lan" DNS suffix ("managed network").
PostInstall-Tweaks.ps1 now sets Teredo to "enterpriseclient" (Microsoft server
win1910.ipv6.microsoft.com) with netsh AND as a Group Policy, so it sticks.
Check it (the ping times out - that is expected, it just wakes Teredo):
    ping -6 -n 2 2001:4860:4860::8888; netsh interface teredo show state
Good result: State qualified (or dormant), UPNP: Yes.
Router: keep UPnP enabled for Open NAT.
Manual alternative: gpedit.msc > Computer Configuration > Administrative
Templates > Network > TCPIP Settings > IPv6 Transition Technologies >
"Set Teredo State" = Enabled, Enterprise Client; "Set Teredo Server Name" =
win1910.ipv6.microsoft.com.

TWEAK SCHEDULER (TweakScheduler.cmd)
------------------------------------
Shows the current CPU scheduler setting (Win32PrioritySeparation - the same
value Bitsum TweakScheduler edits; use one tool or the other) and switches
between three presets, with undo:
  1 - Windows default (0x02)   RECOMMENDED for Halo Infinite. Short turns,
                               game gets 3x longer turns + max wake-up boost.
  2 - Game (0x2A = 42)         The community "game" value. Same for the game,
                               background apps get equal turns. No measured benefit.
  3 - Long turns (0x16 = 22)   Keeps 3:1 + boost, doubles every turn. Unmeasured.
Research found no controlled benchmark where any value beats the default.
To test for yourself: restart after each change, capture with CapFrameX
(overlay off) on a fixed route in an empty custom map, 10+ runs per value,
and first compare default against default to learn your normal run-to-run
spread. Keep a preset only if its gain is clearly bigger than that spread.

REMOVE-BLOAT (Remove-Bloat.cmd)
-------------------------------
Removes apps "in depth" and stops Windows putting them back:
  - deprovisions each app (feature updates and new profiles no longer reinstall it)
  - uninstalls it for all users + writes the Deprovisioned marker
  - deletes its Windows Update auto-install entry (Outlook, Dev Home, Cross
    Device, Teams) - EdgeUpdate / Store / language-pack entries are kept
  - cleans leftovers like Revo: app data folders, alias folders, Copilot and
    Teams add-in folders, Outlook cache, stale registry entries
  - files are MOVED to C:\ProgramData\Debloat\backup\<date>\quarantine and
    registry keys are exported first; a restore point is made before starting
  - a SYSTEM task (\Debloat\ReapplyDebloat, startup + daily) removes anything
    a future update brings back
SAFE list: Copilot (+ OfficeHub identity), Clipchamp, News, Weather, Bing
  Search, Solitaire, Power Automate, Dev Home stub, Camera, Feedback Hub, MSN
  widgets, pen handwriting, Steps Recorder, Hello Face, Print Management,
  Recall (policy), OneDrive leftovers.
OPTIONAL list: Widgets (works around the UCPD block), Teams + meeting add-in,
  new Outlook, Phone Link, Cross Device, To Do, Sticky Notes, Clock, Sound
  Recorder, Quick Assist, Get Help (troubleshooters!), Xbox speech-to-text
  overlay, WMP Legacy, IE engine, math input, PowerShell ISE, speech, OCR.
  Skip any with:  .\Remove-Bloat.ps1 -Apply -Optional -Skip MSTeams,Microsoft.Todos
NEVER touched: Xbox app, Gaming Services, Game Bar, Xbox sign-in, Halo, Store,
  winget, Windows Security, Claude, Edge/WebView2, Photos, Paint, Snipping
  Tool, Notepad, Calculator, Terminal, Media Player, codecs, system packages.
Undo:  .\Remove-Bloat.ps1 -Restore   (reinstalls the apps that have a Store
  id for the current user, only unblocks the ones without one - Bing Search,
  Dev Home, MSN widgets, handwriting, Widgets runtime, Cross Device, Xbox
  speech-to-text, Teams add-in - which you reinstall by hand; re-enables the
  ShowFeedbackToast task, the Recall feature and the handwriting framework;
  reverts registry/policy changes including the Default-user OneDrive Run
  entry; removes the task). After it: set the Copilot key in
  Settings > Personalization > Text input if you want it to do something.

USB PORT FINDER (USB-PortFinder.cmd)
------------------------------------
Shows how close each USB port is to the CPU and which port each device is on.
No admin needed, changes nothing.
  Tier 1  CPU (built into the CPU)        best for controller / headset
  Tier 2  CPU lanes via the USB4 chip     the USB4 Type-C ports, also fine
  Tier 3  Chipset
  Tier 4  Second chipset (furthest)
Option 1 = map of every device. Option 2 = plug-in test: plug a flash drive
into each rear port in turn and it prints that port's tier - label the
Tier 1 ports. Plug straight into the PC (no hub / front panel / monitor port).

RTX 5080 PROFILE MANAGER (RTX5080\RTX5080-ProfileManager.cmd)
--------------------------------------------------------------
Switches the RTX 5080 between the undervolt presets through MSI Afterburner
profile slots and shows which one is applied right now:
  Stock        slot 3   no offsets                         memory +0
  Efficiency   slot 2   2700 MHz from 900 mV               memory +1000
  Balanced     slot 1   2800 MHz from 925 mV               memory +1500
  Performance  slot 4   2900 MHz from 950 mV               memory +2000
"Currently applied" = live memory offset (nvidia-smi, under load) + the
settings Afterburner restores at startup. Needs MSI Afterburner installed and
running (settings only apply while it runs). If a slot is missing it is
written for you; the Afterburner profile file is backed up to RTX5080\backups
first ("Restore last backup" puts it back). After a reinstall: install
Afterburner, open and close it once, then run the manager.
Copy the whole RTX5080 folder to C:\Users\<you>\Documents\RTX5080-Balanced
(or anywhere) - the Apply-*.bat files are the older one-click versions.

PC TUNER (PCTuner.exe) - everything above in one program
--------------------------------------------------------
Double-click PCTuner.exe (asks for admin). Tabs: Services, Startup Cleaner,
Remove Bloat, Post-install Tweaks, Network, Performance, CPU Scheduler,
RTX 5080 Profiles, Fan Tuner, USB Port Finder - same lists, rules, safety
checks and undo as the separate tools; it also imports the Service Manager
and Startup Cleaner change history. Its own history: %LOCALAPPDATA%\PCTuner.
Source code: pctuner\ (Rust). Rebuild: install Rust (rustup, MSVC toolchain),
Visual Studio 2022 Build Tools (C++) and the .NET 8 SDK (winget install
Microsoft.DotNet.SDK.8), then in pctuner\ run:
  powershell -ExecutionPolicy Bypass -File fanhelper\build.ps1
and then  cargo build --release  (the fan helper is embedded into the exe;
fan control needs the PawnIO driver).

How it works: every system change (services, startup, bloat, tweaks,
network, performance, CPU scheduler, GPU profiles) opens a window that first
shows exactly what will change (with a warning if it is risky), then a
progress bar, then each item ticking off with a count and an Undo button.
Nothing is applied until you press the green button in that window.
Remove Bloat confirms in that window, then shows the progress and each item
in the page's Result card, with an "Undo (Restore everything)" button when a
removal finishes.
If you close PC Tuner while a change is being applied, it waits and closes
by itself when the change has finished (so it is recorded and can be
undone); "Close anyway" is there if you really need it.
Scripts that run as administrator (Remove Bloat, Post-install Tweaks) are
written to %ProgramData%\PCTuner, which only administrators can change.
If a history file cannot be read, PC Tuner says so and keeps it as
<name>.corrupt-<time> instead of overwriting it.
Exceptions: Fan Tuner changes apply live (no preview window); BIOS mode or
closing PC Tuner hands the fans back to the BIOS. Power / BIOS asks once,
then restarts or shuts down.

Power / BIOS (bottom of the sidebar, above "Reset GPU to stock"):
- "Restart into BIOS" restarts straight into the motherboard's UEFI/BIOS
  setup; "Shut down, BIOS on next start" turns the PC off so the next start opens it.
  Uses Windows' own shutdown /fw - no need to press Del at the right moment.
- Needs PC Tuner run as administrator and a UEFI-booted PC (otherwise the
  buttons are disabled with the reason). It asks for confirmation, then every
  open program closes immediately without saving. There is nothing to undo.

Performance tab:
- Built from researched evidence: most "gaming tweaks" online are placebo or
  harmful on Windows 11; the tab lists those under "Left alone on purpose".
- Checks (read only): Game Mode, GPU scheduling (HAGS), background recording,
  refresh rate (flags a 540 Hz PG27AQWP-W running at 500 Hz: needs a DP 2.1
  UHBR20 cable), animations, transparency, USB suspend, PCIe power saving,
  Do Not Disturb while gaming (opens the setting).
- Recommended: Optimizations for windowed games (flip model for DX10/11
  borderless games), faster submenus (400 -> 100 ms, applies at once).
- Optional: Windows VRR for old DX11 games, faster tooltips, taskbar
  animations off, faster taskbar thumbnails, no startup-app delay, classic
  right-click menu, CPU energy preference 25 -> 10, mouse acceleration off,
  hibernation off (frees hiberfil.sys, ~12.6 GB here).
- Taskbar / right-click changes need "Restart Explorer" (not during a game).
- Every change is saved in %LOCALAPPDATA%\PCTuner\perf-history.json and
  "Undo last change" restores the exact previous values (failed parts stay
  in the history so they can be undone again).

Fan Tuner tab:
- Needs the PawnIO driver (already installed on this PC) and PC Tuner run as
  administrator. Without admin only the graphics card is shown and nothing
  can be changed. A saved startup profile is shown but not applied until PC
  Tuner runs as administrator.
- Live RPM / duty per fan, CPU / GPU / board temperatures, graphs.
- Per fan: BIOS (hands off), Manual (slider) or Curve (drag the points;
  follows the CPU, the GPU or the hotter of the two). Headers the library
  cannot control are shown as "read-only"; headers that never spin (nothing
  plugged in) are folded away under "headers at 0 RPM". On the MSI X870E
  Tomahawk every header, SYS fans included, is controllable (tested).
- Profiles: Silent / Balanced / Performance (made for your fans the first time
  you pick one; "Reset to default" undoes your edits) and your own ("Save as
  new"). The last profile you picked is applied when PC Tuner starts.
  A fan you change yourself gets its own setting ("own setting" chip):
  switching profiles leaves it alone until you press "Follow profile".
  "Give all fans back to BIOS" puts every fan on BIOS control, clears own
  settings, and no profile is applied at startup until you pick one again.
  "This is a pump" ticks are kept for the fan itself, the same in every
  profile. "Save as new" refuses a name that is already used (pick that
  profile and use Save changes instead).
  Saved in %LOCALAPPDATA%\PCTuner\fan-profiles.json.
- Safety (inside the PCTunerFan helper, so it holds if the window freezes):
  never below 20 % (pumps 50 %, GPU fan 30 % = its driver minimum); CPU or
  GPU at 90 C -> every controlled fan 100 % until both are under 80 C; if
  temperatures cannot be read for 5 s the fans go back to the BIOS until they
  can be read again (your settings then come back by themselves; a banner
  shows it); fans go
  back to the BIOS when PC Tuner closes, crashes, or stops talking for 10 s.
- Limits: if both PC Tuner and its PCTunerFan helper are force-killed
  together (e.g. End process tree), fans stay at their last speed (never
  under the floor) until the PC restarts. GPU fans: turn off MSI Afterburner's fan control first.
- First-run checklist (run as administrator):
  1. The tab lists your motherboard fans; note which say "read-only".
  2. CPU Fan > Manual > move the slider: the RPM follows within a few seconds.
  3. Close PC Tuner: the fan returns to its BIOS speed.
  4. Pick Balanced, close and reopen PC Tuner: Balanced is applied again.
  5. Optional: End task on PCTuner.exe in Task Manager - within ~10 s the
     fans return to BIOS control.

Network tab:
- Latency test: 20 pings to the internet (1.1.1.1 by default) and to your
  router, with average, jitter and packet loss. Changes nothing.
- Settings: adapter power saving off (EEE, Green Ethernet, Power Saving Mode,
  Gigabit Lite, Selective Suspend), Interrupt Moderation and Flow Control off,
  network throttling off, TCP delayed ACK / Nagle off (TCP only - Halo's match
  traffic is UDP). Optional: DSCP 46 tag for HaloInfinite.exe (only helps if
  the router uses DSCP), UDP receive coalescing off (A/B test), Wake-on-LAN off.
- Applying adapter settings restarts the adapter: about 5 seconds offline.
- Left alone on purpose: IPv6 (Teredo/Xbox need it), TCP auto-tuning,
  offloads, DNS, and old registry tweaks Windows 10/11 ignore.
- The biggest fix for lag spikes is usually bufferbloat on the router
  (turn on SQM / Smart Queue). The tab links to a test for it.

GPU safety:
- "Reset GPU to stock" (bottom-left, always visible) puts the card back on
  the Stock preset: no undervolt, no memory overclock.
- Applying a preset you have not kept before starts a TRIAL. A yellow box asks
  "keep this tune?" with Keep / Revert now. While the trial is open, a sign-in
  task (\PCTuner\GpuTrialGuard) checks after every restart: if Windows logged
  a crash or blue screen since the tune was applied, Stock is sent to MSI
  Afterburner and PC Tuner tells you why next time it opens. The check is only
  removed once Stock is confirmed (live memory offset, or Afterburner's saved
  startup settings); otherwise the notice says "not yet confirmed" and the
  check sends Stock again at the next sign-in.
- Restore last backup also ends an open trial (it is listed as a step).
- If a change fails after Afterburner was closed, PC Tuner starts it again
  unchanged (the sign-in check leaves it closed instead, so the crashed
  preset is not re-applied).
- Limits: the revert happens at the next sign-in after a crash, not during a
  freeze. It needs MSI Afterburner installed. If you move PCTuner.exe, the
  task is re-created at the next trial. Log: %LOCALAPPDATA%\PCTuner\gpu-guard.log

First-run checklist (these could only be tested without admin rights so far):
1. Services: select one harmless service (e.g. Fax) > Disable > check the
   result window > Undo last change.
2. Startup Cleaner: Clean junk > check the result > Undo this cleanup.
3. CPU Scheduler: Apply selected preset > Undo last change.
4. RTX 5080: Apply Balanced > the yellow trial box appears > Keep.
   Then press Reset GPU to stock and confirm the tab shows Stock.
5. In Task Scheduler, \PCTuner\GpuTrialGuard should exist only while a trial
   is open.

Tray, Power and extra Performance options (1.3.1):
- Minimizing PC Tuner hides it to the system tray. Right-click the tray icon
  for Show PC Tuner, Restart into BIOS, Shut down (BIOS on next start), Quit.
  Closing with X still quits (fans go back to the BIOS).
- Power / BIOS acts on one click - every open program closes without saving.
- Performance > VBS off (optional): turns off virtualization-based security
  (DeviceGuard + Windows Hello enhanced sign-in). Restart needed. Lowers
  security; small gain because Memory Integrity is already off. Exact undo.
- Performance > Background apps off (optional): Store apps cannot run in the
  background (policy); Xbox app, Game Bar, Xbox sign-in, Gaming Services and
  the Store are exempt. Desktop programs are not affected. Exact undo.
- Performance > Defender: game folders: excludes Steam / XboxGames / Epic /
  EA / Battle.net (and folders you add) from Defender scans. Undo removes only
  the exclusions PC Tuner added. Needs administrator.

Services backups (1.3.2):
- Services > Backups > Create backup now saves the startup type of every
  service to Documents\PCTuner-Backups\Services. PC Tuner also makes one by
  itself before the first service change.
- Restore this backup (in the app) shows which services differ, then puts
  them back: sc.exe config, then a registry write, then - for services
  Windows protects - the same write as SYSTEM. Restart afterwards.
- Every backup also has RESTORE-services-<time>.cmd (right-click > Run as
  administrator; works in Safe Mode; skips services that already match) and
  services-<time>.reg (full export of the services registry key).
