import type { SchedulerAction, SchedulerOption, SchedulerRow, SchedulerSection, SchedulerView } from "../tabs/Scheduler";
import type { Tone } from "../lib/types";
import { mockAdmin } from "./env";
import type { MockTab } from "./types";

type Val = number | string | null;

const DISABLED = 0xffffffff;
const CPUS = 16; // Ryzen 7 9800X3D: 8 cores, 16 threads
const FULL = (1 << CPUS) - 1;
const TASKS = ["Audio", "Capture", "DisplayPostProcessing", "Distribution", "Games", "Playback", "Pro Audio", "Window Manager"];
const SYS_FIELDS: [string, string][] = [
  ["NetworkThrottlingIndex", "Network throttling index"],
  ["SystemResponsiveness", "System responsiveness reserved CPU"],
];
const TASK_FIELDS: [string, string][] = [
  ["Affinity", "Affinity"],
  ["Clock Rate", "Clock rate"],
  ["GPU Priority", "GPU priority"],
  ["Priority", "Priority"],
  ["Scheduling Category", "Scheduling category"],
  ["SFIO Priority", "I/O priority"],
  ["Background Only", "Background only"],
  ["BackgroundPriority", "Background priority"],
];
const TASK_DEFAULTS: Record<string, [number, string, string, number | null]> = {
  Audio: [6, "Medium", "True", null],
  Capture: [5, "Medium", "True", null],
  DisplayPostProcessing: [8, "High", "True", 8],
  Distribution: [4, "Medium", "True", null],
  Games: [2, "Medium", "False", null],
  Playback: [3, "Medium", "False", 4],
  "Pro Audio": [1, "High", "False", null],
  "Window Manager": [5, "Medium", "True", null],
};

function defaultOf(task: string, name: string): Val {
  if (task === "#prio") return 2;
  if (task === "#fg") return 200000;
  if (task === "") return name === "NetworkThrottlingIndex" ? 10 : 20;
  const [prio, cat, bgOnly, bgPrio] = TASK_DEFAULTS[task];
  switch (name) {
    case "Affinity":
      return 0;
    case "Clock Rate":
      return 10000;
    case "GPU Priority":
      return 8;
    case "Priority":
      return prio;
    case "Scheduling Category":
      return cat;
    case "SFIO Priority":
      return "Normal";
    case "Background Only":
      return bgOnly;
    default:
      return bgPrio;
  }
}

function show(name: string, v: Val): string {
  if (v === null) return "(not set)";
  if (name === "NetworkThrottlingIndex" && v === DISABLED) return "Disabled";
  if (name === "SystemResponsiveness") return `${v}%`;
  if (name === "Affinity") return v === 0 ? "all" : `0x${(v as number).toString(16).toUpperCase()}`;
  if (name === "Win32PrioritySeparation") return `0x${(v as number).toString(16).toUpperCase()}`;
  if (name === "ForegroundLockTimeout") return `${v} ms`;
  return String(v);
}

function choices(name: string): Val[] {
  const range = (a: number, b: number) => Array.from({ length: b - a + 1 }, (_, i) => a + i);
  switch (name) {
    case "NetworkThrottlingIndex":
      return [DISABLED, 1, 5, 10, 20, 30, 40, 50, 60, 70];
    case "SystemResponsiveness":
      return range(0, 10).map((n) => n * 10);
    case "GPU Priority":
      return range(0, 31);
    case "Priority":
      return range(1, 8);
    case "Scheduling Category":
      return ["High", "Medium", "Low"];
    case "SFIO Priority":
      return ["High", "Normal", "Low", "Idle"];
    case "Background Only":
      return ["True", "False"];
    case "BackgroundPriority":
      return [null, ...range(1, 8)];
    default:
      return [];
  }
}

const sectionOf = (task: string): SchedulerSection => (task === "#prio" || task === "#fg" ? "Processor" : "Multimedia");
const place = (task: string, name: string) => (task === "" || task.startsWith("#") ? name : `${task} › ${name}`);
const id = (task: string, name: string) => `${task}|${name}`;
const split = (key: string) => key.split("|") as [string, string];

// what the registry holds: this PC has a few values changed from the Windows default
const cur = new Map<string, Val>();
cur.set(id("#prio", "Win32PrioritySeparation"), 0x26);
cur.set(id("#fg", "ForegroundLockTimeout"), 200000);
for (const [n] of SYS_FIELDS) cur.set(id("", n), defaultOf("", n));
for (const t of TASKS) for (const [n] of TASK_FIELDS) cur.set(id(t, n), defaultOf(t, n));
cur.set(id("", "SystemResponsiveness"), 10);
cur.set(id("Games", "Priority"), 6);
cur.set(id("Games", "Scheduling Category"), "High");
const edit = new Map(cur);

