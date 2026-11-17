// Services: change startup types with full undo, plus the History and Backups sheets.
import { Check as CheckIcon, ChevronDown, ChevronUp, FolderOpen, History as HistoryIcon, RefreshCw, Save } from "lucide-react";
import { Fragment, useState } from "react";
import { ABOUT_HINT, AboutRow, useOpenRows, type About } from "../components/About";
import { BackupButtons } from "../components/TabBackups";
import { OpSheet } from "../components/OpSheet";
import { Badge, Banner, Button, Check, Dot, EmptyState, ErrorState, Group, Hero, LoadingState, Page, Row, SearchField, Sheet, Spinner, Stat, Switch, ViewOnlyBanner, cx, type BadgeColor } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/services.css";

export type SortCol = "name" | "display" | "startup" | "status" | "note" | "suggested";
export type RowTone = "gaming" | "locked" | "safe" | "disabled" | "";

export interface ServiceRow {
  name: string;
  display: string;
  start: string;
  startDisabled: boolean;
  state: string;
  running: boolean;
  note: string;
  tone: RowTone;
  suggested: string | null;
  suggestedDone: boolean;
  selected: boolean;
  /** shown when the row is right-clicked */
  about: About;
}

export interface HistoryRow {
  index: number;
  time: string;
  name: string;
  display: string;
  from: string;
  to: string;
  undone: boolean;
  selected: boolean;
}

export interface HistoryView {
  active: number;
  recorded: number;
  rows: HistoryRow[];
  undoLabel: string;
  canUndo: boolean;
  canClear: boolean;
  busy: boolean;
}

export interface BackupsView {
  /** the built-in "Windows default" row: startup types of a clean Windows 11 install */
  windowsDefault: { detail: string; matches: boolean; diff: string; canRestore: boolean };
  msg: { text: string; ok: boolean } | null;
  canCreate: boolean;
  creating: boolean;
  list: { index: number; time: string; detail: string; matches: boolean; diff: string; canRestore: boolean }[];
  op: OpWindow | null;
  opCan: boolean;
}

export interface View {
  loading: boolean;
  refreshing: boolean;
  busy: boolean;
  hero: { running: number; disabled: number; total: number; undoable: number; chip: string; chipOk: boolean };
  search: string;
  runningOnly: boolean;
  hideLocked: boolean;
  stopOnDisable: boolean;
  sort: SortCol;
  sortAsc: boolean;
  rows: ServiceRow[];
  selText: string;
  selActive: boolean;
  canChange: boolean;
  canClear: boolean;
  canUndo: boolean;
  status: { text: string; error: boolean } | null;
  op: OpWindow | null;
  opCan: boolean;
  history: HistoryView | null;
  backups: BackupsView | null;
}

export type Action =
  | { kind: "toggle"; name: string }
  | { kind: "search"; text: string }
  | { kind: "running_only" | "hide_locked" | "stop_on_disable"; on: boolean }
  | { kind: "sort"; col: SortCol }
  | { kind: "change"; ty: "Disabled" | "Manual" | "Automatic" }
  | { kind: "clear" | "undo_last" | "refresh" | "history" | "history_close" | "hist_undo" | "hist_clear" | "backups" | "backups_close" | "backup_create" | "backup_folder" | "backup_restore_default" }
  | { kind: "hist_toggle"; index: number }
  | { kind: "backup_restore" | "backup_delete"; index: number }
  | { kind: "op" | "backup_op"; ev: OpEvent };

type Act = (a: Action) => Promise<void>;

const SUBTITLE = "Change startup types with full undo. Gaming dependencies warn first; core Windows services are locked.";

const COLUMNS: { col: SortCol; label: string; width: string }[] = [
  { col: "name", label: "Name", width: "19%" },
  { col: "display", label: "Display name", width: "27%" },
  { col: "startup", label: "Startup", width: "15%" },
  { col: "status", label: "Status", width: "9%" },
  { col: "note", label: "Note", width: "16%" },
  { col: "suggested", label: "Suggested", width: "14%" },
];

const TONE_DOT: Record<RowTone, BadgeColor> = { safe: "green", gaming: "orange", locked: "red", disabled: "", "": "" };

