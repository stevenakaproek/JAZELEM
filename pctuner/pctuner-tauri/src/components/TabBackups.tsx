// The Backups sheet shared by Startup Cleaner, Optimization (each part), Group Policy and Processes:
// save the tab's settings now, see how each saved backup differs from the PC, and restore one (through
// the tab's own preview window).
import { FolderOpen, History, Save, Trash2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { Banner, Button, Row, Sheet } from "./ui";
import "../styles/backups.css";

export interface TabBackupsView {
  about: string;
  folder: string;
  msg: { ok: boolean; text: string } | null;
  canCreate: boolean;
  admin: boolean;
  list: {
    index: number;
    time: string;
    detail: string;
    /** the settings now are known (false while they are read) */
    known: boolean;
    diffs: { title: string; now: string; backup: string }[];
    newSince: number;
    canRestore: boolean;
  }[];
}

export type BackupAction = { kind: "backups" | "backups_close" | "backup_create" | "backup_folder" } | { kind: "backup_restore" | "backup_delete"; index: number };

const SHOWN = 8;

/** The toolbar button that opens the sheet. */
export function BackupsButton({ act }: { act: (a: BackupAction) => void }) {
  return (
    <Button icon={<History />} title="Save these settings, or put a saved set back." onClick={() => act({ kind: "backups" })}>
      Backups
    </Button>
  );
}

/**
 * A backup row's buttons: Restore (passed in) and Delete. Delete asks first, in the row itself:
 * "Delete" again confirms (the files go to the Recycle Bin), Cancel keeps the backup.
 */
export function BackupButtons({ time, restore, confirming, onAsk, onCancel, onDelete }: { time: string; restore: ReactNode; confirming: boolean; onAsk: () => void; onCancel: () => void; onDelete: () => void }) {
  if (confirming) {
    return (
      <span className="bk-confirm">
        <span className="tone-warn">Delete the backup from {time}? It goes to the Recycle Bin.</span>
        <span className="hstack">
          <Button variant="danger-filled" icon={<Trash2 />} onClick={onDelete}>
            Delete
          </Button>
          <Button onClick={onCancel}>Cancel</Button>
        </span>
      </span>
    );
  }
  return (
    <span className="hstack">
      {restore}
      <Button variant="danger" icon={<Trash2 />} title="Delete this backup (asks first; it goes to the Recycle Bin)." onClick={onAsk}>
        Delete
      </Button>
    </span>
  );
}

export function TabBackupsSheet({ b, kicker, act }: { b: TabBackupsView; kicker: string; act: (a: BackupAction) => void }) {
  const [confirm, setConfirm] = useState<string | null>(null);
  return (
    <Sheet
      kicker={kicker}
      title="Backups"
      subtitle={b.about}
      onClose={() => act({ kind: "backups_close" })}
      footer={
        <Button size="lg" onClick={() => act({ kind: "backups_close" })}>
          Close
        </Button>
      }
    >
      {b.msg && <Banner kind={b.msg.ok ? "ok" : "error"}>{b.msg.text}</Banner>}
      <div className="toolbar">
        <Button variant="primary" icon={<Save />} disabled={!b.canCreate} title={b.canCreate ? undefined : "The settings are still being read."} onClick={() => act({ kind: "backup_create" })}>
          Back up now
        </Button>
        <Button icon={<FolderOpen />} title={b.folder} onClick={() => act({ kind: "backup_folder" })}>
          Open backup folder
        </Button>
      </div>
      {!b.admin && <div className="tone-warn">Backing up works without administrator; restoring needs PC Tuner started as administrator.</div>}
      {b.list.length === 0 ? (
        <p className="secondary">No backups yet.</p>
      ) : (
        <div className="bk-scroll">
          {b.list.map((x) => (
            <Row
              key={x.index}
              title={x.time}
              subtitle={
                <>
                  {x.detail}
                  {!x.known ? (
                    <div className="secondary">Reading the settings now...</div>
                  ) : x.diffs.length === 0 ? (
                    <div className="tone-ok">Matches the PC now</div>
                  ) : (
                    <>
                      <div className="tone-junk">
                        {x.diffs.length} {x.diffs.length === 1 ? "differs" : "differ"} from now
                      </div>
                      <ul className="bk-diffs">
                        {x.diffs.slice(0, SHOWN).map((d, i) => (
                          <li key={i}>
                            {d.title}: <span className="secondary">{d.now}</span> → <strong>{d.backup}</strong>
                          </li>
                        ))}
                        {x.diffs.length > SHOWN && <li className="secondary">... and {x.diffs.length - SHOWN} more (all shown in the preview)</li>}
                      </ul>
                    </>
                  )}
                  {x.newSince > 0 && (
                    <div className="tertiary">
                      {x.newSince} new since this backup - left as {x.newSince === 1 ? "it is" : "they are"}
                    </div>
                  )}
                </>
              }
              accessory={
                <BackupButtons
                  time={x.time}
                  confirming={confirm === x.time}
                  onAsk={() => setConfirm(x.time)}
                  onCancel={() => setConfirm(null)}
                  onDelete={() => {
                    setConfirm(null);
                    act({ kind: "backup_delete", index: x.index });
                  }}
                  restore={
                    <Button variant={x.diffs.length ? "primary" : "default"} disabled={!x.canRestore} title={b.admin ? "Puts back the settings that differ (a preview comes first)." : "Run PC Tuner as administrator."} onClick={() => act({ kind: "backup_restore", index: x.index })}>
                      Restore
                    </Button>
                  }
                />
              }
            />
          ))}
        </div>
      )}
    </Sheet>
  );
}
