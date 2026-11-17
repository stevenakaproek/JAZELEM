import type { Action, Compare, ProcRow, Side, View } from "../tabs/Processes";
import { mockAdmin, mockParams } from "./env";
import { mockBackups } from "./backups";
import type { MockTab } from "./types";

type Item = [exe: string, title: string, kind: ProcRow["kind"], mb: number | null, what: string, loses: string];

const items: Item[] = [
  ["CrossDeviceResume", "Resume from phone", "off", 61, "Offers to continue on this PC what you were doing in an app or OneDrive file on your phone.", "The \"resume on PC\" pop-ups from your phone (Settings > Apps > Resume). Phone Link itself is not affected."],
  ["TabTip", "Touch keyboard", "off", 96, "The on-screen keyboard for touchscreens and tablets.", "The touch keyboard no longer pops up by itself. Your real keyboard, emoji panel (Win + .) and the Accessibility on-screen keyboard still work."],
  ["AppActions", "App actions", "lower", 46, "Suggests actions from apps for what is on screen (Click to Do).", "Nothing - suggestions may appear a moment later."],
  ["WidgetBoard", "Widgets board", "lower", 28, "The news and weather Widgets panel (Win + W).", "Nothing - Widgets may open a little slower."],
  ["WidgetService", "Widgets service", "lower", 24, "Feeds data to the Widgets panel.", "Nothing - widget data may refresh a little later."],
  ["SecurityHealthSystray", "Windows Security tray icon", "lower", 18, "Only the shield icon in the taskbar corner.", "Nothing - the icon reacts a moment later."],
  ["PhoneExperienceHost", "Phone Link", "lower", null, "Phone Link's background part.", "Nothing - phone notifications may arrive a moment later."],
  ["vctip", "Visual Studio build telemetry", "lower", 57, "Sends compiler usage data to Microsoft while you build code.", "Nothing."],
  ["HaloInfinite", "Your games", "keep", 4824, "Games and their launchers.", "Anti-cheat can flag a game whose priority is changed by another program."],
  ["GameBar", "Xbox Game Bar", "keep", 159, "Game Bar, captures and the Xbox overlay.", "Needed for Game Mode, captures and Xbox party chat."],
  ["audiodg", "Windows audio", "keep", 34, "Plays all sound.", "A lower priority causes crackling audio."],
  ["dwm", "Desktop Window Manager", "keep", 142, "Draws every window and the game's frames to the screen.", "Lowering it causes stutter."],
];

// exe -> what PC Tuner did
const changed = new Map<string, { what: "off" | "lower"; when: string }>();
let note: View["note"] = null;
let measuring: number | null = null;
const measures: Side[] = [];
const rowsAt: { mb: number; cpu: string }[][] = [];

