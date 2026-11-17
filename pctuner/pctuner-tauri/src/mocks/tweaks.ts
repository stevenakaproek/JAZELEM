import type { TweakRow, TweaksAction, TweaksView } from "../tabs/Tweaks";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import { mockBackups } from "./backups";
import type { MockTab } from "./types";

const TASKS = "Scheduled tasks";
const EDGE = "Edge";
const XBOX = "Xbox networking";
const ONEDRIVE = "OneDrive";
const GROUPS = [TASKS, EDGE, XBOX, ONEDRIVE];
const ONEDRIVE_WARNING = 'OneDrive will be uninstalled. "Undo tweaks" does not bring it back - reinstall it from the Microsoft Store if you want it again.';

type Seed = [name: string, text: string, applied: boolean, off: string, on: string];

// [name, explanation, applied, value when not applied, value when applied]
const seeds: Seed[] = [
  ["Telemetry and tips tasks off", "32 telemetry, tips, maps and family-safety tasks are disabled. Xbox and Steam tasks are kept.", false, "27 still on", "All off"],
  ["Edge preloading off", "Startup Boost and background mode off. Edge stays installed.", true, "On", "Off"],
  ["Teredo on for Xbox online play", 'Teredo set to "enterpriseclient" by Group Policy so Windows keeps it.', false, "client", "enterpriseclient"],
  ["Remove OneDrive", 'Uninstalls OneDrive. "Undo tweaks" does not bring it back.', false, "Installed", "Not installed"],
];

const applied = seeds.map((s) => s[2]);
const selected = [false, false, false, false];
const op = new MockOp();
const state = mockParams.get("state");
/** Ticks left until the status read finishes (0 = known). */
let checking = state === "loading" ? Infinity : 0;
let lastTitle: string | null = null;
let log: string[] | null = state === "error" ? ["Run this script from an elevated (Administrator) PowerShell."] : null;
let exit: number | null = state === "error" ? 1 : null;
let showDetails = state === "error";
if (state === "error") lastTitle = "Apply tweaks";

const running = () => op.op?.stage === "Running";

function title(groups: number[]): string {
  if (groups.length === 4) return "Apply + remove OneDrive";
  if (groups.length === 3 && !groups.includes(3)) return "Apply tweaks";
  if (groups.length > 1) return "Apply selected tweaks";
  return ["Disable the scheduled tasks", "Stop Edge preloading", "Turn on Teredo", "Remove OneDrive"][groups[0]];
}

function logFor(groups: number[], undo: boolean): string[] {
  const out: string[] = [];
  if (groups.includes(0)) {
    out.push(
      "Scheduled tasks:",
      `  ${undo ? "enabled " : "disabled"}  \\Microsoft\\Windows\\Application Experience\\Microsoft Compatibility Appraiser`,
      `  ${undo ? "enabled " : "disabled"}  \\Microsoft\\Windows\\Customer Experience Improvement Program\\Consolidator`,
      `  ${undo ? "enabled " : "disabled"}  \\Microsoft\\Windows\\Maps\\MapsToastTask`,
      "  skip      \\Microsoft\\Windows\\RetailDemo\\CleanupOfflineContent (not present)",
      `  ${undo ? "enabled " : "disabled"}  \\Microsoft\\Windows\\Maintenance\\WinSAT`,
      "",
    );
  }
  if (groups.includes(1)) out.push("Edge background launching:", undo ? "  Edge policies removed (Edge back to defaults)" : '  Startup Boost and background mode turned off (Edge shows "Managed by your organization" - expected)', "");
  if (groups.includes(2)) out.push("Xbox networking (Teredo):", undo ? "  Teredo put back as it was before the tweaks (type client)" : "  Teredo: enterpriseclient via win1910.ipv6.microsoft.com -> state 'qualified', router UPnP 'Yes'", "");
  if (groups.includes(3)) out.push("OneDrive:", "  OneDrive removed (reinstall any time from the Microsoft Store or winget).", "");
  out.push("Done.");
  return out;
}

function chips(lines: string[]): { text: string; ok: boolean }[] {
  const count = (from: string, match: (l: string) => boolean) => {
    const start = lines.indexOf(from);
    if (start < 0) return 0;
    let n = 0;
    for (let i = start + 1; i < lines.length && lines[i].startsWith("  "); i++) if (match(lines[i])) n++;
    return n;
  };
  const sections: [string, string][] = [
    [TASKS, "Scheduled tasks:"],
    [EDGE, "Edge background launching:"],
    [XBOX, "Xbox networking (Teredo):"],
    [ONEDRIVE, "OneDrive:"],
  ];
  return sections
    .map(([g, header]) => {
      const total = count(header, () => true);
      const failed = count(header, (l) => l.includes("FAIL"));
      return { g, total, failed };
    })
    .filter((s) => s.total > 0)
    .map((s) => ({ text: `${s.g}  ${s.total - s.failed}/${s.total}`, ok: s.failed === 0 }));
}