let section: SchedulerSection = "Processor";
let task = TASKS.indexOf("Games");
let report: { text: string; tone: Tone }[] = [];
let reportFor: SchedulerSection = "Processor";
const history: { section: SchedulerSection; changes: [string, Val, Val][] }[] = [];

function status(name: string, c: Val, v: Val, def: Val): { text: string; tone: Tone } {
  if (v !== c) return { text: `will change (now ${show(name, c)})`, tone: "Accent" };
  if (def !== null && c !== def) return { text: `changed - default ${show(name, def)}`, tone: "Ok" };
  if (def !== null) return { text: "default", tone: "Faint" };
  return { text: "", tone: "Faint" };
}

function pending(s: SchedulerSection): [string, Val, Val][] {
  return [...edit].filter(([k, v]) => sectionOf(split(k)[0]) === s && cur.get(k) !== v).map(([k, v]) => [k, cur.get(k) ?? null, v]);
}

function line([k, a, b]: [string, Val, Val]): string {
  const [t, n] = split(k);
  return `${place(t, n)}: ${show(n, a)} → ${show(n, b)}`;
}

function parts(v: number): [number, number, number] {
  const fix = (x: number) => (x === 3 ? 0 : x);
  return [fix((v >> 4) & 3), fix((v >> 2) & 3), Math.min(v & 3, 2)];
}

function row(t: string, name: string, label: string): SchedulerRow {
  const k = id(t, name);
  const v = edit.get(k) ?? null;
  const base = { task: t, name, label, text: show(name, v), status: status(name, cur.get(k) ?? null, v, defaultOf(t, name)) };
  if (name === "Clock Rate") return { ...base, control: "number", number: (v as number) ?? 10000, min: 0, max: 1000000 };
  if (name === "Affinity") {
    const mask = (v as number) ?? 0;
    return { ...base, control: "affinity", all: mask === 0, cpus: Array.from({ length: CPUS }, (_, c) => mask === 0 || (mask & (1 << c)) !== 0) };
  }
  const opts = choices(name);
  if (!opts.includes(v)) opts.push(v);
  return { ...base, control: "choice", selected: opts.indexOf(v), options: opts.map((o) => show(name, o)) };
}

function setPart(index: 0 | 1 | 2, value: number) {
  const k = id("#prio", "Win32PrioritySeparation");
  const p = edit.get(k) as number;
  const before = parts(p);
  const next = [...before] as [number, number, number];
  next[index] = value;
  if (next.join() !== before.join()) edit.set(k, (next[0] << 4) | (next[1] << 2) | next[2]);
}

