import type { Action, BloatRow, View } from "../tabs/Bloat";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import type { MockTab } from "./types";

// [friendly name, package / capability name, what you lose, optional, installed (null = not a Store app)]
type Seed = [name: string, id: string, loses: string, optional: boolean, installed: boolean | null];

const seeds: Seed[] = [
  ["Copilot app (desktop + identity) and the Copilot key action", "Microsoft.MicrosoftOfficeHub", "Copilot app (desktop + identity) and the Copilot key action", false, true],
  ["Clipchamp video editor", "Clipchamp.Clipchamp", "Clipchamp video editor", false, false],
  ["Microsoft News", "Microsoft.BingNews", "Microsoft News", false, true],
  ["Weather", "Microsoft.BingWeather", "Weather", false, true],
  ["Bing web results in Start search", "Microsoft.BingSearch", "Bing web results in Start search", false, false],
  ["Solitaire", "Microsoft.MicrosoftSolitaireCollection", "Solitaire", false, true],
  ["Power Automate", "Microsoft.PowerAutomateDesktop", "Power Automate", false, false],
  ["Dev Home (retired)", "Microsoft.Windows.DevHome", "nothing (retired stub)", false, false],
  ["Camera app", "Microsoft.WindowsCamera", "Camera app", false, true],
  ["Feedback Hub", "Microsoft.WindowsFeedbackHub", "Feedback Hub", false, true],
  ["MSN widgets", "Microsoft.StartExperiencesApp", "MSN widgets", false, true],
  ["Pen handwriting", "Microsoft.Ink.Handwriting.Main.en-US.1.0.1", "pen handwriting", false, false],
  ["Steps Recorder", "App.StepsRecorder", "Steps Recorder", false, null],
  ["Windows Hello face (PIN unaffected)", "Hello.Face", "Windows Hello face (PIN unaffected)", false, null],
  ["Print Management console", "Print.Management.Console", "Print Management console", false, null],
  ["Pen handwriting language data", "Language.Handwriting", "pen handwriting language data", false, null],
  ["Recall (AI screen snapshots)", "Recall", "nothing (Recall off by policy)", false, null],
  ["OneDrive leftovers", "OneDrive leftovers", "nothing (stops OneDrive returning)", false, null],
  ["Widgets board", "MicrosoftWindows.Client.WebExperience", "Widgets board", true, true],
  ["Third-party widgets", "Microsoft.WidgetsPlatformRuntime", "third-party widgets", true, true],
  ["Teams + Office meeting add-in", "MSTeams", "Teams + Office meeting add-in", true, false],
  ["New Outlook", "Microsoft.OutlookForWindows", "new Outlook", true, true],
  ["Phone Link", "Microsoft.YourPhone", "Phone Link", true, true],
  ["Mobile devices page", "MicrosoftWindows.CrossDevice", "Mobile devices page", true, true],
  ["To Do", "Microsoft.Todos", "To Do", true, false],
  ["Sticky Notes", "Microsoft.MicrosoftStickyNotes", "Sticky Notes", true, true],
  ["Clock / timers", "Microsoft.WindowsAlarms", "Clock / timers", true, true],
  ["Sound Recorder", "Microsoft.WindowsSoundRecorder", "Sound Recorder", true, true],
  ["Quick Assist", "MicrosoftCorporationII.QuickAssist", "Quick Assist", true, true],
  ["Get Help + Settings troubleshooters", "Microsoft.GetHelp", "Get Help + Settings troubleshooters", true, true],
  ["Game Bar speech-to-text overlay", "Microsoft.XboxSpeechToTextOverlay", "Game Bar speech-to-text overlay", true, true],
  ["Windows Media Player Legacy", "Media.WindowsMediaPlayer", "Windows Media Player Legacy", true, null],
  ["Old IE engine", "Browser.InternetExplorer", "old IE engine", true, null],
  ["Ink math input", "MathRecognizer", "ink math input", true, null],
  ["PowerShell ISE", "Microsoft.Windows.PowerShell.ISE", "PowerShell ISE", true, null],
  ["Voice typing / speech", "Language.Speech", "voice typing / speech", true, null],
  ["Copy text from images", "Language.OCR", "copy text from images", true, null],
];

