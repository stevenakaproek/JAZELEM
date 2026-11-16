import type { Action, BackupsView, HistoryRow, RowTone, ServiceRow, SortCol, View } from "../tabs/Services";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import type { MockTab } from "./types";

const GAMING = "Gaming dependency";
const LOCKED = "Core Windows (locked)";
const PER_USER = "Per-user service";

type Seed = [name: string, display: string, start: string, state: string, note?: string, suggested?: string];

const seeds: Seed[] = [
  ["amd3dvcacheSvc", "AMD 3D V-Cache Performance Optimizer Service", "Automatic", "Running", GAMING],
  ["AppXSvc", "AppX Deployment Service (AppXSVC)", "Manual", "Running", GAMING],
  ["AudioEndpointBuilder", "Windows Audio Endpoint Builder", "Automatic", "Running", LOCKED],
  ["Audiosrv", "Windows Audio", "Automatic", "Running", LOCKED],
  ["BFE", "Base Filtering Engine", "Automatic", "Running", LOCKED],
  ["BITS", "Background Intelligent Transfer Service", "Manual", "Stopped", GAMING],
  ["bthserv", "Bluetooth Support Service", "Manual", "Running", GAMING],
  ["CDPSvc", "Connected Devices Platform Service", "Automatic (Delayed)", "Running", "", "Manual"],
  ["CDPUserSvc_5fec7", "Connected Devices Platform User Service_5fec7", "Automatic", "Running", PER_USER],
  ["ClipSVC", "Client License Service (ClipSVC)", "Manual", "Stopped", GAMING],
  ["CryptSvc", "Cryptographic Services", "Automatic", "Running", LOCKED],
  ["DcomLaunch", "DCOM Server Process Launcher", "Automatic", "Running", LOCKED],
  ["Dhcp", "DHCP Client", "Automatic", "Running", LOCKED],
  ["DiagTrack", "Connected User Experiences and Telemetry", "Disabled", "Stopped", "", "Disabled"],
  ["Dnscache", "DNS Client", "Automatic", "Running", LOCKED],
  ["DoSvc", "Delivery Optimization", "Automatic (Delayed)", "Running", GAMING],
  ["DPS", "Diagnostic Policy Service", "Automatic", "Running", "", "Disabled"],
  ["EasyAntiCheat_EOS", "Easy Anti-Cheat (Epic Online Services)", "Manual", "Stopped", GAMING],
  ["edgeupdate", "Microsoft Edge Update Service (edgeupdate)", "Automatic (Delayed)", "Stopped", "", "Manual"],
  ["EventLog", "Windows Event Log", "Automatic", "Running", LOCKED],
  ["GameInputSvc", "GameInput Service", "Manual", "Running", GAMING],
  ["GamingServices", "Gaming Services", "Automatic", "Running", GAMING],
  ["hidserv", "Human Interface Device Service", "Manual", "Running", GAMING],
  ["LanmanServer", "Server", "Automatic", "Running", "", "Manual"],
  ["LanmanWorkstation", "Workstation", "Automatic", "Running", LOCKED],
  ["lfsvc", "Geolocation Service", "Disabled", "Stopped", "", "Disabled"],
  ["LGHUBUpdaterService", "LGHUB Updater Service", "Automatic", "Running"],
  ["MapsBroker", "Downloaded Maps Manager", "Automatic (Delayed)", "Stopped", "", "Disabled"],
  ["MSIService", "MSI Center Service", "Automatic", "Running"],
  ["NVDisplay.ContainerLocalSystem", "NVIDIA Display Container LS", "Automatic", "Running", GAMING],
  ["PcaSvc", "Program Compatibility Assistant Service", "Automatic (Delayed)", "Running", "", "Disabled"],
  ["PhoneSvc", "Phone Service", "Manual", "Stopped", "", "Disabled"],
  ["PlugPlay", "Plug and Play", "Manual", "Running", LOCKED],
  ["RemoteRegistry", "Remote Registry", "Disabled", "Stopped", "", "Disabled"],
  ["RpcSs", "Remote Procedure Call (RPC)", "Automatic", "Running", LOCKED],
  ["Spooler", "Print Spooler", "Automatic", "Running", "", "Disabled"],
  ["Steam Client Service", "Steam Client Service", "Manual", "Stopped", GAMING],
  ["StorSvc", "Storage Service", "Automatic (Delayed)", "Running", "", "Manual"],
  ["SysMain", "SysMain", "Automatic", "Running", "", "Manual"],
  ["TermService", "Remote Desktop Services", "Manual", "Stopped", "", "Disabled"],
  ["Themes", "Themes", "Automatic", "Running", LOCKED],
  ["TrkWks", "Distributed Link Tracking Client", "Automatic", "Running", "", "Disabled"],
  ["tzautoupdate", "Auto Time Zone Updater", "Disabled", "Stopped"],
  ["W32Time", "Windows Time", "Manual", "Stopped"],
  ["WerSvc", "Windows Error Reporting Service", "Manual", "Stopped", "", "Manual"],
  ["WinDefend", "Microsoft Defender Antivirus Service", "Automatic", "Running", LOCKED],
  ["WlanSvc", "WLAN AutoConfig", "Automatic", "Running", "", "Manual"],
  ["WSearch", "Windows Search", "Automatic (Delayed)", "Running", "", "Disabled"],
  ["wuauserv", "Windows Update", "Manual", "Stopped", GAMING],
  ["XblAuthManager", "Xbox Live Auth Manager", "Manual", "Running", GAMING],
  ["XboxGipSvc", "Xbox Accessory Management Service", "Manual", "Stopped", GAMING],
];

