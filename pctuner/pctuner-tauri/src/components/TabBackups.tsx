// The Backups sheet shared by Startup Cleaner, Optimization (each part), Group Policy and Processes:
// save the tab's settings now, see how each saved backup differs from the PC, and restore one (through
// the tab's own preview window).
import { FolderOpen, History, Save } from "lucide-react";
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

export type BackupAction = { kind: "backups" | "backups_close" | "backup_create" | "backup_folder" } | { kind: "backup_restore"; index: number };

const SHOWN = 8;

/** The toolbar button that opens the sheet. */
export function BackupsButton({ act }: { act: (a: BackupAction) => void }) {
  return (
    <Button icon={<History />} title="Save these settings, or put a saved set back." onClick={() => act({ kind: "backups" })}>
      Backups
    </Button>
  );
}

export function TabBackupsSheet({ b, kicker, act }: { b: TabBackupsView; kicker: string; act: (a: BackupAction) => void }) {
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
                <Button variant={x.diffs.length ? "primary" : "default"} disabled={!x.canRestore} title={b.admin ? "Puts back the settings that differ (a preview comes first)." : "Run PC Tuner as administrator."} onClick={() => act({ kind: "backup_restore", index: x.index })}>
                  Restore
                </Button>
              }
            />
          ))}
        </div>
      )}
    </Sheet>
  );
}
