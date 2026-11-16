//! Plain-English descriptions for the Services and Startup Cleaner tabs (shown when a row is right-clicked):
//! what the item is, and what switching it off means. Written for people who are not IT experts.
//! Unknown items fall back to Windows' own description (services) or to what PC Tuner can tell from the
//! entry itself (startup items).

use serde::Serialize;

/// What a row's description panel shows.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct About {
    /// What it is, in plain words.
    pub what: String,
    /// What happens if it is switched off / disabled.
    pub off: String,
    /// Windows' own (often technical) description, when it adds something.
    pub windows: String,
}

/// Services PC Tuner knows, keyed by lower-case service name (per-user services by their base name).
const SERVICES: &[(&str, &str)] = &[
    // --- core Windows (locked)
    ("rpcss", "The switchboard that lets parts of Windows talk to each other. Almost everything depends on it."),
    ("rpceptmapper", "Tells programs where to find the Windows parts they want to talk to. Part of the same switchboard."),
    ("dcomlaunch", "Starts many of Windows' own background parts when they are needed."),
    ("lsm", "Keeps track of who is signed in and their sessions."),
    ("plugplay", "Notices when you plug a device in or out and sets it up."),
    ("power", "Manages power: sleep, wake, power plans and the power button."),
    ("profsvc", "Loads your user profile (desktop, settings, documents) when you sign in."),
    ("eventlog", "Keeps Windows' logbook of errors and events. PC Tuner's crash check reads it too."),
    ("brokerinfrastructure", "Coordinates background work for apps, so they only run when allowed."),
    ("samss", "Looks after the list of user accounts and passwords on this PC."),
    ("cryptsvc", "Checks that files and updates are genuine (digital signatures) and handles certificates."),
    ("winmgmt", "Lets programs ask Windows about the hardware and settings (WMI). PC Tuner uses it to read your PC."),
    ("coremessagingregistrar", "Passes messages between the parts of the Windows desktop."),
    ("staterepository", "Keeps the records of installed Store apps and the Start menu."),
    ("usermanager", "Manages signed-in users for the rest of Windows."),
    ("systemeventsbroker", "Wakes up background jobs at the right time (for example after a restart)."),
    ("gpsvc", "Applies Group Policy settings, including the ones PC Tuner sets on the Group Policy tab."),
    ("schedule", "Task Scheduler: runs jobs at set times or events. Many programs and PC Tuner's own sign-in tasks use it."),
    ("bfe", "The base of the Windows Firewall and network security. The internet stops working safely without it."),
    ("mpssvc", "Windows Firewall: blocks unwanted connections to your PC."),
    ("dhcp", "Gets your PC's network address from your router automatically."),
    ("dnscache", "Turns website names into addresses and remembers them, so web pages open faster."),
    ("nsi", "Keeps track of network connections for the rest of Windows."),
    ("windefend", "Microsoft Defender Antivirus: scans for and blocks viruses and malware."),
    ("securityhealthservice", "Runs the Windows Security app and its health checks."),
    ("wscsvc", "Watches your antivirus, firewall and update status and warns you if one is off."),
    ("mdcoresvc", "Part of Microsoft Defender's core protection."),
    ("wdnissvc", "Defender's network check: spots attacks coming over the network."),
    ("eventsystem", "Delivers notifications between Windows parts (for example 'the network is back')."),
    ("trustedinstaller", "Installs and removes Windows updates and features."),
    ("msiserver", "Windows Installer: installs, repairs and removes many programs (.msi setups)."),
    ("appidsvc", "Checks which apps are allowed to run, for security rules."),
    ("appinfo", "Shows the 'Do you want to allow this app to make changes?' prompt and runs apps as administrator."),
    ("camsvc", "Controls which apps may use your camera, microphone and location."),
    ("textinputmanagementservice", "Handles typing: keyboard layouts, the emoji panel and input methods."),
    ("dispbrokerdesktopsvc", "Manages display settings (resolution, scaling, multiple monitors)."),
    ("themes", "Draws Windows' look: colours, window styles and themes."),
    ("fontcache", "Remembers fonts so text appears quickly."),
    ("ngcsvc", "Windows Hello: sign in with a PIN, face or fingerprint."),
    ("ngcctnrsvc", "Stores the keys Windows Hello uses for your PIN and fingerprint."),
    ("vaultsvc", "Credential Manager: stores saved passwords for websites, network drives and apps."),
    ("keyiso", "Keeps encryption keys safe and separate. Also needed for some game and Store sign-ins."),
    ("lanmanworkstation", "Lets your PC open shared folders and network drives on other computers."),
    ("netman", "Manages network connections (the list in Network Connections)."),
    ("netprofm", "Decides whether a network is public or private and shows the network icon."),
    ("nlasvc", "Detects which network you are on and whether it has internet."),
    ("wcmsvc", "Picks which network connection to use (Wi-Fi, Ethernet, mobile)."),
    ("sppsvc", "Checks that Windows is activated (licensed)."),
    ("waasmedicsvc", "Repairs Windows Update if it gets broken."),
    ("deviceinstall", "Installs drivers when new hardware is plugged in."),
    ("dsmsvc", "Sets up devices after Plug and Play has found them."),
    ("audiosrv", "Windows Audio: without it there is no sound at all."),
    ("audioendpointbuilder", "Finds your speakers, headsets and microphones for Windows Audio."),
    ("sens", "Tells programs about system events like signing in or the network changing."),
    ("shellhwdetection", "Notices inserted USB sticks and discs (AutoPlay)."),
    // --- gaming dependencies
    ("gamingservices", "Xbox / Game Pass: installs and runs Xbox PC games. Game Pass games will not start without it."),
    ("gamingservicesnet", "Network part of Gaming Services, for Xbox PC games."),
    ("xblauthmanager", "Signs you in to Xbox Live for games and the Xbox app."),
    ("xblgamesave", "Syncs your Xbox game saves to the cloud."),
    ("xboxnetapisvc", "Xbox networking: multiplayer connections and the NAT check."),
    ("xboxgipsvc", "Makes Xbox accessories work (Xbox controllers, headsets, firmware updates)."),
    ("xboxservice", "Xbox app helper service."),
    ("gameinputsvc", "Reads controllers and other game input devices for modern games."),
    ("gameinputredistservice", "Installs and updates the GameInput controller support some games need."),
    ("bcastdvruserservice", "Game Bar recording and screenshots (Win+Alt+R, Win+Alt+PrtScn)."),
    ("steam client service", "Lets Steam install and update games without asking for administrator rights each time."),
    ("easyanticheat_eos", "Anti-cheat used by many online games (Epic Online Services). Those games will not start without it."),
    ("easyanticheat", "Anti-cheat used by many online games. Those games will not start without it."),
    ("beservice", "BattlEye anti-cheat, needed by games that use it (for example PUBG, Rainbow Six Siege)."),
    ("installservice", "Microsoft Store installs and updates apps and Game Pass games."),
    ("clipsvc", "Checks licences of Store apps and Game Pass games."),
    ("licensemanager", "Checks licences of Store apps and games."),
    ("wlidsvc", "Signs you in with your Microsoft account (Store, Xbox, OneDrive)."),
    ("tokenbroker", "Keeps you signed in to Microsoft accounts inside apps."),
    ("appxsvc", "Installs, updates and removes Store apps."),
    ("iphlpsvc", "IPv6 helper: some games and Xbox multiplayer use it to connect."),
    ("ikeext", "Sets up secure connections (VPN, Xbox Live party chat)."),
    ("winhttpautoproxysvc", "Finds proxy settings so apps can reach the internet."),
    ("dosvc", "Delivery Optimization: downloads Windows and Store updates (Game Pass downloads too)."),
    ("bits", "Downloads files in the background, gently, for Windows Update and many apps."),
    ("usosvc", "Plans and runs Windows Update."),
    ("wuauserv", "Windows Update: finds and downloads updates. Store and Game Pass installs use it too."),
    ("hidserv", "Makes extra buttons work on keyboards, mice and controllers (media keys, game pads)."),
    ("bthserv", "Bluetooth: needed for Bluetooth controllers, headsets and mice."),
    ("ssdpsrv", "Finds devices on your network (consoles, media players). Xbox networking uses it."),
    ("upnphost", "Lets network devices offer their features to your PC (UPnP)."),
    ("qwave", "Helps voice and video stay smooth on your home network."),
    ("wpnservice", "Delivers notifications (toasts) to apps, including game invites."),
    ("nvdisplay.containerlocalsystem", "NVIDIA's display helper: the NVIDIA app / Control Panel and some driver features need it."),
    ("amd3dvcachesvc", "AMD's helper that puts games on the CPU cores with the extra 3D V-Cache (X3D processors)."),
    ("coworkvmservicestore", "Helper service for a Store app's virtual machine features."),
    ("hvhost", "Hyper-V host features, also used by Windows' security features and some anti-cheats."),
    ("sharedaccess", "Internet Connection Sharing; also used by Xbox and Hyper-V networking."),
    ("dusmsvc", "Tracks data usage per network; Store / Xbox app downloads can hang at 'Pending' without it."),
    ("ncbservice", "Network connection helper for Xbox app notifications and invites."),
    ("timebrokersvc", "Schedules background tasks for Store and Xbox apps."),
    ("rmsvc", "Radio Management: Bluetooth on/off and airplane mode."),
    // --- the Safe preset
    ("diagtrack", "Connected User Experiences and Telemetry: sends usage and diagnostic data to Microsoft."),
    ("dmwappushservice", "Routes telemetry and device-management messages. Not needed on a home PC."),
    ("wuqisvc", "Collects Windows Update quality data for Microsoft."),
    ("retaildemo", "The shop-display demo mode for PCs in stores."),
    ("mapsbroker", "Downloads offline maps for the Maps app."),
    ("lfsvc", "Location: tells apps where you are (GPS / Wi-Fi location)."),
    ("wisvc", "Windows Insider: preview builds of Windows. Not needed unless you test them."),
    ("inventorysvc", "Collects a list of your installed apps for compatibility checks."),
    ("pcasvc", "Program Compatibility Assistant: pops up 'This program might not have installed correctly' messages."),
    ("trkwks", "Fixes shortcuts to files that moved on network drives. Rarely needed at home."),
    ("wersvc", "Windows Error Reporting: sends crash reports to Microsoft."),
    ("spooler", "Print Spooler: needed to print (also 'Print to PDF')."),
    ("printnotify", "Shows printer notifications and printer setup extensions."),
    ("printdeviceconfigurationservice", "Sets up newer printers automatically."),
    ("printscanbrokerservice", "Lets apps use printers and scanners safely."),
    ("stisvc", "Scanners and cameras that import pictures (Windows Image Acquisition)."),
    ("wiarpc", "Lets apps get pictures from scanners and cameras."),
    ("tapisrv", "Old telephone and modem support."),
    ("phonesvc", "Phone features for apps (calls through a linked phone)."),
    ("smsrouter", "Text messages through a linked phone or mobile connection."),
    ("termservice", "Remote Desktop: lets other computers connect to and control this PC."),
    ("sessionenv", "Remote Desktop session settings."),
    ("umrdpservice", "Lets Remote Desktop use your printers and drives."),
    ("winrm", "Remote management for IT departments (PowerShell remoting)."),
    ("remoteregistry", "Lets other computers change this PC's registry. A security risk at home."),
    ("webclient", "Opens web folders (WebDAV) in File Explorer. Rarely used."),
    ("rasauto", "Automatically dials old dial-up / VPN connections."),
    ("icssvc", "Mobile hotspot: shares this PC's internet over Wi-Fi."),
    ("wwansvc", "Mobile broadband (a SIM card / 4G modem in the PC)."),
    ("autotimesvc", "Sets the time zone from a mobile network."),
    ("wlansvc", "Wi-Fi. Needed if this PC uses Wi-Fi; not needed on a wired (Ethernet) connection."),
    ("peerdistsvc", "BranchCache: shares downloads between office PCs. For businesses."),
    ("msiscsi", "Connects to network storage over iSCSI. For servers and businesses."),
    ("lltdsvc", "Draws the network map in old Windows network tools."),
    ("ncdautosetup", "Automatically sets up network devices it finds (printers, TVs)."),
    ("wcncsvc", "Wi-Fi Protected Setup (the WPS button on routers)."),
    ("netlogon", "Signs in to a company network (domain). Not used at home."),
    ("cscservice", "Offline Files: copies of network files for when you are offline. For offices."),
    ("semgrsvc", "Tap-to-pay (NFC) payments."),
    ("walletservice", "Wallet / payment cards for apps."),
    ("wpcmonsvc", "Family Safety: parental controls and screen time."),
    ("wmpnetworksvc", "Shares your Windows Media Player library on the network."),
    ("fhsvc", "File History: automatic backups of your files to another drive."),
    ("sensorservice", "Sensors in tablets and laptops (rotation, light, motion)."),
    ("sensordataservice", "Delivers sensor readings to apps."),
    ("sensrsvc", "Turns sensors on and off for apps."),
    ("edgeupdate", "Keeps Microsoft Edge up to date. On Manual it still updates, just not at every start."),
    ("microsoftedgeelevationservice", "Lets Microsoft Edge do tasks that need administrator rights."),
    ("microsoftcopilotelevationservice", "Lets the Copilot app do tasks that need administrator rights."),
    ("storsvc", "Storage settings and app installs to other drives. Starts by itself when needed."),
    ("vmicguestinterface", "Hyper-V: used only when Windows itself runs inside a virtual machine."),
    ("vmicheartbeat", "Hyper-V: only used when Windows runs inside a virtual machine."),
    ("vmickvpexchange", "Hyper-V: only used when Windows runs inside a virtual machine."),
    ("vmicrdv", "Hyper-V: only used when Windows runs inside a virtual machine."),
    ("vmicshutdown", "Hyper-V: only used when Windows runs inside a virtual machine."),
    ("vmictimesync", "Hyper-V: only used when Windows runs inside a virtual machine."),
    ("vmicvmsession", "Hyper-V: only used when Windows runs inside a virtual machine."),
    ("vmicvss", "Hyper-V: only used when Windows runs inside a virtual machine."),
    // --- the Optional preset
    ("wsearch", "Windows Search: indexes your files so Start and File Explorer find them instantly. Without it searching is slower."),
    ("sysmain", "SysMain (Superfetch): preloads apps you use often. Little benefit on fast SSDs, some disk activity."),
    ("lanmanserver", "Shares this PC's folders and printers with other computers on your network."),
    ("dps", "Diagnostic Policy Service: troubleshooters and automatic problem detection."),
    ("wdiservicehost", "Part of Windows' problem detection (troubleshooters)."),
    ("wdisystemhost", "Part of Windows' problem detection (troubleshooters)."),
    ("wsaifabricsvc", "Runs Windows' built-in AI features."),
    ("cdpsvc", "Connected Devices: links your PC with your phone and other devices."),
    ("lmhosts", "Finds old devices on your local network by name (NetBIOS). Websites keep working without it."),
    ("logi_lamparray_service", "Logitech lighting helper for Windows' Dynamic Lighting."),
    ("wbiosrvc", "Fingerprint and face sign-in hardware (Windows Hello biometrics)."),
    ("scardsvr", "Smart card readers (company ID cards)."),
    ("scdeviceenum", "Finds smart card readers."),
    ("scpolicysvc", "Locks the PC when a smart card is removed. For offices."),
    ("certpropsvc", "Copies certificates from smart cards. For offices."),
    ("whesvc", "Windows Health: tracks slowdowns and shows 'your PC is slow' tips."),
    // --- other common services
    ("spectrum", "Windows Mixed Reality (VR headsets) spatial features."),
    ("sstpsvc", "Secure VPN connections (SSTP)."),
    ("w32time", "Keeps the PC's clock correct by checking an internet time server."),
    ("wbengine", "Windows Backup's engine (system image backups)."),
    ("vss", "Volume Shadow Copy: needed for System Restore points and some backups."),
    ("swprv", "Helps Volume Shadow Copy make restore points."),
    ("defragsvc", "Optimise Drives: trims SSDs and defragments hard disks on a schedule."),
    ("diagsvc", "Runs Windows diagnostics when you ask for them."),
    ("displayenhancementservice", "Automatic brightness and colour features of built-in displays."),
    ("hvsics", "Hyper-V virtual machine features."),
    ("vmcompute", "Runs Hyper-V virtual machines and Windows Sandbox."),
    ("vmms", "Hyper-V Manager's virtual machines."),
    ("wslservice", "Windows Subsystem for Linux."),
    ("cbdhsvc", "Clipboard history (Win+V) and clipboard sync."),
    ("onesyncsvc", "Syncs mail, contacts and calendar for Windows apps."),
    ("pimindexmaintenancesvc", "Indexes contacts so apps can search them."),
    ("unistoresvc", "Stores mail, contacts and calendar data for apps."),
    ("userdatasvc", "Lets apps read your contacts and calendar."),
    ("messagingservice", "Text messaging features for apps."),
    ("bluetoothuserservice", "Bluetooth features for your user account."),
    ("btagservice", "Bluetooth audio: needed for Bluetooth headphones and headsets."),
    ("devicesflowusersvc", "Shows 'Connect a device' pop-ups."),
    ("deviceassociationservice", "Pairs wireless devices (Bluetooth, network devices)."),
    ("printworkflowusersvc", "Print workflow for some printers."),
    ("captureservice", "Screen capture for apps (Snipping Tool, Game Bar)."),
    ("udkusersvc", "Draws parts of the Windows 11 desktop (Start, taskbar)."),
    ("webthreatdefsvc", "Phishing protection: warns when you type your password into a fake site."),
    ("webthreatdefusersvc", "Phishing protection for your user account."),
    ("nvcontainerlocalsystem", "NVIDIA helper service (the NVIDIA app, ShadowPlay recording)."),
    ("rtkauduservice", "Realtek audio control panel helper."),
    ("esrv_svc_queencreek", "Intel driver and support assistant telemetry."),
    ("armourycrateservice", "ASUS Armoury Crate (RGB, fan and driver updates)."),
    ("lightingservice", "ASUS Aura lighting control."),
    ("gameinput", "Game controller input service."),
    ("aswbidsagent", "Avast / AVG antivirus helper."),
    ("teamviewer", "TeamViewer: lets you (or someone else) control this PC remotely."),
    ("anydesk", "AnyDesk: lets you (or someone else) control this PC remotely."),
    ("googleupdaterservice", "Keeps Google Chrome and other Google apps up to date."),
    ("googlechromeelevationservice", "Lets Chrome do tasks that need administrator rights."),
    ("mozillamaintenance", "Keeps Firefox up to date without asking for administrator rights."),
    ("adobearmservice", "Keeps Adobe Acrobat Reader up to date."),
    ("aguaservice", "Adobe Creative Cloud helper (updates and licences)."),
    ("dropbox", "Keeps Dropbox up to date."),
    ("onedrive updater service", "Keeps OneDrive up to date."),
    ("razer game manager service", "Razer Synapse / Cortex game helper."),
    ("corsair.service", "Corsair iCUE (RGB lighting, fans and devices)."),
    ("logi_lamparray", "Logitech lighting helper."),
    ("lghub_updater", "Keeps Logitech G HUB up to date."),
    ("epiconlinservices", "Epic Games online services helper."),
    ("battlenet", "Battle.net helper for Blizzard games."),
];