const now = () => {
  const d = new Date();
  return `2026-11-16 ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;
};

// "?state=again": Windows started the switched-off touch keyboard again after sign-in
const again = mockParams.get("state") === "again";
if (again) changed.set("TabTip", { what: "off", when: "2026-11-16 13:43:47" });

function liveMb(exe: string, mb: number | null): number | null {
  if (again && exe === "TabTip") return mb;
  return changed.get(exe)?.what === "off" ? null : mb;
}

function row([exe, title, kind, mb, what, loses]: Item): ProcRow {
  const c = changed.get(exe);
  const m = liveMb(exe, mb);
  const can = mockAdmin && !c;
  return {
    exe,
    file: `${exe}.exe`,
    title,
    what,
    loses,
    kind,
    running: m === null ? 0 : 1,
    mb: m,
    state: kind === "keep" ? "left alone" : c ? (c.what === "off" ? (m !== null ? "off, but running again (low priority)" : "switched off") : "lower priority") : "as Windows set it",
    tone: kind === "keep" ? "Dim" : c ? (c.what === "off" && m !== null ? "Warn" : "Ok") : "Text",
    changed: !!c,
    when: c?.when ?? null,
    detail: c ? `HKLM\\...\\Image File Execution Options\\${exe}.exe\\PerfOptions  CpuPriorityClass = 5\nHKLM\\...\\Image File Execution Options\\${exe}.exe\\PerfOptions  IoPriority = 1` : null,
    canOff: can && kind === "off",
    canLower: can && kind !== "keep",
    canUndo: mockAdmin && !!c,
  };
}

function compare(): Compare | null {
  if (!measures.length) return null;
  const n = measures.length;
  const changeable = items.filter((i) => i[2] !== "keep");
  return {
    before: n >= 2 ? measures[n - 2] : null,
    after: measures[n - 1],
    rows: changeable.map((i, k) => ({
      title: i[1],
      file: `${i[0]}.exe`,
      before: n >= 2 ? (rowsAt[n - 2][k].mb ? rowsAt[n - 2][k] : { mb: 0, cpu: "0.00", gone: true }) : null,
      after: rowsAt[n - 1][k].mb ? rowsAt[n - 1][k] : { mb: 0, cpu: "0.00", gone: true },
    })),
    savedMb: n >= 2 ? measures[n - 2].listedMb - measures[n - 1].listedMb : null,
    count: n,
    // the stand-in pretends a game ran during the first measurement only
    warnings: n >= 2 && n % 2 === 0 ? ["A game was running during the first measurement but not the second, so the whole-PC numbers mostly show the game closing."] : [],
  };
}

function finishMeasure() {
  const changeable = items.filter((i) => i[2] !== "keep");
  const cells = changeable.map((i) => {
    const m = liveMb(i[0], i[3]) ?? 0;
    const low = changed.has(i[0]);
    return { mb: m, cpu: m ? (low ? "0.01" : "0.04") : "0.00" };
  });
  rowsAt.push(cells);
  const mb = cells.reduce((a, c) => a + c.mb, 0);
  measures.push({
    time: now(),
    setup: changed.size ? [...changed].map(([e, c]) => `${e} ${c.what === "off" ? "off" : "lowered"}`).join(", ") : "no changes",
    listedMb: mb,
    listedCpu: cells.reduce((a, c) => a + Number(c.cpu), 0).toFixed(2),
    pcCpu: (6.2 - changed.size * 0.1).toFixed(1),
    ramMb: 13400 - (330 - mb),
  });
}

const shownOf = (c: { what: "off" | "lower" } | undefined) => (c ? (c.what === "off" ? "Switched off" : "Lower priority") : "As Windows set it");
const bk = mockBackups(
  "A backup saves which processes PC Tuner has switched off or lowered. Restore undoes or makes only the changes that differ, one after another, and each one can still be undone here.",
  () => items.filter((i) => i[2] !== "keep").map((i) => ({ id: i[0], title: i[1], value: changed.get(i[0])?.what ?? null, shown: shownOf(changed.get(i[0])) })),
  (diffs) => {
    for (const d of diffs) {
      if (d.backup.value === null) changed.delete(d.backup.id);
      else changed.set(d.backup.id, { what: d.backup.value as "off" | "lower", when: now() });
    }
    note = { ok: true, text: "Backup restored." };
    return null;
  },
  (it) => ({ ...it, value: it.value ? null : "lower", shown: it.value ? "As Windows set it" : "Lower priority" }),
);

export const procs: MockTab<View, Action> = {
  view() {
    const rows = items.map(row);
    const changeable = rows.filter((r) => r.kind !== "keep" && r.running > 0);
    return {
      loading: false,
      refreshing: false,
      admin: mockAdmin,
      busy: null,
      note,
      rows,
      hero: { running: changeable.length, runningMb: changeable.reduce((a, r) => a + (r.mb ?? 0), 0), changed: changed.size },
      canUndoAll: mockAdmin && changed.size > 0,
      signIn: [...changed.values()].some((c) => c.what === "off") ? "Closed again 30 s and 2 min after every sign-in by the task \\PCTuner\\CloseSwitchedOff (Undo removes it)." : null,
      measuring: measuring === null ? null : { secs: Math.min(30, Math.floor(measuring)), of: 30 },
      compare: compare(),
      backups: bk.view(mockAdmin),
    };
  },
  act(a) {
    if (bk.act(a)) return;
    if (a.kind === "dismiss_note") note = null;
    else if (a.kind === "measure") measuring = measuring ?? 0;
    else if (a.kind === "refresh") return;
    else if (!mockAdmin) return;
    else if (a.kind === "undo_all") {
      changed.clear();
      note = { ok: true, text: "Undo all: done." };
    } else if (a.kind === "undo") {
      changed.delete(a.exe);
      note = { ok: true, text: `${items.find((i) => i[0] === a.exe)?.[1]} undone: done.` };
    } else if (a.kind === "off" || a.kind === "lower") {
      changed.set(a.exe,{ what: a.kind, when: now() });
      const title = items.find((i) => i[0] === a.exe)?.[1];
      note = { ok: true, text: `${title} ${a.kind === "off" ? "switched off: done (closed the running copy)" : "lowered: done (lowered the running copy)"}.` };
    }
  },
  tick() {
    if (measuring === null) return false;
    // the stand-in runs 10x faster than the real 30 s
    measuring += 5;
    if (measuring >= 30) {
      measuring = null;
      finishMeasure();
    }
    return true;
  },
};
