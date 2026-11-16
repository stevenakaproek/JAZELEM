// Group Policy: click policies to select them, right-click to enable or disable, then apply the changes.
import { ArrowRight, RefreshCw, Search as SearchIcon } from "lucide-react";
import { useState } from "react";
import { OpSheet } from "../components/OpSheet";
import { Badge, Banner, Button, Check, ContextMenu, EmptyState, ErrorState, Group, Hero, LoadingState, Page, SearchField, Spinner, Stat, ViewOnlyBanner, cx, type BadgeColor, type MenuItem } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/gpolicy.css";

interface Chip {
  text: string;
  color: BadgeColor;
  tip?: string;
}

export interface PolicyOpt {
  label: string;
  value: number;
  /** false when the value is none of the choices (shown as "?"). */
  known: boolean;
  choices: { label: string; value: number }[];
}

export interface PolicyRow {
  index: number;
  name: string;
  advice: string;
  tip: string;
  picked: boolean;
  changed: boolean;
  current: string;
  currentColor: BadgeColor;
  target: string;
  note: Chip | null;
  rec: Chip;
  opts: PolicyOpt[];
  menuTitle: string;
}

export interface View {
  loaded: boolean;
  gpo: boolean;
  edition: string;
  loadError: string | null;
  atRec: number;
  recs: number;
  changes: number;
  canEdit: boolean;
  canApply: boolean;
  canSelect: boolean;
  canDiscard: boolean;
  canUndo: boolean;
  canRefresh: boolean;
  working: boolean;
  picked: number;
  lastAction: string;
  search: string;
  groups: { name: string; rows: PolicyRow[] }[];
  op: OpWindow | null;
  opCan: boolean;
}

export type PolicyChoice = "enabled" | "disabled" | "not_configured";

export type Action =
  | { kind: "toggle" | "keep" | "reset"; index: number }
  | { kind: "set"; index: number; state: PolicyChoice }
  | { kind: "opt"; index: number; opt: number; value: number }
  | { kind: "search"; text: string }
  | { kind: "select_recommended" | "apply" | "discard" | "undo" | "refresh" | "gpedit" | "clear_selection" }
  | { kind: "op"; ev: OpEvent };

type Act = (a: Action) => Promise<void>;