/// Startup items PC Tuner recognises: (lower-case text found in the name or command, description).
const STARTUP: &[(&[&str], &str)] = &[
    (&["7-zip", "7zip"], "7-Zip's right-click menu in File Explorer (Extract here, Add to archive)."),
    (&["winrar"], "WinRAR's right-click menu in File Explorer (extract and create archives)."),
    (&["onedrive"], "OneDrive: syncs your files with Microsoft's cloud. Off: files stop syncing until you open OneDrive."),
    (&["msedge", "microsoft edge"], "Microsoft Edge preloading itself in the background so it opens faster. Off: Edge opens a moment slower."),
    (&["edgeupdate", "microsoftedgeupdate"], "Keeps Microsoft Edge up to date. Off: Edge still updates when you use it."),
    (&["teams"], "Microsoft Teams chat / meetings. Off: Teams only starts when you open it."),
    (&["discord"], "Discord chat and voice. Off: you are not shown online until you open it."),
    (&["steam"], "Steam opens at sign-in so updates and friends are ready. Off: Steam starts when you open it or a game."),
    (&["epicgameslauncher", "epic games"], "Epic Games Launcher. Off: it starts when you open it."),
    (&["battle.net", "battlenet"], "Blizzard Battle.net launcher. Off: it starts when you open it."),
    (&["eadesktop", "ea desktop", "origin"], "EA app (games launcher). Off: it starts when you open it."),
    (&["ubisoft", "upc.exe", "uplay"], "Ubisoft Connect launcher. Off: it starts when you open it."),
    (&["goggalaxy", "gog galaxy"], "GOG Galaxy launcher. Off: it starts when you open it."),
    (&["spotify"], "Spotify music. Off: it opens only when you start it."),
    (&["nvidia", "nvcontainer", "nvbackend"], "NVIDIA app / driver helper (overlay, recording, driver updates). Off: those extras start when you open the NVIDIA app."),
    (&["securityhealth"], "Windows Security's tray icon. Off: the icon goes, protection keeps running."),
    (&["rtkaud", "realtek"], "Realtek audio control panel. Off: sound keeps working; the panel's jack pop-ups and effects may not."),
    (&["armoury", "asus"], "ASUS software (Armoury Crate, lighting, updates)."),
    (&["icue", "corsair"], "Corsair iCUE: RGB lighting, fans and device settings. Off: devices fall back to their saved/default lighting."),
    (&["lghub", "logitech"], "Logitech G HUB: mouse / keyboard settings and lighting. Off: devices use the settings saved on them."),
    (&["razer", "synapse"], "Razer Synapse: Razer device settings and lighting. Off: devices use their saved settings."),
    (&["signalrgb"], "SignalRGB lighting control."),
    (&["openrgb"], "OpenRGB lighting control."),
    (&["gigabyte", "rgbfusion", "gcc.exe", "control center"], "Gigabyte Control Center / RGB Fusion: lighting, fan and update tools for Gigabyte hardware."),
    (&["msi center", "msicenter", "dragon center", "mystic light"], "MSI Center: lighting, fan and update tools for MSI hardware."),
    (&["afterburner"], "MSI Afterburner: GPU overclocking / fan tool. PC Tuner's RTX 5080 tab does this without it."),
    (&["rtss", "rivatuner"], "RivaTuner Statistics Server: on-screen FPS counter and frame limiter."),
    (&["nv-uv-play"], "NV-UV Play: GPU undervolt tool. PC Tuner's RTX 5080 tab uses its profiles without it."),
    (&["dropbox"], "Dropbox: syncs files with the cloud. Off: files stop syncing until you open it."),
    (&["googledrive", "google drive"], "Google Drive: syncs files with the cloud."),
    (&["icloud"], "Apple iCloud: syncs photos and files with Apple devices."),
    (&["googleupdate", "googleupdater"], "Keeps Google Chrome up to date. Off: Chrome still updates when you use it."),
    (&["chrome"], "Google Chrome running in the background (for apps and notifications)."),
    (&["adobe", "acrobat", "creative cloud"], "Adobe software helper (updates, licences, Creative Cloud)."),
    (&["zoom"], "Zoom meetings. Off: Zoom starts when you open it."),
    (&["skype"], "Skype calls and chat."),
    (&["whatsapp"], "WhatsApp desktop."),
    (&["telegram"], "Telegram desktop."),
    (&["slack"], "Slack chat."),
    (&["phonelink", "yourphone"], "Phone Link: your phone's messages and calls on the PC."),
    (&["copilot"], "Microsoft Copilot assistant."),
    (&["widgets"], "Windows Widgets board (news and weather)."),
    (&["cortana"], "Cortana assistant (mostly retired by Microsoft)."),
    (&["xbox", "gamebar"], "Xbox app / Game Bar helper."),
    (&["wallpaperengine", "wallpaper engine"], "Wallpaper Engine: animated desktop wallpapers."),
    (&["obs"], "OBS Studio: streaming and recording."),
    (&["medal"], "Medal.tv: game clip recording."),
    (&["overwolf"], "Overwolf: in-game app overlays."),
    (&["ccleaner"], "CCleaner monitoring / cleaning helper."),
    (&["teamviewer"], "TeamViewer remote control."),
    (&["anydesk"], "AnyDesk remote control."),
    (&["parsec"], "Parsec remote play / desktop streaming."),
    (&["vmware"], "VMware virtual machine helper."),
    (&["virtualbox"], "VirtualBox virtual machine helper."),
    (&["docker"], "Docker Desktop (developer containers). Uses a lot of memory while running."),
    (&["pctuner"], "PC Tuner itself (its sign-in task re-applies your GPU tune and checks for crashes)."),
    (&["defender", "msmpeng"], "Microsoft Defender antivirus."),
    (&["malwarebytes", "mbam"], "Malwarebytes anti-malware."),
    (&["avast", "avg"], "Avast / AVG antivirus."),
    (&["norton"], "Norton antivirus."),
    (&["mcafee"], "McAfee antivirus."),
    (&["bitdefender"], "Bitdefender antivirus."),
    (&["intel"], "Intel driver or support helper."),
    (&["amd", "radeon"], "AMD software / driver helper."),
    (&["javaupdate", "jusched"], "Java update checker."),
    (&["itunes", "ituneshelper"], "iTunes helper (for iPhones and iPods)."),
];