export const scheduler: MockTab<SchedulerView, SchedulerAction> = {
  view() {
    const p = edit.get(id("#prio", "Win32PrioritySeparation")) as number;
    const cp = cur.get(id("#prio", "Win32PrioritySeparation")) as number;
    const [interval, length, boost] = parts(p);
    const now = parts(cp);
    const opts = (picked: number, applied: number, list: [number, string][]): SchedulerOption[] => list.map(([value, label]) => ({ value, label, on: picked === value, now: applied === value }));
    const fg = edit.get(id("#fg", "ForegroundLockTimeout")) as number;
    const todo = pending(section);
    const changed = pending("Multimedia").map(([k]) => split(k)[0]);
    return {
      can: mockAdmin,
      section,
      sections: [
        { id: "Processor", name: "Processor scheduling", dirty: pending("Processor").length > 0 },
        { id: "Multimedia", name: "Multimedia scheduler", dirty: pending("Multimedia").length > 0 },
      ],
      applied: [...cur]
        .filter(([k]) => sectionOf(split(k)[0]) === section)
        .flatMap(([k, v]) => {
          const [t, n] = split(k);
          const def = defaultOf(t, n);
          return def !== null && v !== def ? [`${place(t, n)}: ${show(n, v)} (default ${show(n, def)})`] : [];
        }),
      processor:
        section === "Processor"
          ? {
              interval: opts(interval, now[0], [
                [0, "Let Windows choose (short on this PC)"],
                [2, "Short"],
                [1, "Long"],
              ]),
              length: opts(length, now[1], [
                [0, "Let Windows choose (variable on this PC)"],
                [2, "Fixed length"],
                [1, "Variable length"],
              ]),
              boost: opts(boost, now[2], [
                [0, "No foreground boost (1:1)"],
                [1, "Foreground double length intervals (2:1)"],
                [2, "Foreground triple length intervals (3:1)"],
              ]),
              boostEnabled: mockAdmin && length !== 2,
              foreground: fg,
              foregroundMax: 2000000,
              foregroundStatus: status("ForegroundLockTimeout", cur.get(id("#fg", "ForegroundLockTimeout")) ?? null, fg, 200000),
              bitmask: p.toString(16).toUpperCase(),
              bitmaskStatus: status("Win32PrioritySeparation", cp, p, 2),
            }
          : null,
      multimedia:
        section === "Multimedia"
          ? {
              system: SYS_FIELDS.map(([n, label]) => row("", n, label)),
              tasks: TASKS.map((t) => (changed.includes(t) ? `${t}  •` : t)),
              task,
              rows: TASK_FIELDS.map(([n, label]) => row(TASKS[task], n, label)),
            }
          : null,
      pending: todo.map(line),
      canApply: mockAdmin && todo.length > 0,
      canCancel: todo.length > 0,
      canUndo: mockAdmin && history.some((h) => h.section === section),
      report: reportFor === section ? report : [],
    };
  },
  act(a) {
    const edits = !["section", "task", "cancel", "backup", "reload"].includes(a.kind);
    if (edits && !mockAdmin) return;
    switch (a.kind) {
      case "section":
        section = a.section;
        break;
      case "task":
        task = a.index;
        break;
      case "interval":
        setPart(0, a.value);
        break;
      case "length":
        setPart(1, a.value);
        break;
      case "boost":
        setPart(2, a.value);
        break;
      case "foreground":
        edit.set(id("#fg", "ForegroundLockTimeout"), a.value);
        break;
      case "restore_defaults":
        edit.set(id("#prio", "Win32PrioritySeparation"), 2);
        edit.set(id("#fg", "ForegroundLockTimeout"), 200000);
        break;
      case "choose": {
        const list = choices(a.name);
        if (a.index < list.length) edit.set(id(a.task, a.name), list[a.index]);
        break;
      }
      case "number":
        edit.set(id(a.task, a.name), a.value);
        break;
      case "affinity_all":
        edit.set(id(a.task, "Affinity"), a.on ? 0 : 1);
        break;
      case "affinity_cpu": {
        const k = id(a.task, "Affinity");
        const mask = (edit.get(k) as number) ?? 0;
        const base = mask === 0 ? FULL : mask;
        const m = a.on ? base | (1 << a.cpu) : base & ~(1 << a.cpu);
        if (m !== 0) edit.set(k, (m & FULL) === FULL ? 0 : m);
        break;
      }
      case "apply": {
        const todo = pending(section);
        if (todo.length === 0) break;
        report = [{ text: `Applied ${todo.length} change(s):`, tone: "Ok" }];
        if (!history.some((h) => h.section === section)) {
          const file = section === "Processor" ? "priority-backup-2026-11-16-21-14-03.reg" : "mmcss-backup-2026-11-16-21-14-03.reg";
          report.push({ text: `Backup of the original values: C:\\ProgramData\\PCTuner\\${file}`, tone: "Dim" });
        }
        for (const c of todo) {
          report.push({ text: `  ${line(c)}`, tone: "Text" });
          cur.set(c[0], c[2]);
        }
        reportFor = section;
        history.push({ section, changes: todo });
        break;
      }
      case "cancel":
        for (const [k, v] of cur) if (sectionOf(split(k)[0]) === section) edit.set(k, v);
        break;
      case "undo": {
        const i = history.map((h) => h.section).lastIndexOf(section);
        if (i < 0) break;
        const [h] = history.splice(i, 1);
        report = [{ text: `Undid ${h.changes.length} change(s) from 2026-11-16 21:14:03:`, tone: "Ok" }];
        for (const [k, old, val] of [...h.changes].reverse()) {
          report.push({ text: `  ${line([k, val, old])}`, tone: "Text" });
          cur.set(k, old);
          edit.set(k, old);
        }
        reportFor = section;
        break;
      }
      case "backup":
        reportFor = section;
        report = [{ text: `Backup saved: C:\\ProgramData\\PCTuner\\${section === "Processor" ? "priority" : "mmcss"}-backup-2026-11-16-21-14-03.reg`, tone: "Ok" }];
        break;
      case "reload":
        for (const [k, v] of cur) if (sectionOf(split(k)[0]) === section) edit.set(k, v);
        break;
    }
  },
};