interface Svc {
  name: string;
  display: string;
  start: string;
  state: string;
  note: string;
  suggested: string | null;
}

const mode = mockParams.get("state");
const list: Svc[] = mode === "loading" || mode === "error" || mode === "empty" ? [] : seeds.map(([name, display, start, state, note = "", suggested]) => ({ name, display, start, state, note, suggested: suggested ?? null }));

const selected = new Set<string>();
let query = "";
let runningOnly = false;
let hideLocked = false;
let stopOnDisable = true;
let sort: SortCol = "display";
let sortAsc = true;
let lastAction = "";
let showHistory = mockParams.get("sheet") === "history";
let backupsOpen = mockParams.get("sheet") === "backups";
let backupMsg: BackupsView["msg"] = null;
let creating = 0;

const history: Omit<HistoryRow, "index" | "selected">[] = [
  { time: "2026-10-02 19:12:40", name: "DiagTrack", display: "Connected User Experiences and Telemetry", from: "Automatic", to: "Disabled", undone: false },
  { time: "2026-10-02 19:12:41", name: "lfsvc", display: "Geolocation Service", from: "Manual", to: "Disabled", undone: false },
  { time: "2026-10-02 19:12:41", name: "RemoteRegistry", display: "Remote Registry", from: "Manual", to: "Disabled", undone: false },
  { time: "2026-10-20 21:03:09", name: "WSearch", display: "Windows Search", from: "Automatic (Delayed)", to: "Disabled", undone: true },
];
const histSel = new Set<number>();
const backups = [
  { time: "2026-11-02 18:40:11", detail: "291 services - made by you", diffs: 0 },
  { time: "2026-10-02 19:12:38", detail: "291 services - automatic, before the first change in PC Tuner", diffs: 3 },
];

const op = new MockOp();
const backupOp = new MockOp();

function tone(s: Svc): RowTone {
  if (s.note === GAMING) return "gaming";
  if (s.note === LOCKED) return "locked";
  if (s.suggested) return "safe";
  return s.start === "Disabled" ? "disabled" : "";
}