pub fn lc(s: &str) -> String {
    s.to_lowercase()
}

/// Per-user services carry a suffix ("cbdhsvc_70560"): the known text is under the base name.
fn base(name: &str) -> &str {
    match name.rsplit_once('_') {
        Some((b, suffix)) if !b.is_empty() && suffix.len() >= 4 && suffix.chars().all(|c| c.is_ascii_hexdigit()) => b,
        _ => name,
    }
}

/// Our own text for a service, if PC Tuner knows it.
pub fn service_text(name: &str) -> Option<&'static str> {
    let n = lc(name);
    let b = lc(base(name));
    SERVICES.iter().find(|(k, _)| *k == n || *k == b).map(|(_, t)| *t)
}

/// The first sentence or two of Windows' description, without its ending clutter.
pub fn short_windows_text(desc: &str) -> String {
    let d = desc.trim();
    if d.is_empty() || d.starts_with('@') {
        // an unresolved resource string ("@%SystemRoot%\...") is no use to anyone
        return String::new();
    }
    let mut out = String::new();
    for (i, s) in d.split_inclusive(". ").enumerate() {
        if i >= 2 || out.len() > 220 {
            break;
        }
        out.push_str(s);
    }
    out.trim().to_string()
}

/// A service's description: `note` is the row's note (locked / gaming / per-user), `suggested` the start type
/// PC Tuner's presets would give it, `desc` Windows' own description.
pub fn service(name: &str, display: &str, desc: &str, note: &str, suggested: Option<&str>) -> About {
    let windows = short_windows_text(desc);
    let ours = service_text(name);
    let what = match ours {
        Some(t) => t.to_string(),
        None if !windows.is_empty() => windows.clone(),
        None => format!("{display}: Windows gives no description for this service. If you do not recognise it, look up its name before changing it."),
    };
    let off = if note == crate::services::NOTE_LOCKED {
        "Core Windows: switching it off can stop Windows from starting or signing in. PC Tuner never changes it.".to_string()
    } else if note == crate::services::NOTE_GAMING {
        "Games, Xbox / Game Pass, Steam, anti-cheat or controllers depend on it - leave it as it is.".to_string()
    } else {
        match suggested {
            Some("Disabled") => "Safe to disable if you do not use this feature; Undo puts it back.".to_string(),
            Some("Manual") => "Safe on Manual: Windows still starts it when something needs it.".to_string(),
            _ if note == crate::services::NOTE_PER_USER => "A copy of a service that runs for each signed-in user. Change the main service instead.".to_string(),
            _ => "PC Tuner has no advice for this one. Manual is usually safe (it starts when needed); only disable it if you know what it does.".to_string(),
        }
    };
    // Windows' text is shown too when ours replaced it and it says something different
    let windows = if ours.is_some() { windows } else { String::new() };
    About { what, off, windows }
}