function ServiceTable({ view, act }: { view: View; act: Act }) {
  const { open, toggle } = useOpenRows();
  return (
    <table className="table svc-table">
      <caption className="table-hint">{ABOUT_HINT}</caption>
      <colgroup>
        <col className="col-check" />
        {COLUMNS.map((c) => (
          <col key={c.col} style={{ width: c.width }} />
        ))}
      </colgroup>
      <thead>
        <tr>
          <th />
          {COLUMNS.map((c) => {
            const active = view.sort === c.col;
            return (
              <th key={c.col} className={cx("sortable", active && "active")} aria-sort={active ? (view.sortAsc ? "ascending" : "descending") : undefined} onClick={() => act({ kind: "sort", col: c.col })}>
                {c.label}
                {active && (view.sortAsc ? <ChevronUp /> : <ChevronDown />)}
              </th>
            );
          })}
        </tr>
      </thead>
      <tbody>
        {view.rows.map((r) => {
          const dim = r.tone === "disabled";
          const expanded = open.has(r.name);
          return (
            <Fragment key={r.name}>
            <tr
              className={cx("clickable", r.selected && "selected", expanded && "expanded")}
              aria-selected={r.selected}
              aria-expanded={expanded}
              title={ABOUT_HINT}
              onClick={() => act({ kind: "toggle", name: r.name })}
              onContextMenu={(e) => {
                e.preventDefault();
                toggle(r.name);
              }}
            >
              <td>
                <Check on={r.selected} />
              </td>
              <td title={r.name}>
                <span className={cx("svc-name", dim && "secondary")}>
                  {r.tone ? <Dot color={TONE_DOT[r.tone]} /> : <span className="dot hidden" />}
                  <span>{r.name}</span>
                </span>
              </td>
              <td className={dim ? "secondary" : undefined} title={r.display}>
                {r.display}
              </td>
              <td className={r.startDisabled ? "secondary" : undefined}>{r.start}</td>
              <td className={dim || !r.running ? "secondary" : undefined}>{r.state}</td>
              <td className={r.tone === "gaming" ? "tone-junk" : r.tone === "locked" ? "tone-error" : "secondary"}>{r.note}</td>
              <td className="tone-ok">
                {r.suggested}
                {r.suggestedDone && <CheckIcon className="svc-done" aria-label="already set" />}
              </td>
            </tr>
            {expanded && <AboutRow about={r.about} cols={COLUMNS.length + 1} />}
            </Fragment>
          );
        })}
      </tbody>
    </table>
  );
}

