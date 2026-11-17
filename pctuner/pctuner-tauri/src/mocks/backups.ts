import type { BackupAction, TabBackupsView } from "../components/TabBackups";
import { mockAdmin } from "./env";
import { MockOp, makeOp, opItem } from "./op";

/** One setting as a backup stores it (same shape as pctuner_core::tabbackup::Item). */
export interface BkItem {
  id: string;
  title: string;
  value: string | null;
  shown: string;
}

export interface BkDiff {
  backup: BkItem;
  now: BkItem;
}

/**
 * The Backups sheet's stand-in for a mock tab. `now` = the settings now; `restore` puts the differing ones
 * back (open the tab's preview) and returns null, or returns why it could not.
 * It starts with one older backup in which the first two settings differ, so Restore has something to show.
 */
export function mockBackups(about: string, now: () => BkItem[], restore: (diffs: BkDiff[]) => string | null, older?: (it: BkItem) => BkItem) {
  let open = false;
  let msg: TabBackupsView["msg"] = null;
  let list: { time: string; items: BkItem[] }[] | null = null;

  const flip = (it: BkItem): BkItem =>
    it.value === "on" || it.value === "off"
      ? { ...it, value: it.value === "on" ? "off" : "on", shown: it.value === "on" ? "Off" : "On" }
      : { ...it, value: it.value === null ? "1" : null, shown: it.value === null ? "1" : "Not set" };
  const seed = () => (list ??= [{ time: "2026-11-15 20:12:03", items: now().map((it, i) => (i < 2 ? (older ?? flip)(it) : it)) }]);
  const diffs = (items: BkItem[]): BkDiff[] => {
    const n = now();
    return items.flatMap((b) => {
      const c = n.find((x) => x.id === b.id);
      return c && c.value !== b.value ? [{ backup: b, now: c }] : [];
    });
  };
  const stamp = () => {
    const d = new Date();
    const p = (x: number) => String(x).padStart(2, "0");
    return `2026-11-16 ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  };

  return {
    view(can: boolean): TabBackupsView | null {
      if (!open) return null;
      const n = now();
      return {
        about,
        folder: "C:\\Users\\you\\Documents\\PCTuner-Backups",
        msg,
        canCreate: true,
        admin: mockAdmin,
        list: seed().map((b, index) => {
          const d = diffs(b.items);
          return {
            index,
            time: b.time,
            detail: `${b.items.length} settings`,
            known: true,
            diffs: d.map((x) => ({ title: x.backup.title, now: x.now.shown, backup: x.backup.shown })),
            newSince: n.filter((x) => !b.items.some((y) => y.id === x.id)).length,
            canRestore: can && mockAdmin && d.length > 0,
          };
        }),
      };
    },
    /** Handles a Backups action; false = not one of them. */
    act(a: { kind: string }): boolean {
      const b = a as BackupAction;
      switch (b.kind) {
        case "backups":
          open = true;
          msg = null;
          return true;
        case "backups_close":
          open = false;
          return true;
        case "backup_create":
          seed().unshift({ time: stamp(), items: now() });
          msg = { ok: true, text: "Backup saved (stand-in: nothing is written)." };
          return true;
        case "backup_folder":
          return true;
        case "backup_restore": {
          const bk = seed()[b.index];
          if (!bk) return true;
          const err = mockAdmin ? restore(diffs(bk.items)) : "Run PC Tuner as administrator to restore a backup.";
          if (err) msg = { ok: false, text: err };
          else open = false;
          return true;
        }
        case "backup_delete": {
          const bk = seed()[b.index];
          if (!bk) return true;
          seed().splice(b.index, 1);
          msg = { ok: true, text: `Backup from ${bk.time} moved to the Recycle Bin.` };
          return true;
        }
        default:
          return false;
      }
    },
  };
}

/** Restore through a pretend preview window: the differing settings, then `apply` on each when it runs. */
export function restoreOp(op: MockOp, kicker: string, diffs: BkDiff[], apply: (d: BkDiff) => void): string | null {
  if (op.op) return "Another change is still open - finish it first.";
  if (!diffs.length) return "Nothing to restore - every setting already matches this backup.";
  op.open(
    makeOp(kicker, "restore", "restored", diffs.map((d) => opItem(d.backup.id, d.backup.title, "From the backup", `${d.now.shown} -> ${d.backup.shown}`)), {
      noun: ["setting", "settings"],
      go_label: "Restore",
      preview_text: "Puts these back to how they were in the backup. Each change is saved so it can be undone.",
      running_text: "Restoring...",
    }),
    () => diffs.forEach(apply),
  );
  return null;
}