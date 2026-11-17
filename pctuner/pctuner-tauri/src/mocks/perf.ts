import type { PerfAction, PerfRow, PerfView } from "../tabs/Performance";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import { mockBackups, restoreOp } from "./backups";
import type { MockTab } from "./types";

type Seed = [name: string, group: string, why: string, optional: boolean, now: string, target: string, set: boolean, applicable?: boolean];

const GAMES = "Games";
const DESKTOP = "Snappier desktop";
const POWER = "Power, mouse and disk";

const seeds: Seed[] = [
  ["Optimizations for windowed games", GAMES, "Older DX10/DX11 games in a borderless window switch to the modern 'flip' model: lower latency and working VRR. Applies at the next game launch.", false, "On", "On", true],
  ["Windows variable refresh rate", GAMES, "Lets old DX11 games without their own VRR support use G-SYNC. Small: most games already handle VRR.", true, "Off", "On", false],
  ["Faster submenus", DESKTOP, "Submenus (Send to, Open with, menu bars) open after 100 ms instead of 400 ms. You feel it; no effect on fps. Applies at once.", false, "400 ms", "100 ms", false],
  ["Faster tooltips", DESKTOP, "Hover pop-ups in classic apps appear after 200 ms instead of 400 ms. Many modern apps use their own timing.", true, "400 ms", "200 ms", false],
  ["Taskbar animations", DESKTOP, "Taskbar buttons stop sliding and fading. Small; needs an Explorer restart.", true, "Off", "Off", true],
  ["Classic right-click menu", DESKTOP, "Right-click shows the full menu straight away instead of 'Show more options'. Needs an Explorer restart.", true, "Windows 11 menu", "Classic menu", false],
  ["CPU responds faster (energy preference)", POWER, "Plugged-in energy preference 25 -> 10, the same value Windows' 'Best performance' power mode uses: clocks ramp up sooner.", true, "25", "10", false],
  ["Mouse acceleration off", POWER, "'Enhance pointer precision' off: the cursor moves 1:1 with your hand.", true, "Off", "Off", true],
  ["Hibernation off (frees disk space)", POWER, "Deletes hiberfil.sys (about 40 % of your RAM in size). Disk space only, not speed.", true, "On", "Off", false],
  ["Virtualization-based security (VBS) off", POWER, "Stops Windows running its security layer in a hidden virtual machine. Costs security. Restart needed.", true, "", "", false, false],
];

const rows: PerfRow[] = seeds.map(([name, , why, optional, now, target, set, applicable = true], index) => ({
  index,
  name,
  why,
  optional,
  applicable,
  blocked: applicable ? "" : "This PC's firmware keeps VBS locked on.",
  set,
  selectable: applicable && !set,
  selected: false,
  now,
  target,
  extra: name.startsWith("Hibernation") ? "hiberfil.sys is 12.7 GB now" : null,
  canSwitch: false,
}));
/** what a switched-off row shows (the stand-in's "value from before") */
const offText = seeds.map(([, , , , now, target, set]) => (set ? (now === "On" ? "Off" : now === "Off" ? "On" : now) : now === target ? "Off" : now));

const op = new MockOp();
const defenderOp = new MockOp();
const folders = [
  { kind: "Steam library", path: "C:\\Program Files (x86)\\Steam\\steamapps\\common", done: true, on: true },
  { kind: "Xbox games", path: "D:\\XboxGames", done: false, on: true },
  { kind: "Epic Games", path: "D:\\Epic Games", done: false, on: false },
];
let note: string | null = null;
let customErr: string | null = null;
const state = mockParams.get("state");

const bk = mockBackups(
  "A backup saves every performance setting in this list (not the Defender folders). Restore puts back only the settings that differ, with the usual preview, and can be undone.",
  () => rows.filter((r) => r.applicable).map((r) => ({ id: String(r.index), title: r.name, value: r.now, shown: r.now })),
  (diffs) =>
    restoreOp(op, "RESTORE PERFORMANCE BACKUP", diffs, (d) => {
      const r = rows[Number(d.backup.id)];
      r.now = d.backup.shown;
      r.set = r.now === r.target;
      r.selectable = r.applicable && !r.set;
    }),
  (it) => ({ ...it, value: it.value === "On" ? "Off" : "On", shown: it.value === "On" ? "Off" : "On" }),
);

