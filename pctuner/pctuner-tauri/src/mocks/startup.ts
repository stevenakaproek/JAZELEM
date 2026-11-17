import type { Tone } from "../lib/types";
import type { StartupAction, StartupKind, StartupView } from "../tabs/Startup";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import { mockBackups, restoreOp } from "./backups";
import type { MockTab } from "./types";

type Category = "Junk" | "Optional" | "Keep" | "Review" | "Windows";

interface Entry {
  id: string;
  kind: StartupKind;
  name: string;
  location: string;
  publisher: string;
  command: string;
  on: boolean;
  category: Category;
  advice: string;
}

const KIND_LABEL: Record<StartupKind, string> = {
  Run: "Logon (Run key)",
  StartupFolder: "Logon (Startup folder)",
  Task: "Scheduled task",
  StoreApp: "Store app startup",
  ShellExt: "Explorer extension",
  Tweak: "Windows setting",
};
const KIND_ORDER: StartupKind[] = ["Run", "StartupFolder", "Task", "StoreApp", "ShellExt", "Tweak"];
const GROUP: Record<StartupKind, string> = {
  Run: "Startup programs",
  StartupFolder: "Startup programs",
  Task: "Scheduled tasks",
  StoreApp: "Store app startup",
  ShellExt: "Explorer add-ons",
  Tweak: "Windows settings",
};

const JUNK_TASK = "Junk - Windows telemetry / tips / unused feature";
const WIN_TASK = "Windows task - leave unless you know it";
const REVIEW = "Review - starts with Windows; switch off if you do not need it at login";
const REVIEW_EXT = "Review - third-party Explorer add-on; switch off if you never use its menu item / icon";

type Seed = [kind: StartupKind, name: string, location: string, publisher: string, command: string, on: boolean, category: Category, advice: string];

