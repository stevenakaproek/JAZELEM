// Group Policy: every policy has its own Not configured / Enabled / Disabled control; Apply writes the changes.
import { RefreshCw, Search as SearchIcon } from "lucide-react";
import { useState } from "react";
import { OpSheet } from "../components/OpSheet";
import { Badge, Banner, Button, EmptyState, ErrorState, Group, Hero, LoadingState, Page, SearchField, Segmented, Spinner, Stat, ViewOnlyBanner, cx, type BadgeColor } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/gpolicy.css";

interface Chip {
  text: string;
  color: BadgeColor;
  tip?: string;
  /** The recommended label can be clicked to choose that setting. */
  can?: boolean;
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
  changed: boolean;
  /** What the policy will be after Apply ("other": set by something else, no segment lit). */
  state: PolicyChoice | "other";
  current: string;
  currentColor: BadgeColor;
  target: string;
  note: Chip | null;
  rec: Chip;
  opts: PolicyOpt[];
}

export interface View {
  loaded: boolean;
  gpo: boolean;
  edition: string;
  loadError: string | null;
  /** Policies that are Enabled or Disabled now, and how many there are. */
  configured: number;
  total: number;
  changes: number;
  canEdit: boolean;
  canApply: boolean;
  canDiscard: boolean;
  canUndo: boolean;
  canRefresh: boolean;
  working: boolean;
  lastAction: string;
  search: string;
  groups: { name: string; rows: PolicyRow[] }[];
  op: OpWindow | null;
  opCan: boolean;
}

export type PolicyChoice = "enabled" | "disabled" | "not_configured";

export type Action =
  | { kind: "reset" | "recommend"; index: number }
  | { kind: "set"; index: number; state: PolicyChoice }
  | { kind: "opt"; index: number; opt: number; value: number }
  | { kind: "search"; text: string }
  | { kind: "apply" | "discard" | "undo" | "refresh" | "gpedit" }
  | { kind: "op"; ev: OpEvent };

type Act = (a: Action) => Promise<void>;

const CHOICES: { value: PolicyChoice; label: string }[] = [
  { value: "not_configured", label: "Not configured" },
  { value: "enabled", label: "Enabled" },
  { value: "disabled", label: "Disabled" },
];

/** One policy with its own Not configured / Enabled / Disabled control. Nothing is written until Apply. */
function PolicyLine({ r, canEdit, act }: { r: PolicyRow; canEdit: boolean; act: Act }) {
  // "recommended: Enabled" is a button: a click chooses that setting for this policy
  const rec =
    r.rec.can && canEdit ? (
      <button type="button" className="badge gp-rec" title="Click to choose the recommended setting for this policy." onClick={() => void act({ kind: "recommend", index: r.index })}>
        {r.rec.text}
      </button>
    ) : (
      <Badge color={r.rec.color}>{r.rec.text}</Badge>
    );
  return (
    <div className={cx("row", "gp-row", r.changed && "changed")}>
      <span className="row-main">
        <div className="row-title" title={r.tip}>
          {r.name}
        </div>
        <div className="row-sub">{r.advice}</div>
        {r.opts.length > 0 && (
          <div className="gp-opts">
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
      <span className="gp-side" data-state={r.state}>
        <Segmented value={r.state as PolicyChoice} options={CHOICES} disabled={!canEdit} onChange={(state) => void act({ kind: "set", index: r.index, state })} />
        <span className="gp-status">
          {r.changed ? (
            <>
              <span className="secondary">now: {r.current}</span>
              <Badge color="accent">changes on Apply</Badge>
              <Button variant="plain" title="Keep it as it is now." onClick={() => void act({ kind: "reset", index: r.index })}>
                Reset
              </Button>
              {rec}
            </>
          ) : (
            <>
              {r.note && (
                <Badge color={r.note.color} title={r.note.tip}>
                  {r.note.text}
                </Badge>
              )}
              {r.state === "other" && <Badge color={r.currentColor}>{r.current}</Badge>}
              {rec}
            </>
          )}
        </span>
      </span>
    </div>
  );
}

function Body({ view, admin, act }: { view: View; admin: boolean; act: Act }) {
  const [search, setSearch] = useState(view.search);
  // the backend keeps the search text (it filters the list); the field itself stays local so typing never lags
  const type = (text: string) => {
    setSearch(text);
    void act({ kind: "search", text });
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
            <Hero value={`${view.configured} of ${view.total}`} unit="set" caption="policies are Enabled or Disabled now" tone="Accent" />
          ) : (
            <Hero value="..." unit="set" caption="Reading the policies" tone="Faint" />
          )}
          <Stat value={view.changes} label="changes to apply" tone={view.changes > 0 ? "Accent" : "Text"} />
          <div className="gp-hero-actions">
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
        {view.lastAction === "" && <span className="tertiary">Choose Enabled, Disabled or Not configured on any policy, then Apply changes.</span>}
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
              <PolicyLine key={r.index} r={r} canEdit={view.canEdit} act={act} />
            ))}
          </Group>
        ))
      )}
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