function visible(): Svc[] {
  const q = query.trim().toLowerCase();
  const key = (s: Svc): string => {
    switch (sort) {
      case "name":
        return s.name.toLowerCase();
      case "startup":
        return s.start;
      case "status":
        return s.state;
      case "note":
        return s.note;
      case "suggested":
        return s.suggested ? `0${s.suggested}` : "1";
      default:
        return s.display.toLowerCase();
    }
  };
  const rows = list.filter((s) => (!q || s.name.toLowerCase().includes(q) || s.display.toLowerCase().includes(q)) && (!runningOnly || s.state === "Running") && (!hideLocked || s.note !== LOCKED));
  rows.sort((a, b) => (key(a).localeCompare(key(b)) || a.display.toLowerCase().localeCompare(b.display.toLowerCase())) * (sortAsc ? 1 : -1));
  return rows;
}

function change(ty: "Disabled" | "Manual" | "Automatic") {
  const picked = visible().filter((s) => selected.has(s.name));
  const locked = picked.filter((s) => s.note === LOCKED).map((s) => s.name);
  const names = picked.filter((s) => s.note !== LOCKED);
  const work = names.filter((s) => s.start !== ty && !(ty === "Automatic" && s.start === "Automatic (Delayed)"));
  const gaming = names.filter((s) => s.note === GAMING).map((s) => s.name);
  const warn: string[] = [];
  if (gaming.length && ty !== "Automatic") warn.push(`These are needed by Xbox / Game Pass / Steam / anti-cheat: ${gaming.join(", ")}. Set them to ${ty} anyway?`);
  if (locked.length) warn.push(`These are core Windows services and are locked (they will be skipped): ${locked.join(", ")}`);
  let text = ty === "Disabled" ? "They will not be able to start at all." : ty === "Manual" ? "They will start only when something needs them." : "They will start together with Windows.";
  if (ty === "Disabled" && stopOnDisable) text += " Running ones are stopped right away.";
  op.open(
    makeOp(
      "Services",
      "change",
      "changed",
      work.map((s) => opItem(s.name, s.name, `Set to ${ty}`, `${s.start} -> ${ty}`)),
      {
        noun: ["service", "services"],
        preview_text: text,
        go_label: `Set to ${ty}`,
        running_text: "Changing services...",
        reassure: "Every change is recorded - 'Undo last change' puts it back.",
        undo_label: "Undo these changes",
        warning: warn.length ? warn.join("\n") : null,
        stage: work.length ? "Preview" : "Done",
        empty_title: names.length ? "Nothing to change" : "Nothing was changed",
        empty_text: names.length ? `Everything you selected is already set to ${ty}.` : `These are core Windows services and cannot be changed: ${locked.join(", ")}`,
      },
    ),
    () => {
      for (const s of work) {
        history.push({ time: "2026-11-16 20:15:02", name: s.name, display: s.display, from: s.start, to: ty, undone: false });
        s.start = ty;
        if (ty === "Disabled" && stopOnDisable) s.state = "Stopped";
      }
      lastAction = `${work.length} set to ${ty}.`;
    },
  );
}

function undo(indices: number[]) {
  const entries = indices.filter((i) => !history[i].undone);
  op.open(
    makeOp(
      "Undo",
      "restore",
      "restored",
      entries.map((i) => opItem(String(i), history[i].name, `Back to ${history[i].from}`, `${history[i].to} -> ${history[i].from}`)),
      {
        noun: ["service", "services"],
        preview_text: "Each service goes back to the startup type it had before the change.",
        reassure: "Services that were running before are started again.",
        go_label: "Undo",
        running_text: "Restoring services...",
        stage: entries.length ? "Preview" : "Done",
        empty_title: "Nothing to undo",
        empty_text: "These changes were already undone.",
      },
    ),
    () => {
      for (const i of entries) {
        history[i].undone = true;
        const s = list.find((x) => x.name === history[i].name);
        if (s) s.start = history[i].from;
      }
      if (op.op) op.op.can_undo = false;
      lastAction = `Undid ${entries.length} change(s).`;
    },
  );
}