function ask(groups: number[], undo: boolean) {
  const go = undo ? "Undo tweaks" : title(groups);
  const items = groups.map((g) => opItem(GROUPS[g], ["Scheduled tasks", "Edge background launching", "Xbox networking (Teredo)", "OneDrive"][g], GROUPS[g]));
  op.open(
    makeOp("POST-INSTALL TWEAKS", undo ? "undo" : "apply", undo ? "undone" : "applied", items, {
      noun: ["tweak", "tweaks"],
      go_label: go,
      undo_label: "Undo tweaks",
      preview_text: undo
        ? "Puts back the state saved before the tweaks on 2026-11-12 10:00:00: tasks that were on are switched back on, the Edge and Teredo policies go back to what they were."
        : groups.length === 1
          ? ["Only the telemetry and tips tasks are switched off. Edge, Teredo and OneDrive are left alone.", "Only Edge's Startup Boost and background mode are switched off. The other tweaks are left alone.", "Only Teredo is turned on for Xbox online play. The other tweaks are left alone.", "Only OneDrive is uninstalled. The other tweaks are left alone."][groups[0]]
          : `Applies: ${groups.map((g) => GROUPS[g]).join(", ")}. The other tweaks are left alone.`,
      running_text: undo ? "Undoing the tweaks..." : "Applying the tweaks...",
      warning: undo
        ? "Teredo goes back to how it was before the tweaks (type client). Xbox party chat and multiplayer may stop connecting until you apply the tweaks again."
        : groups.includes(3)
          ? ONEDRIVE_WARNING
          : null,
      reassure: undo ? "5 tasks already off before the tweaks stay off." : 'The state before is saved first. "Undo tweaks" puts back every tweak applied so far.',
    }),
    () => {
      lastTitle = go;
      log = logFor(groups, undo);
      exit = 0;
      groups.forEach((g) => (applied[g] = !undo));
      selected.fill(false);
      checking = 2;
      // undo is offered after an apply, never after an undo
      if (op.op) op.op.can_undo = !undo;
    },
  );
}

const bk = mockBackups(
  "A backup saves which of these tweaks are in place. Restore applies the ones the backup had. The script can only undo all its tweaks at once, so to take one back use Undo tweaks; OneDrive is never reinstalled.",
  () => seeds.map((s, i) => ({ id: String(i), title: s[0], value: applied[i] ? "on" : "off", shown: applied[i] ? `Applied (${s[4]})` : `Not applied (${s[3]})` })),
  (diffs) => {
    if (op.op) return "Another change is still open - finish it first.";
    const groups = diffs.filter((d) => d.backup.value === "on").map((d) => Number(d.backup.id));
    const left = diffs.filter((d) => d.backup.value !== "on").map((d) => `"${d.backup.title}" is applied now but was not in this backup - "Undo tweaks" puts all tweaks back`);
    if (!groups.length) return left.join("; ") || "Nothing to restore - every tweak already matches this backup.";
    ask(groups, false);
    const w = op.op as { notes: string[] } | null; // opened by ask()
    if (w && left.length) {
      w.notes = [`Not changed by this restore: ${left.join("; ")}.`];
      (w as { preview_text?: string }).preview_text += ` Not changed by this restore: ${left.join("; ")}.`;
    }
    return null;
  },
  (it) => (Number(it.id) < 3 ? { ...it, value: it.value === "on" ? "off" : "on", shown: it.value === "on" ? "Not applied" : "Applied" } : it),
);

export const tweaks: MockTab<TweaksView, TweaksAction> = {
  view() {
    const known = checking === 0;
    const can = mockAdmin && !running();
    const rows: TweakRow[] = seeds.map(([name, text, , off, on], index) => {
      const selectable = can && known && !applied[index];
      return {
        index,
        name,
        text,
        danger: index === 3,
        applied: known ? applied[index] : null,
        selectable,
        selected: selected[index] && selectable,
        now: known ? (applied[index] ? on : off) : null,
      };
    });
    const pending = rows.filter((r) => r.selected).length;
    return {
      rows,
      pending,
      canApply: can && pending > 0,
      canUndo: can,
      canRefresh: !running() && known,
      lastRun:
        log === null && !running()
          ? { title: "Last run", ran: false }
          : {
              title: lastTitle ? `Last run - ${lastTitle}` : "Last run",
              ran: true,
              running: running(),
              exit: running() ? null : exit,
              chips: running() ? [] : chips(log ?? []),
              showDetails,
              lines: showDetails && !running() ? (log ?? []).map((text) => ({ text, fail: text.includes("FAIL") })) : [],
            },
      op: op.op,
      opCan: mockAdmin && !running(),
      backups: bk.view(can && !op.op),
    };
  },
  act(a) {
    if (bk.act(a)) return;
    const can = mockAdmin && !running();
    switch (a.kind) {
      case "toggle":
        if (can && checking === 0 && !applied[a.index]) selected[a.index] = !selected[a.index];
        break;
      case "clear":
        selected.fill(false);
        break;
      case "apply": {
        const groups = selected.map((s, i) => (s && !applied[i] ? i : -1)).filter((i) => i >= 0);
        if (can && groups.length > 0) ask(groups, false);
        break;
      }
      case "undo":
        if (can) ask([0, 1, 2], true);
        break;
      case "refresh":
        if (!running() && checking === 0) checking = 2;
        break;
      case "details":
        showDetails = a.on;
        break;
      case "op":
        if (a.ev === "Go" && op.op) lastTitle = op.op.go_label;
        // the result's "Undo tweaks" opens the same preview as the page's button, never a one-click undo
        if (op.event(a.ev) === "undo" && can) ask([0, 1, 2], true);
        break;
    }
  },
  tick() {
    let changed = op.tick();
    if (checking > 0 && Number.isFinite(checking)) {
      checking--;
      changed = true;
    }
    return changed;
  },
};
