import type { Action, DiskView, StepView, View } from "../tabs/Health";
import { mockAdmin, mockParams } from "./env";
import type { MockTab } from "./types";

type Id = StepView["id"];
type Last = NonNullable<StepView["last"]>;

const TITLES: Record<Id, string> = {
  chkdsk: "CHKDSK - file system on C:",
  dism: "DISM - Windows component store",
  sfc: "SFC - Windows system files",
};
const ORDER: Id[] = ["chkdsk", "dism", "sfc"];

const disk: DiskView = {
  loading: false,
  refreshing: false,
  time: "2026-11-16 17:02:11",
  error: null,
  drives: [
    { name: "Samsung SSD 990 PRO 2TB", kind: "NVMe SSD", sizeGb: 1863, health: "Healthy", wear: 2, temp: 41, errors: 0, hours: 3120, warnings: [], level: "ok" },
    { name: "WD Blue SN580 1TB", kind: "NVMe SSD", sizeGb: 932, health: "Healthy", wear: 1, temp: 38, errors: 0, hours: 1460, warnings: [], level: "ok" },
  ],
  cFree: 612.4,
  cSize: 1862.4,
  lowSpace: null,
};

interface Run {
  id: Id;
  phase: number;
  pct: number;
  elapsed: number;
  sinceMove: number;
  stage: string;
  /** ticks left to wait at the current % (DISM's 62.3 % pause) */
  hold: number;
  stopping: boolean;
}

const DISM_PHASES = ["Quick check (1 of 3)", "Scanning the component store (2 of 3)", "Repairing from Windows Update (3 of 3)"];

const stalledState = mockParams.get("state") === "stalled";
let run: Run | null = stalledState ? { id: "dism", phase: 2, pct: 62.3, elapsed: 2240, sinceMove: 640, stage: "", hold: Infinity, stopping: false } : null;
let queue: Id[] = stalledState ? ["sfc"] : [];
let all = false;
let lastAll: string | null = null;
let note: View["note"] = null;
const last: Partial<Record<Id, Last>> = {};

const now = () => {
  const d = new Date();
  const p = (x: number) => String(x).padStart(2, "0");
  return `2026-11-16 ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
};

function start(id: Id) {
  run = { id, phase: 0, pct: 0, elapsed: 0, sinceMove: 0, stage: id === "sfc" ? "Beginning verification phase of system scan" : "", hold: 0, stopping: false };
}

function next() {
  const id = queue.shift();
  if (id) start(id);
  else {
    run = null;
    if (all) lastAll = now();
    all = false;
  }
}

function finish(r: Run, res: Omit<Last, "time" | "secs" | "hasLog">) {
  last[r.id] = { ...res, time: now(), secs: r.elapsed, hasLog: true };
  next();
}

/** One tick of the stand-in = 15 seconds of the real tools. */
function step(r: Run) {
  if (r.stopping) {
    finish(r, { level: "warn", summary: "Stopped by you - nothing was finished", detail: [], canFix: false });
    queue = [];
    all = false;
    return;
  }
  r.elapsed += 15;
  if (r.hold > 0) {
    r.hold--;
    r.sinceMove += 15;
    return;
  }
  const move = (to: number) => {
    r.pct = Math.min(100, to);
    r.sinceMove = 0;
  };
  if (r.id === "chkdsk") {
    move(r.pct + 12);
    r.stage = r.pct < 34 ? "Stage 1: Examining basic file system structure" : r.pct < 67 ? "Stage 2: Examining file name linkage" : "Stage 3: Examining security descriptors";
    if (r.pct >= 100) finish(r, { level: "warn", summary: "Problems found on C: - they can be fixed at the next restart (takes seconds)", detail: [], canFix: true });
  } else if (r.id === "sfc") {
    move(r.pct + 15);
    if (r.pct >= 100) finish(r, { level: "ok", summary: "No damaged system files", detail: [], canFix: false });
  } else {
    const steps = [50, 25, 12];
    const before = r.pct;
    move(r.pct + steps[r.phase]);
    // the restore pass waits at 62.3 % while it downloads repair files (8 ticks = 2 minutes)
    if (r.phase === 2 && before < 62.3 && r.pct >= 62.3) {
      r.pct = 62.3;
      r.hold = 8;
    }
    if (r.pct >= 100) {
      if (r.phase < 2) {
        r.phase++;
        r.pct = 0;
      } else finish(r, { level: "ok", summary: "Damage found and repaired from Windows Update", detail: [], canFix: false });
    }
  }
}

function stepView(id: Id): StepView {
  const r = run && run.id === id ? run : null;
  const busy = !!r && !stalledState;
  const note =
    r && r.sinceMove >= 30
      ? r.id === "dism" && r.phase === 2
        ? "DISM often waits here for many minutes while it downloads repair files from Windows Update."
        : "It can stay on one % for several minutes."
      : null;
  return {
    id,
    title: TITLES[id],
    running: r
      ? {
          phase: r.id === "dism" ? DISM_PHASES[r.phase] : "",
          pct: r.pct || null,
          stage: r.stage,
          elapsed: r.elapsed,
          sinceMove: r.sinceMove,
          activity: busy ? (r.id === "dism" ? "TiWorker 14 % CPU · 3.0 MB/s · DismHost 2 % CPU · 0.4 MB/s" : r.id === "sfc" ? "TiWorker 9 % CPU · 22.0 MB/s" : "chkdsk 6 % CPU · 48.0 MB/s") : null,
          busy,
          note,
          stalled: stalledState,
          stopping: r.stopping,
        }
      : null,
    queued: queue.includes(id),
    canRun: mockAdmin && !run,
    last: last[id] ? { ...last[id]!, canFix: last[id]!.canFix && mockAdmin && !run } : null,
  };
}

export const health: MockTab<View, Action> = {
  view() {
    const levels = ORDER.map((i) => last[i]?.level).filter(Boolean);
    const overall = !levels.length ? "Not checked yet" : levels.includes("error") || levels.includes("warn") ? "Needs attention" : "All good";
    return {
      admin: mockAdmin,
      overall,
      tone: overall === "All good" ? "Ok" : overall === "Needs attention" ? (levels.includes("error") ? "Error" : "Warn") : "Text",
      lastAll,
      running: !!run,
      canRunAll: mockAdmin && !run,
      canStop: !!run && !run.stopping,
      note,
      prep: "Windows Modules Installer, Background Intelligent Transfer Service are Disabled. DISM and SFC hang or fail without them, so they are set to Manual just for the check and put back to Disabled afterwards.",
      disk,
      steps: ORDER.map(stepView),
    };
  },
  act(a) {
    switch (a.kind) {
      case "run_all":
        if (!mockAdmin || run) return;
        all = true;
        queue = [...ORDER];
        next();
        break;
      case "run":
        if (!mockAdmin || run) return;
        all = false;
        queue = [];
        start(a.step as Id);
        break;
      case "stop":
        if (run) run.stopping = true;
        break;
      case "fix_chkdsk":
        if (!mockAdmin || !last.chkdsk) return;
        last.chkdsk = { ...last.chkdsk, canFix: false, detail: ["CHKDSK will fix C: the next time the PC restarts (a few seconds at boot)."] };
        note = { ok: true, text: "CHKDSK will fix C: the next time the PC restarts (a few seconds at boot)." };
        break;
      case "dismiss_note":
        note = null;
        break;
      case "refresh_disk":
      case "open_log":
        break;
    }
  },
  tick() {
    if (!run || stalledState) return false;
    step(run);
    return true;
  },
};