function HistorySheet({ h, admin, act }: { h: HistoryView; admin: boolean; act: Act }) {
  return (
    <Sheet
      wide
      kicker="Services"
      title="Change history"
      subtitle="Tick the changes you want to take back, newest first."
      onClose={() => act({ kind: "history_close" })}
      footer={
        <>
          <Button variant="primary" size="lg" disabled={!h.canUndo} onClick={() => act({ kind: "hist_undo" })}>
            {h.undoLabel}
          </Button>
          <Button size="lg" disabled={!h.canClear} title="Removes already-undone rows from this list. Changes nothing on the PC." onClick={() => act({ kind: "hist_clear" })}>
            Clear undone entries
          </Button>
          <Button size="lg" onClick={() => act({ kind: "history_close" })}>
            Close
          </Button>
          {h.busy && <Spinner />}
          {!admin && <span className="tone-error">View only - start PC Tuner as administrator.</span>}
        </>
      }
    >
      <div className="hstack">
        <Badge>{h.recorded} recorded</Badge>
        <Badge color={h.active > 0 ? "accent" : ""}>{h.active} can be undone</Badge>
      </div>
      {h.rows.length === 0 ? (
        <p className="secondary">No changes recorded yet.</p>
      ) : (
        <div className="svc-sheet-scroll">
          <table className="table svc-table">
            <colgroup>
              <col className="col-check" />
              <col style={{ width: "19%" }} />
              <col style={{ width: "19%" }} />
              <col style={{ width: "24%" }} />
              <col style={{ width: "14%" }} />
              <col style={{ width: "14%" }} />
              <col style={{ width: "10%" }} />
            </colgroup>
            <thead>
              <tr>
                <th />
                <th>Time</th>
                <th>Name</th>
                <th>Display name</th>
                <th>From</th>
                <th>To</th>
                <th>State</th>
              </tr>
            </thead>
            <tbody>
              {h.rows.map((r) => (
                <tr key={r.index} className={cx(!r.undone && "clickable", r.selected && "selected", r.undone && "tertiary")} aria-selected={r.selected} onClick={r.undone ? undefined : () => act({ kind: "hist_toggle", index: r.index })}>
                  <td>{!r.undone && <Check on={r.selected} />}</td>
                  <td>{r.time}</td>
                  <td title={r.name}>{r.name}</td>
                  <td title={r.display}>{r.display}</td>
                  <td>{r.from}</td>
                  <td>{r.to}</td>
                  <td className={r.undone ? undefined : "tone-accent"}>{r.undone ? "Undone" : "Active"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Sheet>
  );
}

function BackupsSheet({ b, admin, act }: { b: BackupsView; admin: boolean; act: Act }) {
  const [confirm, setConfirm] = useState<string | null>(null);
  if (b.op) return <OpSheet op={b.op} can={b.opCan} admin={admin} onEvent={(ev) => act({ kind: "backup_op", ev })} />;
  return (
    <Sheet
      kicker="Services"
      title="Backups"
      subtitle="A backup saves the startup type of every service. Each one has three ways back: Restore here, a double-click RESTORE .cmd (works in Safe Mode), and a full .reg export."
      onClose={() => act({ kind: "backups_close" })}
      footer={
        <Button size="lg" onClick={() => act({ kind: "backups_close" })}>
          Close
        </Button>
      }
    >
      {b.msg && <Banner kind={b.msg.ok ? "ok" : "error"}>{b.msg.text}</Banner>}
      <div className="toolbar">
        <Button variant="primary" icon={<Save />} disabled={!b.canCreate} onClick={() => act({ kind: "backup_create" })}>
          Create backup now
        </Button>
        {b.creating && <Spinner />}
        <Button icon={<FolderOpen />} onClick={() => act({ kind: "backup_folder" })}>
          Open backup folder
        </Button>
      </div>
      {!admin && <div className="tone-error">Run PC Tuner as administrator to create or restore backups.</div>}
      <div className="svc-sheet-scroll">
        <Row
          title="Windows default"
          subtitle={
            <>
              {b.windowsDefault.detail}
              <div className={b.windowsDefault.matches ? "tone-ok" : "tone-junk"}>{b.windowsDefault.diff}</div>
            </>
          }
          accessory={
            <Button
              variant="default"
              disabled={!b.windowsDefault.canRestore}
              title="Puts every Windows service back to its startup type on a fresh install (a preview comes first). Wi-Fi and mobile-broadband services are left alone: their default depends on your hardware."
              onClick={() => act({ kind: "backup_restore_default" })}
            >
              Restore defaults
            </Button>
          }
        />
      </div>
      {b.list.length === 0 ? (
        <p className="secondary">No backups yet. PC Tuner also makes one by itself before the first service change.</p>
      ) : (
        <div className="svc-sheet-scroll">
          {b.list.map((x) => (
            <Row
              key={x.index}
              title={x.time}
              subtitle={
                <>
                  {x.detail}
                  <div className={x.matches ? "tone-ok" : "tone-junk"}>{x.diff}</div>
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
                    void act({ kind: "backup_delete", index: x.index });
                  }}
                  restore={
                    <Button variant={x.matches ? "default" : "primary"} disabled={!x.canRestore} title="Puts every service back to the startup type in this backup." onClick={() => act({ kind: "backup_restore", index: x.index })}>
                      Restore this backup
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

function Body({ view, admin, act }: { view: View; admin: boolean; act: Act }) {
  // typed text is kept here so the field stays responsive; the filter itself runs in Rust
  const [query, setQuery] = useState(view.search);
  const search = (text: string) => {
    setQuery(text);
    void act({ kind: "search", text });
  };
  const h = view.hero;
  return (
    <>
      <Group padded>
        <div className="hstack hero-row">
          <Hero value={h.running} unit="running" caption="services active right now" tone="Accent" />
          <Stat value={h.disabled} label="disabled" tone="Dim" />
          <Stat value={h.total} label="total" />
          <Stat value={h.undoable} label="changes that can be undone" tone={h.undoable > 0 ? undefined : "Dim"} />
          <Badge color={h.chipOk ? "green" : ""}>{h.chip}</Badge>
        </div>
      </Group>

      <div className="toolbar">
        <SearchField value={query} onChange={search} placeholder="Search services" />
        <label className="hstack">
          <Switch on={view.runningOnly} onChange={(on) => act({ kind: "running_only", on })} label="Running only" />
          Running only
        </label>
        <label className="hstack" title="Hide core Windows services (they cannot be changed).">
          <Switch on={view.hideLocked} onChange={(on) => act({ kind: "hide_locked", on })} label="Hide locked" />
          Hide locked
        </label>
      </div>

      <div className="toolbar">
        <Badge color={view.selActive ? "accent" : ""}>{view.selText}</Badge>
        <Button variant="danger" disabled={!view.canChange} title="Set selected services to Disabled (they cannot start at all)." onClick={() => act({ kind: "change", ty: "Disabled" })}>
          Disable
        </Button>
        <Button disabled={!view.canChange} title="Set selected services to Manual (start only when something needs them)." onClick={() => act({ kind: "change", ty: "Manual" })}>
          Manual
        </Button>
        <Button disabled={!view.canChange} title="Set selected services to start automatically at boot." onClick={() => act({ kind: "change", ty: "Automatic" })}>
          Automatic
        </Button>
        {view.canClear && (
          <Button title="Clear the selection." onClick={() => act({ kind: "clear" })}>
            Clear
          </Button>
        )}
        <span className="spacer" />
        {view.busy && <Spinner />}
        <Button disabled={!view.canUndo} title="Revert the most recent batch of changes." onClick={() => act({ kind: "undo_last" })}>
          Undo last change
        </Button>
      </div>

      <Group
        footer={
          <div className="svc-legend">
            <span>
              <Dot color="green" />
              Safe to change (see Suggested)
            </span>
            <span>
              <Dot color="orange" />
              Gaming dependency
            </span>
            <span>
              <Dot color="red" />
              Locked core service
            </span>
            <span>
              <Dot />
              Disabled
            </span>
            <label title="Also stop a running service right away when it is set to Disabled.">
              <Switch on={view.stopOnDisable} onChange={(on) => act({ kind: "stop_on_disable", on })} label="Stop when disabling" />
              Stop when disabling
            </label>
            <span className="spacer" />
            {view.status && (
              <span className={cx("svc-status", view.status.error && "tone-error")} title={view.status.text}>
                {view.status.text}
              </span>
            )}
          </div>
        }
      >
        {view.loading ? (
          <LoadingState title="Loading services..." />
        ) : view.rows.length === 0 ? (
          view.status?.error ? (
            <ErrorState title="Could not load services" text={view.status.text} />
          ) : (
            <EmptyState title="No services to show" />
          )
        ) : (
          <div className="svc-scroll">
            <ServiceTable view={view} act={act} />
          </div>
        )}
      </Group>

      {view.op ? (
        <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} />
      ) : view.backups ? (
        <BackupsSheet b={view.backups} admin={admin} act={act} />
      ) : (
        view.history && <HistorySheet h={view.history} admin={admin} act={act} />
      )}
    </>
  );
}

export function Services() {
  const { admin } = useShell();
  const { view, error, act } = useTab<View, Action>("services");
  return (
    <Page
      title="Services"
      subtitle={SUBTITLE}
      actions={
        view && (
          <>
            {view.refreshing && <Spinner />}
            <Button icon={<HistoryIcon />} title="Every change made here, with undo per entry." onClick={() => act({ kind: "history" })}>
              History
            </Button>
            <Button icon={<Save />} title="Save or restore the startup type of every service." onClick={() => act({ kind: "backups" })}>
              Backups
            </Button>
            <Button icon={<RefreshCw />} disabled={view.refreshing} title="Read the service list again." onClick={() => act({ kind: "refresh" })}>
              Refresh
            </Button>
          </>
        )
      }
    >
      <ViewOnlyBanner admin={admin} />
      {error ? <ErrorState text={error} /> : !view ? <LoadingState title="Loading services..." /> : <Body view={view} admin={admin} act={act} />}
    </Page>
  );
}
