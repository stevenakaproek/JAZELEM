SERVICE MANAGER - after-reinstall toolkit
==========================================

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
Undo:  .\Remove-Bloat.ps1 -Restore   (reinstalls from the Store, reverts
  registry/policy changes, removes the task). After it: set the Copilot key in
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
Remove Bloat, Post-install Tweaks, CPU Scheduler, RTX 5080 Profiles, USB Port
Finder - same lists, rules, safety checks and undo as the separate tools; it
also imports their change history. Its own history: %LOCALAPPDATA%\PCTuner.
Source code: pctuner\ (Rust). Rebuild: install Rust (rustup) + Visual Studio
2022 Build Tools (C++), then in pctuner\ run:  cargo build --release

How it works: every change opens a window that first shows exactly what will
change (with a warning if it is risky), then a progress bar, then each item
ticking off with a count and an Undo button. Nothing is applied until you
press the green button in that window.

Fan Tuner tab:
- Needs the PawnIO driver (already installed on this PC) and PC Tuner run as
  administrator. Without admin only the graphics card is shown and nothing
  can be changed.
- Live RPM / duty per fan, CPU / GPU / board temperatures, graphs.
- Per fan: BIOS (hands off), Manual (slider) or Curve (drag the points;
  follows the CPU, the GPU or the hotter of the two). Headers the library
  cannot control are shown as "read-only"; headers that never spin (nothing
  plugged in) are folded away under "headers at 0 RPM". On the MSI X870E
  Tomahawk every header, SYS fans included, is controllable (tested).
- Profiles: Silent / Balanced / Performance (made for your fans the first time
  you pick one; "Reset to default" undoes your edits) and your own ("Save as
  new"). The last profile you picked is applied when PC Tuner starts.
  Saved in %LOCALAPPDATA%\PCTuner\fan-profiles.json.
- Safety (inside the PCTunerFan helper, so it holds if the window freezes):
  never below 20 % (pumps 50 %, GPU fan 30 % = its driver minimum); CPU or
  GPU at 90 C -> every controlled fan 100 % until both are under 80 C; if
  temperatures cannot be read for 5 s the fans go back to the BIOS; fans go
  back to the BIOS when PC Tuner closes, crashes, or stops talking for 10 s.
- Limits: if both PC Tuner and its helper are force-killed, fans stay at
  their last speed (never under the floor) until PC Tuner is opened again or
  the PC restarts. GPU fans: turn off MSI Afterburner's fan control first.
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
  a crash or blue screen since the tune was applied, the GPU is set back to
  Stock and PC Tuner tells you why next time it opens.
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