/// What PC Tuner recognises a startup entry as (by its name or command), if anything.
pub fn startup_text(name: &str, command: &str) -> Option<&'static str> {
    let hay = lc(&format!("{name} {command}"));
    STARTUP.iter().find(|(keys, _)| keys.iter().any(|k| hay.contains(k))).map(|(_, t)| *t)
}

/// The program a startup command runs ("C:\Program Files\X\x.exe" -> "x.exe").
pub fn program_of(command: &str) -> String {
    let c = command.trim();
    let path = if let Some(rest) = c.strip_prefix('"') { rest.split('"').next().unwrap_or(rest) } else { c.split(" -").next().unwrap_or(c).split(" /").next().unwrap_or(c) };
    path.rsplit(['\\', '/']).next().unwrap_or(path).trim().to_string()
}

/// A startup item's description. `kind` is the entry type as shown ("Logon (Run key)", "Scheduled task", ...).
pub fn startup(name: &str, command: &str, publisher: &str, kind: &str, category: &str, advice: &str) -> About {
    let known = startup_text(name, command);
    let prog = program_of(command);
    let when = match kind {
        k if k.starts_with("Logon") => "It starts every time you sign in to Windows.",
        "Scheduled task" => "A scheduled task: Windows runs it at set times or events (for example at sign-in or once a day).",
        "Store app startup" => "A Store app that is allowed to start when you sign in.",
        "Explorer extension" => "An add-on loaded into File Explorer (for example extra right-click menu items).",
        "Windows setting" => "A Windows setting that makes something start or run in the background.",
        _ => "It starts automatically.",
    };
    let who = match (publisher.trim(), prog.is_empty()) {
        ("", true) => String::new(),
        ("", false) => format!(" It runs {prog}."),
        (p, true) => format!(" Made by {p}."),
        (p, false) => format!(" It runs {prog}, made by {p}."),
    };
    // a known item's text may end in "Off: ..." - that part belongs with the switching-off line
    let (known_what, known_off) = match known.map(|t| t.split_once(" Off: ")) {
        Some(Some((w, o))) => {
            let mut c = o.chars();
            let o = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
            (Some(w.to_string()), Some(o))
        }
        _ => (known.map(str::to_string), None),
    };
    let what = match known_what {
        Some(t) => format!("{t} {when}"),
        None => format!("{name}. {when}{who}"),
    };
    let off = match category {
        "Junk" => "Safe to switch off: Windows or the program works without it. Switching off never deletes anything and can be undone.".to_string(),
        "Keep" => "Better left on - see the advice. Switching off never deletes anything and can be undone.".to_string(),
        "Windows" => "Part of Windows. Only switch it off if you know you do not need it; it can be undone.".to_string(),
        _ if kind == "Explorer extension" => "Switching it off removes its extra menu items or icons from File Explorer; the program itself keeps working, nothing is deleted, and it can be undone.".to_string(),
        _ => "Switching it off only stops it starting by itself: you can still open the program yourself, nothing is deleted, and it can be undone.".to_string(),
    };
    let off = match known_off {
        Some(o) => format!("{o} {off}"),
        None => off,
    };
    let off = if advice.is_empty() { off } else { format!("{off} PC Tuner's advice: {advice}.") };
    About { what, off, windows: String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explain_services() {
        // known, per-user suffix, case
        assert!(service_text("Spooler").unwrap().contains("print"));
        assert!(service_text("cbdhsvc_70560").unwrap().contains("Clipboard history"));
        assert_eq!(service_text("NoSuchService"), None);
        let a = service("Spooler", "Print Spooler", "This service spools print jobs and handles interaction with the printer. If you turn off this service, you won't be able to print.", "", Some("Disabled"));
        assert!(a.what.starts_with("Print Spooler") && a.off.contains("Safe to disable") && a.windows.contains("spools print jobs"));
        // unknown: Windows' text, shortened
        let u = service("FooSvc", "Foo", "Does foo. Also bar. And a third sentence that is dropped.", "", None);
        assert_eq!(u.what, "Does foo. Also bar.");
        assert!(u.windows.is_empty() && u.off.contains("no advice"));
        // nothing at all, or an unresolved resource string
        assert!(service("FooSvc", "Foo", "@%SystemRoot%\\x.dll,-1", "", None).what.contains("gives no description"));
        // locked and gaming notes win over presets
        assert!(service("RpcSs", "RPC", "", crate::services::NOTE_LOCKED, None).off.contains("never changes it"));
        assert!(service("XblAuthManager", "Xbox", "", crate::services::NOTE_GAMING, None).off.contains("leave it"));
        // every service in PC Tuner's own lists has a description
        for n in crate::services::GAMING.iter().chain(crate::services::CRITICAL).chain(crate::services::PRESET_SAFE.iter().map(|p| &p.0)).chain(crate::services::PRESET_OPTIONAL.iter().map(|p| &p.0)) {
            assert!(service_text(n).is_some(), "no description for {n}");
        }
    }

    #[test]
    fn explain_startup() {
        assert_eq!(program_of(r#""C:\Program Files\Discord\Update.exe" --processStart Discord.exe"#), "Update.exe");
        assert_eq!(program_of(r"C:\Windows\system32\SecurityHealthSystray.exe"), "SecurityHealthSystray.exe");
        assert_eq!(program_of(r"C:\Tools\x.exe /silent"), "x.exe");
        let d = startup("Discord", r#""C:\Users\e\AppData\Local\Discord\Update.exe" --processStart Discord.exe"#, "Discord Inc.", "Logon (Run key)", "Optional", "");
        assert_eq!(d.what, "Discord chat and voice. It starts every time you sign in to Windows.");
        assert!(d.off.starts_with("You are not shown online until you open it. Switching it off only stops it") && d.off.contains("can be undone"));
        let u = startup("Mystery", r"C:\x\mystery.exe", "Acme", "Scheduled task", "Review", "Review - starts with Windows");
        assert!(u.what.contains("It runs mystery.exe, made by Acme") && u.what.contains("scheduled task"));
        assert!(u.off.contains("PC Tuner's advice: Review - starts with Windows."));
        assert!(startup("x", "y", "", "Logon (Run key)", "Junk", "").off.starts_with("Safe to switch off"));
        let z = startup("7-Zip", r"C:Program Files-Zip-zip.dll", "Igor Pavlov", "Explorer extension", "Review", "");
        assert!(z.what.starts_with("7-Zip's right-click menu") && z.off.contains("removes its extra menu items"));
    }
}
