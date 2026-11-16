// Startup Cleaner: everything that starts with Windows. Click rows to select them, switch them off or on,
// and take changes back from the history.
import { History as HistoryIcon, RefreshCw, SearchX } from "lucide-react";
import { Fragment, useState } from "react";
import { ABOUT_HINT, AboutRow, useOpenRows, type About } from "../components/About";
import { OpSheet } from "../components/OpSheet";
import { Badge, Banner, Button, Check, EmptyState, ErrorState, Group, Hero, LoadingState, Page, SearchField, Sheet, Spinner, Stat, Switch, ViewOnlyBanner, cx } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow, Tone } from "../lib/types";
import { toneClass } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/startup.css";

/** pctuner_core::startup::Kind */
export type StartupKind = "Run" | "StartupFolder" | "Task" | "StoreApp" | "ShellExt" | "Tweak";

export interface StartupRow {
  id: string;
  on: boolean;
  name: string;
  type: string;
  location: string;
  publisher: string;
  advice: string;
  command: string;
  /** Colour of the name and advice (category, or dim when switched off). */
  tone: Tone;
  selected: boolean;
  /** shown when the row is right-clicked */
  about: About;
}

export interface StartupHistRow {
  index: number;
  time: string;
  name: string;
  type: string;
  location: string;
  change: string;
  state: string;
  undone: boolean;
  selected: boolean;
}

export interface StartupHistory {
  open: boolean;
  recorded: number;
  active: number;
  rows: StartupHistRow[];
  selected: number;
  undoSelectedLabel: string;
  canUndoSelected: boolean;
  canClear: boolean;
  canUndoAll: boolean;
  busy: boolean;
  status: string;
}

export interface StartupView {
  /** false while the first scan runs. */
  ready: boolean;
  busy: boolean;
  /** "Working / scanning...", "Restarting Explorer..." or the result of the last action. */
  status: string;
  hero: { junk: string; tone: Tone; caption: string; on: string; optional: string; optionalTone: Tone; pending: string };
  warnings: string[];
  search: string;
  kindFilter: StartupKind | null;
  kinds: { value: StartupKind; label: string }[];
  enabledOnly: boolean;
  showWindows: boolean;
  rows: StartupRow[];
  selected: number;
  cleanCount: number;
  canClean: boolean;
  canSwitchOn: boolean;
  canUndoLast: boolean;
  canRescan: boolean;
  canExplorer: boolean;
  history: StartupHistory;
  op: OpWindow | null;
  opCan: boolean;
}

export type StartupAction =
  | { kind: "toggle"; id: string }
  | { kind: "select_all" | "clear_selection" | "clean" | "switch_on" | "undo_last" | "rescan" | "explorer" }
  | { kind: "search"; text: string }
  | { kind: "kind_filter"; value: StartupKind | null }
  | { kind: "enabled_only" | "show_windows"; on: boolean }
  | { kind: "history_open" | "history_close" | "hist_undo_selected" | "hist_undo_all" | "hist_clear" }
  | { kind: "hist_toggle"; index: number }
  | { kind: "op"; ev: OpEvent };

type Act = (a: StartupAction) => Promise<void>;