const seeds: Seed[] = [
  ["Run", "OneDrive", "HKCU Run", "Microsoft Corporation", '"C:\\Program Files\\Microsoft OneDrive\\OneDrive.exe" /background', true, "Junk", "Junk - cloud sync; start it by hand when you need it"],
  ["Run", "MicrosoftEdgeAutoLaunch", "HKCU Run", "Microsoft Corporation", '"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe" --no-startup-window --win-session-start', true, "Junk", "Junk - Edge preloads itself at login"],
  ["Run", "Discord", "HKCU Run", "Discord Inc.", '"C:\\Users\\e\\AppData\\Local\\Discord\\Update.exe" --processStart Discord.exe', true, "Optional", "Optional - opens Discord at login"],
  ["Run", "Steam", "HKCU Run", "Valve Corporation", '"C:\\Program Files (x86)\\Steam\\steam.exe" -silent', true, "Optional", "Optional - opens Steam at login"],
  ["Run", "SecurityHealth", "HKLM Run", "Microsoft Corporation", "%windir%\\system32\\SecurityHealthSystray.exe", true, "Keep", "Keep - Windows Security tray icon"],
  ["Run", "RtkAudUService", "HKLM Run", "Realtek Semiconductor", '"C:\\Windows\\System32\\DriverStore\\FileRepository\\realtekservice.inf_amd64\\RtkAudUService64.exe" -background', true, "Keep", "Keep - Realtek audio (jack detection, effects)"],
  ["Run", "MSI Center", "HKLM Run (32-bit)", "Micro-Star INT'L CO., LTD.", '"C:\\Program Files (x86)\\MSI\\MSI Center\\MSI.CentralServer.exe"', false, "Review", REVIEW],
  ["StartupFolder", "MSI Afterburner", "Startup folder (user)", "MSI Co., LTD", "C:\\Program Files (x86)\\MSI Afterburner\\MSIAfterburner.exe /s", true, "Keep", "Keep - applies your graphics card profile at login"],
  ["Task", "Consolidator", "\\Microsoft\\Windows\\Customer Experience Improvement Program\\", "Microsoft Corporation", "%SystemRoot%\\System32\\wsqmcons.exe", true, "Junk", JUNK_TASK],
  ["Task", "UsbCeip", "\\Microsoft\\Windows\\Customer Experience Improvement Program\\", "Microsoft Corporation", "(custom handler)", true, "Junk", JUNK_TASK],
  ["Task", "MapsUpdateTask", "\\Microsoft\\Windows\\Maps\\", "Microsoft Corporation", "(custom handler)", false, "Junk", JUNK_TASK],
  ["Task", "QueueReporting", "\\Microsoft\\Windows\\Windows Error Reporting\\", "Microsoft Corporation", "%windir%\\system32\\wermgr.exe -upload", true, "Junk", JUNK_TASK],
  ["Task", "XblGameSaveTask", "\\Microsoft\\XblGameSave\\", "Microsoft Corporation", "%windir%\\System32\\XblGameSaveTask.exe standby", true, "Keep", "Keep - syncs Xbox / Game Pass cloud saves"],
  ["Task", "NvTmRep_CrashReport1", "\\", "NVIDIA Corporation", '"C:\\Program Files\\NVIDIA Corporation\\NvBackend\\NvTmRep.exe"', true, "Junk", "Junk - NVIDIA telemetry"],
  ["Task", "MicrosoftEdgeUpdateTaskMachineCore", "\\", "Microsoft Corporation", '"C:\\Program Files (x86)\\Microsoft\\EdgeUpdate\\MicrosoftEdgeUpdate.exe" /c', true, "Optional", "Optional - Edge updater; Edge still updates when opened"],
  ["Task", "ScheduledDefrag", "\\Microsoft\\Windows\\Defrag\\", "Microsoft Corporation", "%windir%\\system32\\defrag.exe -c -h -k -g -$", true, "Windows", WIN_TASK],
  ["Task", "SynchronizeTime", "\\Microsoft\\Windows\\Time Synchronization\\", "Microsoft Corporation", "%windir%\\system32\\sc.exe start w32time task_started", true, "Windows", WIN_TASK],
  ["StoreApp", "Phone Link", "Microsoft.YourPhone", "Microsoft Corporation", "YourPhoneAppProxy", true, "Junk", "Junk - Phone Link background start"],
  ["StoreApp", "Xbox", "Microsoft.GamingApp", "Microsoft Corporation", "XboxPcAppStartupTask", false, "Optional", "Optional - opens the Xbox app at login"],
  ["ShellExt", "NvAppShExt", "Context menu (Desktop background)", "NVIDIA Corporation", "C:\\Windows\\System32\\DriverStore\\FileRepository\\nvdmui.inf_amd64\\nvshext.dll", true, "Junk", "Junk - 'NVIDIA Control Panel' entry in the desktop right-click menu"],
  ["ShellExt", "7-Zip", "Context menu (All files)", "Igor Pavlov", "C:\\Program Files\\7-Zip\\7-zip.dll", true, "Review", REVIEW_EXT],
  ["Tweak", "Bing web results in Start search", "Windows setting", "Microsoft Corporation", "DisableSearchBoxSuggestions = 1", true, "Junk", "Junk - Start search sends what you type to Bing"],
  ["Tweak", "Suggested apps, silent app installs and tips", "Windows setting", "Microsoft Corporation", "ContentDeliveryManager values = 0", true, "Junk", "Junk - Windows installs and advertises apps by itself"],
  ["Tweak", "Widgets (news feed board)", "Windows setting", "Microsoft Corporation", "AllowNewsAndInterests = 0", false, "Junk", "Junk - news feed process in the background"],
  ["Tweak", "Windows Spotlight on the lock screen", "Windows setting", "Microsoft Corporation", "RotatingLockScreenEnabled = 0", true, "Optional", "Optional - downloads lock-screen pictures and tips"],
];

const entries: Entry[] = seeds.map(([kind, name, location, publisher, command, on, category, advice]) => ({ id: `${kind}|${location}|${name}`, kind, name, location, publisher, command, on, category, advice }));

interface Hist {
  batch: number;
  time: string;
  id: string;
  kind: StartupKind;
  name: string;
  location: string;
  from: boolean;
  to: boolean;
  undone: boolean;
}

const find = (name: string) => entries.find((e) => e.name === name)!;
const hist = (batch: number, time: string, name: string, undone = false): Hist => {
  const e = find(name);
  return { batch, time, id: e.id, kind: e.kind, name, location: e.location, from: true, to: false, undone };
};
let history: Hist[] = [hist(1, "2026-11-02 21:14:08", "MSI Center", true), hist(2, "2026-11-09 19:40:51", "MapsUpdateTask"), hist(2, "2026-11-09 19:40:51", "Widgets (news feed board)"), hist(2, "2026-11-09 19:40:51", "Xbox")];

const state = mockParams.get("state");
const selected = new Set<string>();
const histSel = new Set<number>();
let search = "";
let kindFilter: StartupKind | null = null;
let enabledOnly = false;
let showWindows = false;
let lastAction = "";
let histStatus = "";
let showHistory = false;
const op = new MockOp();
/** Runs when "Undo" is pressed in the result of the open operation window. */
let undoOpen: (() => void) | null = null;

