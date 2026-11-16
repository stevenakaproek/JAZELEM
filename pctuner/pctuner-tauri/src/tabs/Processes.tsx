// Processes: background processes that can be switched off or given a lower priority without breaking
// Windows or getting in the way of games, each with undo, and a 30-second before / after measurement.
import { ArrowDown, Gauge, PowerOff, RefreshCw, RotateCcw } from "lucide-react";
import { useState } from "react";
import { Badge, Banner, Button, ErrorState, Group, Hero, LoadingState, Page, Progress, Sheet, Spinner, Stat, ToneBadge, ViewOnlyBanner, cx } from "../components/ui";
import type { Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/procs.css";

export interface ProcRow {
  exe: string;
  file: string;
  title: string;
  what: string;
  loses: string;
  kind: "off" | "lower" | "keep";
  running: number;
  mb: number | null;
  state: string;
  tone: Tone;
  changed: boolean;
  when: string | null;
  /** the registry values written and notes, one per line */
  detail: string | null;
  canOff: boolean;
  canLower: boolean;
  canUndo: boolean;
}

export interface Side {
  time: string;
  setup: string;
  listedMb: number;
  listedCpu: string;
  pcCpu: string | null;
  ramMb: number | null;
}

type Cell = { mb: number; cpu: string | null; gone?: boolean } | null;

export interface Compare {
  before: Side | null;
  after: Side;
  rows: { title: string; file: string; before: Cell; after: Cell }[];
  savedMb: number | null;
  count: number;
}

export interface View {
  loading: boolean;
  refreshing: boolean;
  admin: boolean;
  /** "Touch keyboard switched off..." while a change runs */
  busy: string | null;
  note: { ok: boolean; text: string } | null;
  rows: ProcRow[];
  hero: { running: number; runningMb: number; changed: number };
  canUndoAll: boolean;
  measuring: { secs: number; of: number } | null;
  compare: Compare | null;
}

export type Action = { kind: "off" | "lower" | "undo"; exe: string } | { kind: "undo_all" | "measure" | "refresh" | "dismiss_note" };

type Act = (a: Action) => Promise<void>;

function RowView({ r, view, act, onOff }: { r: ProcRow; view: View; act: Act; onOff: (r: ProcRow) => void }) {
  const notAdmin = "Run PC Tuner as administrator.";
  return (
    <div className={cx("row proc-row", r.changed && "proc-changed")} title={r.detail ?? r.file}>
      <span className="row-main">
        <div className="row-title hstack proc-title">
          {r.title}
          <span className="tertiary proc-file">{r.file}</span>
        </div>
        <div className="row-sub">{r.what}</div>
        <div className="row-sub proc-loses">
          {r.kind === "keep" ? "Why: " : "You lose: "}
          {r.loses}
        </div>
        {r.when && <div className="row-sub tertiary">changed {r.when}</div>}
      </span>
      <span className="proc-side">
        <span className="proc-use">{r.running > 0 ? `${r.mb ?? "?"} MB${r.running > 1 ? ` · ${r.running} copies` : ""}` : "not running"}</span>
        <ToneBadge tone={r.tone}>{r.state}</ToneBadge>
      </span>
      {r.kind !== "keep" && (
        <span className="proc-buttons">
          {r.changed ? (
            <Button icon={<RotateCcw />} disabled={!r.canUndo} title={view.admin ? "Put back what was there before." : notAdmin} onClick={() => act({ kind: "undo", exe: r.exe })}>
              Undo
            </Button>
          ) : (
            <>
              {r.kind === "off" && (
                <Button variant="primary" icon={<PowerOff />} disabled={!r.canOff} title={view.admin ? "Switch the feature off, close it now, and keep it at low priority if Windows starts it anyway." : notAdmin} onClick={() => onOff(r)}>
                  Switch off
                </Button>
              )}
              <Button icon={<ArrowDown />} disabled={!r.canLower} title={view.admin ? "Keep it, at below normal CPU and low disk priority (also applied to the running copy now)." : notAdmin} onClick={() => act({ kind: "lower", exe: r.exe })}>
                Lower priority
              </Button>
            </>
          )}
        </span>
      )}
    </div>
  );
}

function cell(c: Cell, unit: "mb" | "cpu") {
  if (!c) return "-";
  if (unit === "mb") return c.gone ? "gone" : `${c.mb} MB`;
  return c.cpu === null ? "?" : `${c.cpu} %`;
}

function MeasureCard({ view, act }: { view: View; act: Act }) {
  const m = view.measuring;
  const c = view.compare;
  return (
    <Group
      title="Measure the effect"
      padded
      footer="Measure once before a change and once after it (ideally after playing a while or restarting). CPU is the share of the whole processor each process used during the 30 seconds."
    >
      <div className="stack">
        <div className="toolbar">
          <Button variant={c ? "default" : "primary"} icon={<Gauge />} disabled={m !== null} onClick={() => act({ kind: "measure" })}>
            Measure 30 s
          </Button>
          {m ? (
            <>
              <Spinner />
              <span className="secondary">
                Measuring... {m.secs} / {m.of} s - keep using the PC as you normally do
              </span>
            </>
          ) : (
            c && <span className="secondary">{c.count} measurement(s) saved</span>
          )}
        </div>
        {m && <Progress value={m.secs / m.of} />}
        {c && (
          <>
            {c.before && c.savedMb !== null && (
              <div className={cx("proc-saved", c.savedMb > 0 ? "tone-ok" : "secondary")}>
                {c.savedMb > 0 ? `${c.savedMb} MB less memory used by these processes` : c.savedMb < 0 ? `${-c.savedMb} MB more memory than before` : "No memory difference"}
              </div>
            )}
            <table className="proc-table">
              <thead>
                <tr>
                  <th />
                  {c.before && <th>Before · {c.before.time.slice(11, 16)}</th>}
                  <th>{c.before ? "After" : "Now"} · {c.after.time.slice(11, 16)}</th>
                </tr>
              </thead>
              <tbody>
                <tr className="proc-total">
                  <td>Changes in effect</td>
                  {c.before && <td>{c.before.setup}</td>}
                  <td>{c.after.setup}</td>
                </tr>
                <tr className="proc-total">
                  <td>These processes: memory</td>
                  {c.before && <td>{c.before.listedMb} MB</td>}
                  <td>{c.after.listedMb} MB</td>
                </tr>
                <tr className="proc-total">
                  <td>These processes: CPU</td>
                  {c.before && <td>{c.before.listedCpu} %</td>}
                  <td>{c.after.listedCpu} %</td>
                </tr>
                <tr className="proc-total">
                  <td>Whole PC: CPU / memory in use</td>
                  {c.before && (
                    <td>
                      {c.before.pcCpu ?? "?"} % · {c.before.ramMb ?? "?"} MB
                    </td>
                  )}
                  <td>
                    {c.after.pcCpu ?? "?"} % · {c.after.ramMb ?? "?"} MB
                  </td>
                </tr>
                {c.rows.map((r) => (
                  <tr key={r.file} title={r.file}>
                    <td>{r.title}</td>
                    {c.before && (
                      <td>
                        {cell(r.before, "mb")} · {cell(r.before, "cpu")}
                      </td>
                    )}
                    <td>
                      {cell(r.after, "mb")} · {cell(r.after, "cpu")}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </div>
    </Group>
  );
}

export function Processes() {
  const { view, error, act } = useTab<View, Action>("procs");
  const [confirm, setConfirm] = useState<ProcRow | null>(null);
  return (
    <Page title="Processes" subtitle="Background processes that can be switched off or given a lower priority without breaking Windows or slowing games. Every change can be undone.">
      {error ? (
        <ErrorState text={error} />
      ) : !view || view.loading ? (
        <LoadingState title="Reading the running processes..." />
      ) : (
        <>
          <ViewOnlyBanner admin={view.admin} />
          <Group padded>
            <div className="hstack hero-row" style={{ flexWrap: "wrap" }}>
              <Hero value={`${view.hero.runningMb} MB`} unit="used by processes you can change" caption={`${view.hero.running} of them running now`} tone="Accent" />
              <Stat value={view.hero.changed} label="changed by PC Tuner" tone={view.hero.changed > 0 ? "Ok" : "Text"} />
            </div>
          </Group>
          <div className="toolbar">
            <Button icon={<RefreshCw />} disabled={view.refreshing} onClick={() => act({ kind: "refresh" })}>
              Refresh
            </Button>
            <Button icon={<RotateCcw />} disabled={!view.canUndoAll} onClick={() => act({ kind: "undo_all" })}>
              Undo all
            </Button>
            {view.refreshing && <Spinner />}
            {view.busy && (
              <>
                <Spinner />
                <span className="secondary">{view.busy}</span>
              </>
            )}
          </div>
          {view.note && (
            <Banner kind={view.note.ok ? "ok" : "error"} action={<Button variant="plain" onClick={() => act({ kind: "dismiss_note" })}>OK</Button>}>
              {view.note.text}
            </Banner>
          )}
          <MeasureCard view={view} act={act} />
          <Group title="Can be switched off" footer="Switch off = the feature's own setting is turned off (as in the Settings app), the running copy is closed, and it is kept at low priority in case Windows starts it anyway.">
            {view.rows.filter((r) => r.kind === "off").map((r) => (
              <RowView key={r.exe} r={r} view={view} act={act} onOff={setConfirm} />
            ))}
          </Group>
          <Group title="Can run at lower priority" footer="Lower priority = below normal CPU and low disk priority every time it starts. It still runs, it just never competes with your game. If Process Lasso has a rule for the same process, its rule wins.">
            {view.rows.filter((r) => r.kind === "lower").map((r) => (
              <RowView key={r.exe} r={r} view={view} act={act} onOff={setConfirm} />
            ))}
          </Group>
          <Group title="Left alone" count={<Badge>never changed by PC Tuner</Badge>}>
            {view.rows.filter((r) => r.kind === "keep").map((r) => (
              <RowView key={r.exe} r={r} view={view} act={act} onOff={setConfirm} />
            ))}
          </Group>
          {confirm && (
            <Sheet
              kicker="Processes"
              title={`Switch off ${confirm.title}?`}
              subtitle={`${confirm.what} You lose: ${confirm.loses}`}
              onClose={() => setConfirm(null)}
              footer={
                <>
                  <Button
                    variant="primary"
                    size="lg"
                    onClick={() => {
                      void act({ kind: "off", exe: confirm.exe });
                      setConfirm(null);
                    }}
                  >
                    Switch off
                  </Button>
                  <Button size="lg" onClick={() => setConfirm(null)}>
                    Cancel
                  </Button>
                </>
              }
            />
          )}
        </>
      )}
    </Page>
  );
}