/** One policy. A click selects it; a right-click opens Enable / Disable / Not configured. Its drop-downs and Reset keep their own clicks. */
function PolicyLine({ r, canEdit, act, onMenu }: { r: PolicyRow; canEdit: boolean; act: Act; onMenu: (e: React.MouseEvent, r: PolicyRow) => void }) {
  const toggle = () => act({ kind: "toggle", index: r.index });
  return (
    <div
      className={cx("row", "gp-row", canEdit && "clickable", r.picked && "selected")}
      role="button"
      tabIndex={canEdit ? 0 : -1}
      aria-pressed={r.picked}
      onClick={canEdit ? toggle : undefined}
      onKeyDown={(e) => {
        if (canEdit && e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
          e.preventDefault();
          void toggle();
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        if (canEdit) onMenu(e, r);
      }}
    >
      <Check on={r.picked} />
      <span className="row-main">
        <div className="row-title" title={r.tip}>
          {r.name}
        </div>
        <div className="row-sub">{r.advice}</div>
        {r.opts.length > 0 && (
          <div className="gp-opts" onClick={(e) => e.stopPropagation()}>
            {r.opts.map((o, k) => (
              <label key={k} className="hstack">
                {o.label}
                <select className="input select" value={o.known ? o.value : ""} disabled={!canEdit} onChange={(e) => act({ kind: "opt", index: r.index, opt: k, value: Number(e.target.value) })}>
                  {!o.known && (
                    <option value="" disabled>
                      ?
                    </option>
                  )}
                  {o.choices.map((c) => (
                    <option key={c.value} value={c.value}>
                      {c.label}
                    </option>
                  ))}
                </select>
              </label>
            ))}
          </div>
        )}
      </span>
      <span className="row-accessory">
        <Badge color={r.rec.color}>{r.rec.text}</Badge>
        {r.changed ? (
          <>
            <span className="hstack secondary" style={{ gap: 4 }}>
              {r.current}
              <ArrowRight size={12} />
            </span>
            <Badge color="accent">{r.target}</Badge>
            <Button
              variant="plain"
              title="Keep it as it is now."
              onClick={(e) => {
                e.stopPropagation();
                void act({ kind: "reset", index: r.index });
              }}
            >
              Reset
            </Button>
          </>
        ) : (
          <>
            {r.note && (
              <Badge color={r.note.color} title={r.note.tip}>
                {r.note.text}
              </Badge>
            )}
            <Badge color={r.currentColor}>{r.current}</Badge>
          </>
        )}
      </span>
    </div>
  );
}

function Body({ view, admin, act }: { view: View; admin: boolean; act: Act }) {
  const [search, setSearch] = useState(view.search);
  const [menu, setMenu] = useState<{ x: number; y: number; row: PolicyRow } | null>(null);
  // the backend keeps the search text (it filters the list); the field itself stays local so typing never lags
  const type = (text: string) => {
    setSearch(text);
    void act({ kind: "search", text });
  };
  const menuItems = (r: PolicyRow): (MenuItem | "separator")[] => {
    const set = (state: PolicyChoice) => () => void act({ kind: "set", index: r.index, state });
    return [
      { label: r.menuTitle, disabled: true, onClick: () => {} },
      "separator",
      { label: "Enable", onClick: set("enabled") },
      { label: "Disable", onClick: set("disabled") },
      { label: "Not configured", onClick: set("not_configured") },
      "separator",
      { label: "Keep as it is (no change)", onClick: () => void act({ kind: "keep", index: r.index }) },
    ];
  };
  return (
    <>
      <ViewOnlyBanner admin={admin} />
      {!view.gpo && (
        <Banner>
          Windows {view.edition} has no Group Policy editor: policies are written straight to the registry. A few protected ones (Widgets) may be refused - remove Widgets in Remove Bloat instead.
        </Banner>
      )}
      {view.loadError && <Banner kind="warn">{view.loadError}</Banner>}
      <Group padded>
        <div className="hstack hero-row">
          {view.loaded ? (
            <Hero value={`${view.atRec} of ${view.recs}`} unit="recommended" caption="policies already set the recommended way" tone={view.atRec === view.recs ? "Ok" : "Accent"} />
          ) : (
            <Hero value="..." unit="recommended" caption="Reading the policies" tone="Faint" />
          )}
          <Stat value={view.changes} label="changes selected" tone={view.changes > 0 ? "Accent" : "Text"} />
          <div className="gp-hero-actions">
            <Button disabled={!view.canSelect} title="Selects the recommended setting for every policy that has one. Review, change or reset rows, then Apply." onClick={() => act({ kind: "select_recommended" })}>
              Select recommended
            </Button>
            <Button variant="primary" disabled={!view.canApply} title="Shows every change first." onClick={() => act({ kind: "apply" })}>
              Apply changes ({view.changes})
            </Button>
          </div>
        </div>
      </Group>
      <div className="toolbar">
        <SearchField value={search} onChange={type} placeholder="Search policies" />
        <Button disabled={!view.canDiscard} onClick={() => act({ kind: "discard" })}>
          Discard changes
        </Button>
        <Button disabled={!view.canUndo} title="Shows what goes back before anything changes." onClick={() => act({ kind: "undo" })}>
          Undo last change
        </Button>
        <Button icon={view.working ? <Spinner /> : <RefreshCw />} disabled={!view.canRefresh} onClick={() => act({ kind: "refresh" })}>
          Refresh
        </Button>
        {view.gpo && (
          <Button title="Windows' own Group Policy editor (everything set here shows there)." onClick={() => act({ kind: "gpedit" })}>
            Open gpedit
          </Button>
        )}
        {view.picked > 0 && <Button onClick={() => act({ kind: "clear_selection" })}>Clear selection ({view.picked})</Button>}
        {view.picked === 0 && view.lastAction === "" && <span className="tertiary">Click policies to select them, right-click to enable or disable.</span>}
        {view.lastAction !== "" && <span className="secondary">{view.lastAction}</span>}
      </div>
      {!view.loaded ? (
        <LoadingState title="Reading the policies..." />
      ) : view.groups.length === 0 ? (
        <EmptyState icon={<SearchIcon />} title="No policies found" />
      ) : (
        view.groups.map((g) => (
          <Group key={g.name} title={g.name}>
            {g.rows.map((r) => (
              <PolicyLine key={r.index} r={r} canEdit={view.canEdit} act={act} onMenu={(e, row) => setMenu({ x: e.clientX, y: e.clientY, row })} />
            ))}
          </Group>
        ))
      )}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.row)} onClose={() => setMenu(null)} />}
      {view.op && <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} />}
    </>
  );
}

export function Gpolicy() {
  const { admin } = useShell();
  const { view, error, act } = useTab<View, Action>("gpolicy");
  return (
    <Page title="Group Policy" subtitle="Gaming and privacy policies in plain English - no gpedit needed. Every change can be undone.">
      {error ? <ErrorState text={error} /> : !view ? <LoadingState title="Reading the policies..." /> : <Body view={view} admin={admin} act={act} />}
    </Page>
  );
}