function visible(): Entry[] {
  if (state === "empty") return [];
  const q = search.trim().toLowerCase();
  return entries
    .filter((e) => (!q || [e.name, e.command, e.publisher, e.location].some((s) => s.toLowerCase().includes(q))) && (!kindFilter || e.kind === kindFilter) && (!enabledOnly || e.on) && (showWindows || e.category !== "Windows"))
    .sort((a, b) => KIND_LABEL[a.kind].toLowerCase().localeCompare(KIND_LABEL[b.kind].toLowerCase()) || a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
}

function tone(e: Entry): Tone {
  if (!e.on) return "Dim";
  return e.category === "Junk" ? "Junk" : e.category === "Optional" ? "Warn" : e.category === "Keep" ? "Ok" : "Text";
}

function now(): string {
  return new Date().toISOString().slice(0, 19).replace("T", " ");
}

function keepWarning(items: Entry[]): string | null {
  const keep = items.filter((e) => e.category === "Keep" || e.category === "Windows").map((e) => e.name);
  return keep.length ? `These are marked KEEP (gaming / hardware / security / Windows):\n\n${keep.join("\n")}\n\nSwitch them off anyway?` : null;
}

/** Switch entries on or off as one history batch (what the app does when the window's work is done). */
function applyToggle(items: Entry[], on: boolean) {
  const batch = Math.max(0, ...history.map((h) => h.batch)) + 1;
  const time = now();
  for (const e of items) {
    history.push({ batch, time, id: e.id, kind: e.kind, name: e.name, location: e.location, from: e.on, to: on, undone: false });
    e.on = on;
  }
  undoOpen = () => openUndo(history.map((h, i) => (h.batch === batch && !h.undone ? i : -1)).filter((i) => i >= 0), "Undo", true);
}

// what pctuner_core::explain::startup says (shortened)
function about(e: { name: string; kind: string; publisher: string; command: string; category: string; advice: string }) {
  const known: [string, string][] = [
    ["7-zip", "7-Zip's right-click menu in File Explorer (Extract here, Add to archive)."],
    ["discord", "Discord chat and voice. Off: you are not shown online until you open it."],
    ["steam", "Steam opens at sign-in so updates and friends are ready. Off: Steam starts when you open it or a game."],
    ["onedrive", "OneDrive: syncs your files with Microsoft's cloud. Off: files stop syncing until you open OneDrive."],
    ["edge", "Microsoft Edge preloading itself in the background so it opens faster. Off: Edge opens a moment slower."],
    ["nvidia", "NVIDIA app / driver helper (overlay, recording, driver updates). Off: those extras start when you open the NVIDIA app."],
  ];
  const hay = `${e.name} ${e.command}`.toLowerCase();
  const hit = known.find(([k]) => hay.includes(k));
  const when =
    e.kind === "Task"
      ? "A scheduled task: Windows runs it at set times or events (for example at sign-in or once a day)."
      : e.kind === "ShellExt"
        ? "An add-on loaded into File Explorer (for example extra right-click menu items)."
        : "It starts every time you sign in to Windows.";
  const path = e.command.startsWith('"') ? e.command.slice(1).split('"')[0] : e.command.split(" -")[0].split(" /")[0];
  const prog = path.split(/[\\/]/).pop() ?? "";
  const [hw, ho] = hit ? hit[1].split(" Off: ") : ["", ""];
  const what = hit ? `${hw} ${when}` : `${e.name}. ${when}${prog ? ` It runs ${prog}${e.publisher ? `, made by ${e.publisher}` : ""}.` : ""}`;
  const off =
    e.kind === "ShellExt"
      ? "Switching it off removes its extra menu items or icons from File Explorer; the program itself keeps working, nothing is deleted, and it can be undone."
      : e.category === "Junk"
      ? "Safe to switch off: Windows or the program works without it. Switching off never deletes anything and can be undone."
      : "Switching it off only stops it starting by itself: you can still open the program yourself, nothing is deleted, and it can be undone.";
  const off2 = ho ? `${ho[0].toUpperCase()}${ho.slice(1)} ${off}` : off;
  return { what, off: e.advice ? `${off2} PC Tuner's advice: ${e.advice}.` : off2, windows: "" };
}

function openUndo(idx: number[], title: string, run = false) {
  const w = makeOp(
    title,
    "undo",
    "undone",
    idx.map((i) => opItem(`h${i}`, history[i].name, GROUP[history[i].kind], history[i].from ? "back on" : "back off")),
    { noun: ["change", "changes"], preview_text: "Each item goes back to the state it had before the change.", go_label: "Undo", running_text: "Undoing...", empty_title: "Nothing to undo", empty_text: "There are no active changes in the history." },
  );
  if (run) w.stage = "Running";
  undoOpen = null;
  op.open(w, () => {
    for (const i of idx) {
      history[i].undone = true;
      const e = entries.find((x) => x.id === history[i].id);
      if (e) e.on = history[i].from;
    }
    lastAction = `Undid ${idx.length} change(s).`;
    if (op.op) op.op.can_undo = false;
  });
}

const bk = mockBackups(
  "A backup saves whether each startup item is switched on or off. Restore switches back only the items that differ (new items are left as they are), with the usual preview, and can be undone.",
  () => entries.map((e) => ({ id: e.id, title: `${e.name} (${GROUP[e.kind]})`, value: e.on ? "on" : "off", shown: e.on ? "On" : "Off" })),
  (diffs) =>
    restoreOp(op, "RESTORE STARTUP BACKUP", diffs, (d) => {
      const e = entries.find((x) => x.id === d.backup.id);
      if (e) e.on = d.backup.value === "on";
    }),
);

export const startup: MockTab<StartupView, StartupAction> = {
  view() {
    const ready = state !== "loading";
    const rows = visible();
    const sel = rows.filter((e) => selected.has(e.id));
    const cleanCount = sel.filter((e) => e.on).length;
    const busy = op.op?.stage === "Running";
    const junk = entries.filter((e) => e.on && e.category === "Junk").length;
    const optional = entries.filter((e) => e.on && e.category === "Optional").length;
    const active = history.filter((h) => !h.undone).length;
    const val = (n: number) => (ready ? String(n) : "-");
    const histN = [...histSel].filter((i) => history[i] && !history[i].undone).length;
    return {
      ready,
      busy,
      status: busy ? "Working / scanning..." : lastAction,
      hero: {
        junk: val(junk),
        tone: !ready ? "Faint" : junk === 0 ? "Ok" : "Junk",
        caption: !ready ? "Scanning startup locations..." : junk === 0 ? "Nothing left to clean." : "Safe to switch off - nothing is deleted.",
        on: val(entries.filter((e) => e.on && e.category !== "Windows").length),
        optional: val(optional),
        optionalTone: optional === 0 || !ready ? "Text" : "Warn",
        pending: String(active),
      },
      warnings: state === "error" ? ["Could not read startup-history.json (expected value at line 1 column 1). It was kept as startup-history.json.corrupt and a new history was started."] : [],
      search,
      kindFilter,
      kinds: KIND_ORDER.map((value) => ({ value, label: KIND_LABEL[value] })),
      enabledOnly,
      showWindows,
      rows: ready ? rows.map((e) => ({ id: e.id, on: e.on, name: e.name, type: KIND_LABEL[e.kind], location: e.location, publisher: e.publisher, advice: e.advice, command: e.command, tone: tone(e), selected: selected.has(e.id), about: about(e) })) : [],
      selected: sel.length,
      cleanCount,
      canClean: !busy && cleanCount > 0,
      canSwitchOn: !busy && sel.length > 0,
      canUndoLast: !busy && active > 0,
      canRescan: !busy,
      canExplorer: !busy,
      history: {
        open: showHistory && !op.op,
        recorded: history.length,
        active,
        rows: history
          .map((h, index) => ({ index, time: h.time, name: h.name, type: KIND_LABEL[h.kind], location: h.location, change: h.to ? "Switched on" : "Switched off", state: h.undone ? "Undone" : "Active", undone: h.undone, selected: !h.undone && histSel.has(index) }))
          .reverse(),
        selected: histN,
        undoSelectedLabel: histN > 0 ? `Undo selected (${histN})` : "Undo selected",
        canUndoSelected: !busy && histN > 0,
        canClear: !busy,
        canUndoAll: !busy && active > 0,
        busy,
        status: histStatus,
      },
      op: op.op,
      opCan: mockAdmin || op.op?.kicker === "Restart Explorer",
      backups: bk.view(mockAdmin && !op.op),
    };
  },
  act(a) {
    if (bk.act(a)) return;
    const sel = () => visible().filter((e) => selected.has(e.id));
    switch (a.kind) {
      case "toggle":
        if (!selected.delete(a.id)) selected.add(a.id);
        break;
      case "select_all":
        visible().forEach((e) => selected.add(e.id));
        break;
      case "clear_selection":
        selected.clear();
        break;
      case "search":
        search = a.text;
        break;
      case "kind_filter":
        kindFilter = a.value;
        break;
      case "enabled_only":
        enabledOnly = a.on;
        break;
      case "show_windows":
        showWindows = a.on;
        break;
      case "clean": {
        const items = sel().filter((e) => e.on);
        if (!items.length) break;
        undoOpen = null;
        op.open(
          makeOp(
            "Clean selected",
            "clean",
            "cleaned",
            items.map((e) => opItem(e.id, e.name, GROUP[e.kind], "", e.category === "Optional")),
            { preview_text: "These get switched off. Nothing is deleted.", reassure: "One click undoes the whole cleanup.", go_label: "Clean now", running_text: "Cleaning...", undo_label: "Undo this cleanup", empty_title: "Already clean", empty_text: "Everything selected is already switched off.", warning: keepWarning(items) },
          ),
          () => {
            const done = items.filter((e) => op.op?.items.find((i) => i.id === e.id)?.result && "Ok" in op.op.items.find((i) => i.id === e.id)!.result!);
            applyToggle(done, false);
            selected.clear();
            const left = entries.filter((e) => e.on && e.category === "Junk").length;
            lastAction = `${done.length} items switched off.`;
            if (op.op) {
              op.op.footer = [`Junk still switched on: ${left}`, left === 0 ? "Ok" : "Junk"];
              op.op.notes = ["Sign out or restart Windows to finish."];
              if (done.some((e) => e.kind === "ShellExt")) op.op.extras = ["Restart Explorer"];
            }
          },
        );
        break;
      }
      case "switch_on": {
        const items = sel().filter((e) => !e.on);
        if (!items.length) {
          lastAction = "Nothing to change - already switched on.";
          break;
        }
        undoOpen = null;
        op.open(
          makeOp(
            "switch on",
            "switch on",
            "switched on",
            items.map((e) => opItem(e.id, e.name, GROUP[e.kind], "", e.category === "Optional")),
            { preview_text: "These get switched back on.", reassure: "Every change goes into the history and can be undone.", go_label: "Switch on", running_text: "Switching on..." },
          ),
          () => {
            applyToggle(items, true);
            lastAction = `${items.length} switched on.`;
          },
        );
        break;
      }
      case "undo_last": {
        const last = Math.max(...history.filter((h) => !h.undone).map((h) => h.batch));
        openUndo(history.map((h, i) => (h.batch === last && !h.undone ? i : -1)).filter((i) => i >= 0), "Undo last change");
        break;
      }
      case "rescan":
        lastAction = "";
        break;
      case "explorer":
        undoOpen = null;
        op.open(
          makeOp("Restart Explorer", "restart", "restarted", [opItem("explorer", "Windows Explorer", "Explorer", "taskbar, desktop and folder windows")], { noun: ["program", "programs"], preview_text: "Needed for Explorer add-on changes to take effect.", warning: "The taskbar blinks and open folder windows close.", go_label: "Restart now", running_text: "Restarting Explorer..." }),
          () => {
            lastAction = "Explorer restarted.";
            if (op.op) op.op.can_undo = false;
          },
        );
        break;
      case "history_open":
        showHistory = true;
        break;
      case "history_close":
        showHistory = false;
        histStatus = "";
        break;
      case "hist_toggle":
        histStatus = "";
        if (history[a.index] && !history[a.index].undone) {
          if (!histSel.delete(a.index)) histSel.add(a.index);
        } else histSel.delete(a.index);
        break;
      case "hist_undo_selected": {
        const idx = [...histSel].filter((i) => history[i] && !history[i].undone);
        histSel.clear();
        if (idx.length) openUndo(idx, "Undo selected changes");
        else histStatus = "Nothing to undo - the selected changes were already undone.";
        break;
      }
      case "hist_undo_all":
        histStatus = "";
        openUndo(history.map((h, i) => (h.undone ? -1 : i)).filter((i) => i >= 0), "Undo everything");
        break;
      case "hist_clear":
        histStatus = "";
        history = history.filter((h) => !h.undone);
        histSel.clear();
        break;
      case "op": {
        if (!mockAdmin && a.ev !== "Close" && op.op?.kicker !== "Restart Explorer") break;
        const r = op.event(a.ev);
        if (r === "undo") {
          const run = undoOpen;
          op.op = null;
          run?.();
        } else if (r === "extra" && op.op) {
          op.op.extras = [];
          op.op.notes.push("Explorer restarted.");
          lastAction = "Explorer restarted.";
        }
        break;
      }
      default:
        break;
    }
  },
  tick: () => op.tick(),
};