// a few of PC Tuner's plain-English descriptions (pctuner_core::explain)
const SVC_TEXT: Record<string, string> = {
  Spooler: "Print Spooler: needed to print (also 'Print to PDF').",
  DiagTrack: "Connected User Experiences and Telemetry: sends usage and diagnostic data to Microsoft.",
  XblAuthManager: "Signs you in to Xbox Live for games and the Xbox app.",
  RpcSs: "The switchboard that lets parts of Windows talk to each other. Almost everything depends on it.",
  WSearch: "Windows Search: indexes your files so Start and File Explorer find them instantly. Without it searching is slower.",
  SysMain: "SysMain (Superfetch): preloads apps you use often. Little benefit on fast SSDs, some disk activity.",
};

export const services: MockTab<View, Action> = {
  view() {
    const busy = op.op?.stage === "Running" || backupOp.op?.stage === "Running";
    const can = mockAdmin && !busy;
    const rows: ServiceRow[] = visible().map((s) => ({
      name: s.name,
      display: s.display,
      start: s.start,
      startDisabled: s.start === "Disabled",
      state: s.state,
      running: s.state === "Running",
      note: s.note,
      tone: tone(s),
      suggested: s.suggested,
      suggestedDone: s.suggested === s.start,
      selected: selected.has(s.name),
      about: {
        what: SVC_TEXT[s.name] ?? `${s.display}: a Windows service.`,
        off: s.note.startsWith("Core") ? "Core Windows: switching it off can stop Windows from starting or signing in. PC Tuner never changes it." : s.note.startsWith("Gaming") ? "Games, Xbox / Game Pass, Steam, anti-cheat or controllers depend on it - leave it as it is." : s.suggested === "Disabled" ? "Safe to disable if you do not use this feature; Undo puts it back." : s.suggested === "Manual" ? "Safe on Manual: Windows still starts it when something needs it." : "PC Tuner has no advice for this one. Manual is usually safe (it starts when needed); only disable it if you know what it does.",
        windows: s.name === "Spooler" ? "This service spools print jobs and handles interaction with the printer. If you turn off this service, you won't be able to print or see your printers." : "",
      },
    }));
    const vis = rows.filter((r) => r.selected).length;
    const total = selected.size;
    const open = list.filter((s) => s.suggested && s.suggested !== s.start).length;
    const active = history.filter((h) => !h.undone).length;
    return {
      loading: mode === "loading",
      refreshing: mode === "loading",
      busy,
      hero: {
        running: list.filter((s) => s.state === "Running").length,
        disabled: list.filter((s) => s.start === "Disabled").length,
        total: list.length,
        undoable: active,
        chip: list.length === 0 ? "Checking..." : open === 0 ? "Every green service is at its suggested type" : `${open} green services not at their suggested type`,
        chipOk: list.length > 0,
      },
      search: query,
      runningOnly,
      hideLocked,
      stopOnDisable,
      sort,
      sortAsc,
      rows,
      selText: total === 0 ? "Click rows to select" : vis === total ? `${vis} selected` : vis > 0 ? `${vis} selected (${total - vis} hidden by filter)` : `${total} selected, all hidden by filter`,
      selActive: total > 0 && vis > 0,
      canChange: can && vis > 0,
      canClear: total > 0,
      canUndo: can && active > 0,
      status: mode === "error" ? { text: "Could not load services: PowerShell returned no data (exit code 1).", error: true } : lastAction ? { text: lastAction, error: false } : null,
      op: op.op,
      opCan: can,
      history:
        showHistory && !op.op && !backupsOpen
          ? {
              active,
              recorded: history.length,
              rows: history.map((h, index) => ({ ...h, index, selected: histSel.has(index) })).reverse(),
              undoLabel: histSel.size > 0 ? `Undo selected (${histSel.size})` : "Undo selected",
              canUndo: can && histSel.size > 0,
              canClear: !busy,
              busy,
            }
          : null,
      backups: backupsOpen
        ? {
            msg: backupMsg,
            canCreate: mockAdmin && creating === 0,
            creating: creating > 0,
            list: backups.map((b, index) => ({ index, time: b.time, detail: b.detail, matches: b.diffs === 0, diff: b.diffs === 0 ? "matches the PC now" : `${b.diffs} differ from now`, canRestore: mockAdmin && !busy })),
            op: backupOp.op,
            opCan: mockAdmin && backupOp.op?.stage !== "Running",
          }
        : null,
    };
  },
  act(a) {
    switch (a.kind) {
      case "toggle":
        if (!selected.delete(a.name)) selected.add(a.name);
        break;
      case "clear":
        selected.clear();
        break;
      case "search":
        query = a.text;
        break;
      case "running_only":
        runningOnly = a.on;
        break;
      case "hide_locked":
        hideLocked = a.on;
        break;
      case "stop_on_disable":
        stopOnDisable = a.on;
        break;
      case "sort":
        if (sort === a.col) sortAsc = !sortAsc;
        else {
          sort = a.col;
          sortAsc = true;
        }
        break;
      case "change":
        if (mockAdmin) change(a.ty);
        break;
      case "undo_last":
        if (mockAdmin) undo(history.map((_, i) => i).filter((i) => !history[i].undone && history[i].time.slice(0, 16) === history.filter((h) => !h.undone).slice(-1)[0]?.time.slice(0, 16)));
        break;
      case "refresh":
        lastAction = "";
        break;
      case "op":
        if (op.event(a.ev) === "undo" && op.op) {
          const ids = op.op.items.map((i) => history.findIndex((h) => h.name === i.id && !h.undone)).filter((i) => i >= 0);
          undo(ids);
          op.event("Go");
        }
        break;
      case "history":
        showHistory = true;
        break;
      case "history_close":
        showHistory = false;
        break;
      case "hist_toggle":
        if (!history[a.index].undone && !histSel.delete(a.index)) histSel.add(a.index);
        break;
      case "hist_undo":
        if (mockAdmin && histSel.size > 0) {
          undo([...histSel]);
          histSel.clear();
        }
        break;
      case "hist_clear":
        for (let i = history.length - 1; i >= 0; i--) if (history[i].undone) history.splice(i, 1);
        histSel.clear();
        break;
      case "backups":
        backupsOpen = true;
        backupMsg = null;
        break;
      case "backups_close":
        backupsOpen = false;
        break;
      case "backup_create":
        if (mockAdmin) creating = 3;
        break;
      case "backup_folder":
        break;
      case "backup_restore": {
        if (!mockAdmin) break;
        const b = backups[a.index];
        const items = b.diffs === 0 ? [] : history.slice(0, b.diffs).map((h) => opItem(h.name, `${h.display} (${h.name})`, "Startup type", `${h.to} -> ${h.from}`));
        backupOp.open(
          makeOp("RESTORE SERVICES BACKUP", "restore", "restored", items, {
            noun: ["service", "services"],
            preview_text: `Back to the backup from ${b.time}.`,
            go_label: "Restore",
            running_text: "Restoring services...",
            reassure: "Only startup types change. Protected services are restored as SYSTEM; a restart finishes it.",
            stage: items.length ? "Preview" : "Done",
            empty_title: "Nothing to restore",
            empty_text: `Every service already has the startup type saved in the backup from ${b.time}.`,
          }),
          () => {
            b.diffs = 0;
            if (backupOp.op) {
              backupOp.op.can_undo = false;
              backupOp.op.notes = ["Restart the PC so every service starts the way the backup says."];
            }
          },
        );
        break;
      }
      case "backup_op":
        backupOp.event(a.ev);
        break;
    }
  },
  tick() {
    let changed = false;
    if (creating > 0 && --creating === 0) {
      backups.unshift({ time: "2026-11-16 20:14:31", detail: `${list.length + 240} services - made by you`, diffs: 0 });
      backupMsg = { text: "Backup saved: C:\\Users\\e\\Documents\\PCTuner-Backups\\Services\\services-20261116-201431.json", ok: true };
      changed = true;
    }
    const a = op.tick();
    const b = backupOp.tick();
    return changed || a || b;
  },
};