const rows: BloatRow[] = seeds.map(([name, id, loses, optional, installed], index) => ({
  index,
  name,
  tip: `${id}\nYou lose: ${loses}\nTicked = removed by Remove selected`,
  optional,
  installed,
  picked: false,
}));

const op = new MockOp();
let log: string | null = null;
let scanTicks = mockParams.get("state") === "loading" ? Infinity : 0;

export const bloat: MockTab<View, Action> = {
  view() {
    const scanning = scanTicks > 0;
    const selected = rows.filter((r) => r.picked).length;
    const running = op.op?.stage === "Running";
    const apps = rows.filter((r) => r.installed !== null);
    const canRemove = mockAdmin && !running && selected > 0;
    return {
      scanning,
      apps: apps.length,
      gone: scanning ? null : apps.filter((r) => !r.installed).length,
      rows: rows.map((r) => ({ ...r, installed: scanning ? null : r.installed })),
      selected,
      canRemove,
      removeTip: canRemove ? "Removes the ticked apps. Shows the list to confirm first." : mockAdmin ? "Tick the apps to remove in the list below." : "Run PC Tuner as administrator to remove apps.",
      canClear: !running && selected > 0,
      canRescan: !running,
      op: op.op,
      opCan: mockAdmin && !running,
      log,
    };
  },
  act(a) {
    switch (a.kind) {
      case "toggle":
        rows[a.index].picked = !rows[a.index].picked;
        break;
      case "clear":
        rows.forEach((r) => (r.picked = false));
        break;
      case "rescan":
        scanTicks = 3;
        break;
      case "remove": {
        const picked = rows.filter((r) => r.picked);
        if (!mockAdmin || picked.length === 0) break;
        op.open(
          makeOp(
            "REMOVE BLOAT",
            "remove",
            "removed",
            picked.map((r) => opItem(String(r.index), r.name, r.optional ? "Optional list" : "Safe list", r.installed === false ? "already gone" : "", r.optional)),
            {
              preview_text: `Removed for every user, leftovers cleaned up, and blocked from coming back with updates.\n\nRemoves: ${picked.map((r) => r.name).join(", ")}.`,
              warning: "A Windows restore point is created first.",
              reassure: "Never touches Xbox, Game Pass, Store, Security, Edge/WebView2, Photos, Notepad or codecs.",
              go_label: "Remove",
              running_text: "Removing...",
              empty_text: "Nothing is ticked. Tick the apps to remove in the list.",
            },
          ),
          () => {
            picked.forEach((r) => {
              if (r.installed !== null) r.installed = false;
            });
            if (op.op) {
              op.op.can_undo = false;
              op.op.extras = ["Show technical details"];
              op.op.notes = ["Restore point created.", "Registered re-check task \\Debloat\\ReapplyDebloat (startup + daily) - removes anything updates bring back.", "Restart the PC to finish."];
            }
          },
        );
        break;
      }
      case "op":
        if (op.event(a.ev) === "extra") {
          log = ["20:15:22  Restore point created.", ...(op.op?.items ?? []).flatMap((i) => [`20:15:24  Removing ${seeds[Number(i.id)][1]}  -  ${seeds[Number(i.id)][2]}`, `20:15:25    done: ${seeds[Number(i.id)][1]} removed`]), "Restart the PC. Undo any time with:  .\\Remove-Bloat.ps1 -Restore"].join("\n");
        }
        break;
      case "close_log":
        log = null;
        break;
    }
  },
  tick: () => {
    let changed = op.tick();
    if (scanTicks > 0 && scanTicks !== Infinity) {
      scanTicks -= 1;
      changed = true;
    }
    return changed;
  },
};