function HistorySheet({ h, act }: { h: StartupHistory; act: Act }) {
  return (
    <Sheet
      wide
      kicker="Startup Cleaner"
      title={
        <span className="hstack">
          Change history
          <span className="spacer" />
          <Badge>{h.recorded} recorded</Badge>
          <Badge color={h.active > 0 ? "accent" : ""}>{h.active} can be undone</Badge>
        </span>
      }
      subtitle="Click the changes you want to take back, newest first."
      onClose={() => act({ kind: "history_close" })}
      footer={
        <>
          <Button variant="primary" size="lg" disabled={!h.canUndoSelected} onClick={() => act({ kind: "hist_undo_selected" })}>
            {h.undoSelectedLabel}
          </Button>
          <Button size="lg" disabled={!h.canClear} title="Removes already-undone rows from this list. Changes nothing on the PC." onClick={() => act({ kind: "hist_clear" })}>
            Clear undone entries
          </Button>
          <Button size="lg" onClick={() => act({ kind: "history_close" })}>
            Close
          </Button>
          {h.busy ? <Spinner /> : h.status && <span className="secondary">{h.status}</span>}
          <span className="spacer" />
          <Button
            variant="danger"
            size="lg"
            disabled={!h.canUndoAll}
            title="Takes back every change still active in this list. You see the full list before anything happens."
            onClick={() => act({ kind: "hist_undo_all" })}
          >
            Undo everything
          </Button>
        </>
      }
    >
      <div className="group startup-history">
        {h.rows.length === 0 ? (
          <div className="row">
            <span className="secondary">No changes recorded yet.</span>
          </div>
        ) : (
          <table className="table">
            <colgroup>
              <col style={{ width: "19%" }} />
              <col style={{ width: "24%" }} />
              <col style={{ width: "16%" }} />
              <col style={{ width: "18%" }} />
              <col style={{ width: "13%" }} />
              <col style={{ width: "10%" }} />
            </colgroup>
            <thead>
              <tr>
                {["Time", "Name", "Type", "Location", "Change", "State"].map((t) => (
                  <th key={t}>{t}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {h.rows.map((r) => {
                const quiet = r.undone ? "tertiary" : "secondary";
                return (
                  <tr key={r.index} className={cx(!r.undone && "clickable", r.selected && "selected")} aria-selected={r.selected} onClick={() => act({ kind: "hist_toggle", index: r.index })}>
                    <td className={quiet}>{r.time}</td>
                    <td className={r.undone ? "tertiary" : undefined} title={r.name}>
                      {r.name}
                    </td>
                    <td className={quiet}>{r.type}</td>
                    <td className={quiet} title={r.location}>
                      {r.location}
                    </td>
                    <td className={quiet}>{r.change}</td>
                    <td className={r.undone ? "tertiary" : "tone-accent"}>{r.state}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </Sheet>
  );
}

function ItemTable({ rows, act }: { rows: StartupRow[]; act: Act }) {
  const { open, toggle } = useOpenRows();
  return (
    <table className="table">
      <caption className="table-hint">{ABOUT_HINT}</caption>
      <colgroup>
        <col style={{ width: 36 }} />
        <col style={{ width: 44 }} />
        <col style={{ width: "20%" }} />
        <col style={{ width: "13%" }} />
        <col style={{ width: "12%" }} />
        <col style={{ width: "11%" }} />
        <col style={{ width: "24%" }} />
        <col />
      </colgroup>
      <thead>
        <tr>
          <th className="check-cell" />
          {["On", "Name", "Type", "Location", "Publisher", "Advice", "Command"].map((t) => (
            <th key={t}>{t}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => {
          const main = toneClass(r.tone);
          const quiet = r.on ? "secondary" : "tertiary";
          const expanded = open.has(r.id);
          return (
            <Fragment key={r.id}>
            <tr
              className={cx("clickable", r.selected && "selected", expanded && "expanded")}
              aria-selected={r.selected}
              aria-expanded={expanded}
              title={ABOUT_HINT}
              onClick={() => act({ kind: "toggle", id: r.id })}
              onContextMenu={(e) => {
                e.preventDefault();
                toggle(r.id);
              }}
            >
              <td className="check-cell">
                <Check on={r.selected} />
              </td>
              <td className={main}>{r.on ? "On" : "Off"}</td>
              <td className={main} title={r.name}>
                {r.name}
              </td>
              <td className={quiet}>{r.type}</td>
              <td className={quiet} title={r.location}>
                {r.location}
              </td>
              <td className={quiet} title={r.publisher}>
                {r.publisher}
              </td>
              <td className={main} title={r.advice}>
                {r.advice}
              </td>
              <td className={quiet} title={r.command}>
                {r.command}
              </td>
            </tr>
            {expanded && <AboutRow about={r.about} cols={8} />}
            </Fragment>
          );
        })}
      </tbody>
    </table>
  );
}

function Body({ view, act }: { view: StartupView; act: Act }) {
  const { admin } = useShell();
  // the text field answers at once; the list itself is filtered by the app
  const [search, setSearch] = useState(view.search);
  const onSearch = (text: string) => {
    setSearch(text);
    void act({ kind: "search", text });
  };
  const h = view.hero;
  return (
    <>
      <ViewOnlyBanner admin={admin} />
      <Group padded>
        <div className="hstack hero-row">
          <Hero value={h.junk} unit="junk still on" caption={h.caption} tone={h.tone} />
          <Stat value={h.on} label="items switched on" />
          <Stat value={h.optional} label="optional still on" tone={h.optionalTone} />
          <Stat value={h.pending} label="changes can be undone" />
        </div>
      </Group>
      {view.warnings.map((w, i) => (
        <Banner key={i} kind="warn">
          {w}
        </Banner>
      ))}
      <Group>
        <div className="startup-tools">
          <div className="toolbar">
            <SearchField value={search} onChange={onSearch} placeholder="Search name, publisher, command" />
            <select
              className="input select"
              aria-label="Type"
              value={view.kindFilter ?? ""}
              onChange={(e) => act({ kind: "kind_filter", value: e.target.value === "" ? null : (e.target.value as StartupKind) })}
            >
              <option value="">All types</option>
              {view.kinds.map((k) => (
                <option key={k.value} value={k.value}>
                  {k.label}
                </option>
              ))}
            </select>
            <label className="startup-option">
              <Switch on={view.enabledOnly} label="Enabled only" onChange={(on) => act({ kind: "enabled_only", on })} />
              Enabled only
            </label>
            <label className="startup-option">
              <Switch on={view.showWindows} label="Show all Windows tasks" onChange={(on) => act({ kind: "show_windows", on })} />
              Show all Windows tasks
            </label>
            {view.busy ? (
              <span className="hstack secondary">
                <Spinner />
                {view.status}
              </span>
            ) : (
              view.status && <span className="secondary selectable">{view.status}</span>
            )}
          </div>
          <div className="toolbar">
            <Button
              variant="primary"
              disabled={!view.canClean}
              title="Click rows in the list to select them, then switch them off here (nothing is deleted). Shows a preview first; one click undoes it."
              onClick={() => act({ kind: "clean" })}
            >
              Clean selected ({view.cleanCount})
            </Button>
            <span className="startup-divider" />
            <Button disabled={!view.canSwitchOn} title="Switch the selected items back on. Shows a preview first." onClick={() => act({ kind: "switch_on" })}>
              Switch on
            </Button>
            <Button disabled={!view.canUndoLast} title="Shows what goes back before anything changes." onClick={() => act({ kind: "undo_last" })}>
              Undo last change
            </Button>
            <span className="secondary">{view.selected} selected</span>
            <Button variant="plain" onClick={() => act({ kind: "select_all" })}>
              Select all shown
            </Button>
            <Button variant="plain" onClick={() => act({ kind: "clear_selection" })}>
              Clear selection
            </Button>
            <span className="spacer" />
            <Badge color="orange">Junk</Badge>
            <Badge color="yellow">Optional</Badge>
            <Badge color="green">Keep</Badge>
            <Badge>Switched off</Badge>
          </div>
        </div>
        {!view.ready ? (
          <LoadingState title="Scanning startup locations..." />
        ) : view.rows.length === 0 ? (
          <EmptyState icon={<SearchX />} title="Nothing to show" />
        ) : (
          <div className="startup-list">
            <ItemTable rows={view.rows} act={act} />
          </div>
        )}
      </Group>
      {view.op ? <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} /> : view.history.open && <HistorySheet h={view.history} act={act} />}
    </>
  );
}

export function Startup() {
  const { view, error, act } = useTab<StartupView, StartupAction>("startup");
  return (
    <Page
      title="Startup Cleaner"
      subtitle="Everything that starts with Windows. Items are switched off, never deleted - every change can be undone."
      actions={
        <>
          <Button icon={view?.busy ? <Spinner /> : <RefreshCw />} disabled={!view?.canRescan} onClick={() => act({ kind: "rescan" })}>
            Rescan
          </Button>
          <Button icon={<HistoryIcon />} disabled={!view} title="Every change this tool made, with undo." onClick={() => act({ kind: "history_open" })}>
            History...
          </Button>
          <Button disabled={!view?.canExplorer} title="Needed for Explorer add-on changes to take effect. Shows a preview first." onClick={() => act({ kind: "explorer" })}>
            Restart Explorer
          </Button>
        </>
      }
    >
      {error ? <ErrorState text={error} /> : !view ? <LoadingState title="Scanning startup locations..." /> : <Body view={view} act={act} />}
    </Page>
  );
}