export const perf: MockTab<PerfView, PerfAction> = {
  view() {
    const pending = rows.filter((r) => r.selected).length;
    const todo = folders.filter((f) => f.on && !f.done).length;
    return {
      loading: state === "loading",
      refreshing: false,
      error: state === "error" ? "The settings could not be read: PowerShell returned no data (exit code 1)." : null,
      groups: [GAMES, DESKTOP, POWER].map((name) => ({ name, rows: rows.filter((r) => seeds[r.index][1] === name).map((r) => ({ ...r, canSwitch: mockAdmin && r.applicable && !op.op })) })),
      pending,
      canApply: mockAdmin && pending > 0,
      canUndo: mockAdmin,
      canExplorer: !op.op,
      note,
      op: op.op,
      opCan: mockAdmin,
      backups: bk.view(mockAdmin && !op.op),
      defender: {
        scanning: false,
        error: "",
        passive: false,
        readable: mockAdmin,
        rows: folders.map((f) => ({ ...f, canTick: mockAdmin && !f.done })),
        customErr,
        todo,
        canExclude: mockAdmin && todo > 0,
        canUndo: mockAdmin,
        canRescan: true,
        op: defenderOp.op,
        opCan: mockAdmin,
      },
    };
  },
  act(a) {
    if (bk.act(a)) return;
    switch (a.kind) {
      case "switch": {
        const r = rows[a.index];
        if (!mockAdmin || op.op || !r.applicable || r.set === a.on) break;
        const from = r.now;
        const to = a.on ? r.target : offText[a.index];
        op.open(
          makeOp(a.on ? "PERFORMANCE" : "SWITCH OFF", "change", "changed", [opItem(String(r.index), r.name, "Setting", `${from} -> ${to}`)], {
            noun: ["setting", "settings"],
            go_label: a.on ? "Apply" : "Switch off",
            preview_text: a.on ? "Each change is saved so it can be undone." : "Puts back the value from before PC Tuner changed it (or the Windows default). Saved, so \"Undo last change\" switches it on again.",
            running_text: "Applying...",
          }),
          () => {
            r.set = a.on;
            r.now = to;
            r.selected = false;
            r.selectable = r.applicable && !r.set;
          },
        );
        break;
      }
      case "toggle":        if (rows[a.index].selectable) rows[a.index].selected = !rows[a.index].selected;
        break;
      case "clear":
        rows.forEach((r) => (r.selected = false));
        break;
      case "apply": {
        const picked = rows.filter((r) => r.selected);
        op.open(
          makeOp(
            "PERFORMANCE",
            "change",
            "changed",
            picked.map((r) => opItem(String(r.index), r.name, seeds[r.index][1], `${r.now} -> ${r.target}`, r.optional)),
            { noun: ["setting", "settings"], preview_text: "Each change is saved so it can be undone.", running_text: "Applying...", reassure: '"Undo last change" puts every value back.', undo_label: "Undo", extras: ["Restart Explorer"], notes: ["Taskbar and right-click changes show after Explorer restarts (or after signing out)."] },
          ),
          () =>
            picked.forEach((r) => {
              r.set = true;
              r.selected = false;
              r.selectable = false;
              r.now = r.target;
            }),
        );
        break;
      }
      case "undo":
        op.open(makeOp("UNDO PERFORMANCE CHANGE", "undo", "undone", [opItem("u", "Faster submenus", DESKTOP, "100 ms -> 400 ms")], { noun: ["setting", "settings"], go_label: "Undo", running_text: "Putting the old settings back...", reassure: "Restores the values from before that change." }));
        break;
      case "explorer":
        op.open(makeOp("Restart Explorer", "restart", "restarted", [opItem("explorer", "Windows Explorer", "Explorer", "taskbar, desktop and folder windows")], { noun: ["program", "programs"], preview_text: "Shows taskbar and right-click changes.", warning: "The taskbar blinks and open folder windows close. Not during a game.", go_label: "Restart now", running_text: "Restarting Explorer..." }));
        break;
      case "refresh":
        note = "Settings read again.";
        break;
      case "op":
        op.event(a.ev);
        break;
      case "defender_tick": {
        const f = folders.find((x) => x.path === a.path);
        if (f) f.on = a.on;
        break;
      }
      case "defender_add":
        customErr = /^[a-zA-Z]:\\/.test(a.path) ? null : "Type a full folder path, like D:\\Games.";
        if (!customErr) folders.push({ kind: "Added by you", path: a.path, done: false, on: true });
        break;
      case "defender_exclude":
        defenderOp.open(
          makeOp(
            "DEFENDER",
            "exclude",
            "excluded",
            folders.filter((f) => f.on && !f.done).map((f) => opItem(f.path, f.path, "Game folders")),
            { noun: ["folder", "folders"], go_label: "Exclude", warning: "Defender will not scan these folders any more.", running_text: "Adding exclusions..." },
          ),
          () => folders.forEach((f) => f.on && (f.done = true)),
        );
        break;
      case "defender_op":
        defenderOp.event(a.ev);
        break;
      default:
        break;
    }
  },
  tick: () => {
    const a = op.tick();
    const b = defenderOp.tick();
    return a || b;
  },
};
