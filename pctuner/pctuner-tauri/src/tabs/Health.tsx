// Health: the drives' health, then Windows' own repair tools (CHKDSK, DISM, SFC) with live progress,
// an activity line that shows they are still working, and a "looks stuck" warning only when they are not.
import { Activity, FileText, HardDrive, HeartPulse, Play, RefreshCw, Square, Wrench } from "lucide-react";
import { useState } from "react";
import { Badge, Banner, Button, ErrorState, Group, Hero, LoadingState, Page, Progress, Sheet, Spinner, ToneBadge, ViewOnlyBanner, cx } from "../components/ui";
import type { Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/health.css";

type Level = "ok" | "warn" | "error";

export interface Drive {
  name: string;
  kind: string;
  sizeGb: number;
  health: string;
  wear: number | null;
  temp: number | null;
  errors: number | null;
  hours: number | null;
  warnings: string[];
  level: Level;
}

export interface DiskView {
  loading: boolean;
  refreshing?: boolean;
  time?: string;
  error?: string | null;
  drives?: Drive[];
  cFree?: number | null;
  cSize?: number | null;
  lowSpace?: string | null;
}

export interface StepView {
  id: "chkdsk" | "dism" | "sfc";
  title: string;
  running: {
    phase: string;
    pct: number | null;
    stage: string;
    elapsed: number;
    sinceMove: number;
    activity: string | null;
    busy: boolean;
    note: string | null;
    stalled: boolean;
    stopping: boolean;
  } | null;
  queued: boolean;
  canRun: boolean;
  last: { time: string; level: Level; summary: string; detail: string[]; secs: number; hasLog: boolean; canFix: boolean } | null;
}

export interface View {
  admin: boolean;
  overall: string;
  tone: Tone;
  lastAll: string | null;
  running: boolean;
  canRunAll: boolean;
  canStop: boolean;
  note: { ok: boolean; text: string } | null;
  prep: string | null;
  disk: DiskView;
  steps: StepView[];
}

export type Action = { kind: "run_all" | "stop" | "fix_chkdsk" | "refresh_disk" | "dismiss_note" } | { kind: "run" | "open_log"; step: string };

type Act = (a: Action) => Promise<void>;

const ABOUT: Record<StepView["id"], string> = {
  chkdsk: "Scans the file system on C: while Windows runs (no restart). Usually 1-5 minutes.",
  dism: "Checks Windows' store of system components. The slow repair from Windows Update only runs when damage is found.",
  sfc: "Checks every protected Windows file and replaces damaged ones from the component store. Runs after DISM. Usually 5-15 minutes.",
};

const levelTone = (l: Level): Tone => (l === "ok" ? "Ok" : l === "warn" ? "Warn" : "Error");

function dur(s: number): string {
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ${s % 60} s`;
  return `${Math.floor(m / 60)} h ${m % 60} min`;
}

function DiskCard({ d, act }: { d: DiskView; act: Act }) {
  return (
    <Group
      title={
        <span className="hstack">
          <HardDrive size={14} /> Drive health
        </span>
      }
      count={
        <Button variant="plain" icon={d.refreshing ? <Spinner /> : <RefreshCw />} disabled={d.loading || d.refreshing} onClick={() => act({ kind: "refresh_disk" })}>
          Check again
        </Button>
      }
      footer="Read from the drives' own health counters by Windows (wear, temperature, errors). Read only - nothing is changed."
    >
      {d.loading ? (
        <div className="row">
          <Spinner /> <span className="secondary">Reading the drives...</span>
        </div>
      ) : d.error ? (
        <div className="row tone-error">{d.error}</div>
      ) : (
        <>
          {d.lowSpace && (
            <div className="row">
              <span className="tone-warn">{d.lowSpace}</span>
            </div>
          )}
          {(d.drives ?? []).map((x) => (
            <div key={x.name} className="row health-drive">
              <span className="row-main">
                <div className="row-title">{x.name}</div>
                <div className="row-sub">
                  {[x.kind, `${x.sizeGb} GB`, x.wear !== null ? `${x.wear} % worn` : null, x.temp !== null ? `${x.temp} °C` : null, x.errors !== null ? `${x.errors} errors` : null, x.hours !== null ? `${x.hours.toLocaleString()} hours on` : null]
                    .filter(Boolean)
                    .join(" · ")}
                </div>
                {x.warnings.map((w) => (
                  <div key={w} className={cx("row-sub", x.level === "error" ? "tone-error" : "tone-warn")}>
                    {w}
                  </div>
                ))}
              </span>
              <ToneBadge tone={levelTone(x.level)}>{x.health || "Unknown"}</ToneBadge>
            </div>
          ))}
          {d.cFree != null && d.cSize != null && (
            <div className="row">
              <span className="row-main secondary">
                C: {d.cFree} GB free of {d.cSize} GB
              </span>
            </div>
          )}
        </>
      )}
    </Group>
  );
}

function StepCard({ s, act, onFix }: { s: StepView; act: Act; onFix: () => void }) {
  const r = s.running;
  return (
    <Group title={s.title} footer={ABOUT[s.id]}>
      <div className="health-step stack">
        {r ? (
          <>
            <div className="hstack health-head">
              <Spinner />
              <span className="health-phase">{r.stopping ? "Stopping..." : r.phase || "Running"}</span>
              <span className="spacer" />
              <span className="health-pct">{r.pct !== null ? `${r.pct.toFixed(r.pct % 1 ? 1 : 0)} %` : "starting..."}</span>
            </div>
            <Progress value={r.pct !== null ? r.pct / 100 : null} />
            {r.stage && <div className="secondary">{r.stage}</div>}
            <div className={cx("health-activity", r.busy ? "tone-ok" : "tertiary")}>
              <Activity size={13} />
              {r.busy ? "Working" : "Waiting"} · {dur(r.elapsed)}
              {r.activity ? ` · ${r.activity}` : ""}
              {r.sinceMove >= 30 ? ` · same % for ${dur(r.sinceMove)}` : ""}
            </div>
            {r.stalled ? (
              <Banner kind="warn" action={<Button variant="danger" icon={<Square />} onClick={() => act({ kind: "stop" })}>Stop</Button>}>
                Looks stuck: the % has not moved and nothing has been working for {dur(r.sinceMove)}. Stopping is safe - nothing is half-repaired.
              </Banner>
            ) : (
              r.note && <div className="health-note">{r.note} It is still working - no need to stop it.</div>
            )}
          </>
        ) : s.queued ? (
          <div className="secondary">Waiting for the step before it...</div>
        ) : s.last ? (
          <div className="stack health-last">
            <div className="hstack">
              <ToneBadge tone={levelTone(s.last.level)}>{s.last.level === "ok" ? "OK" : s.last.level === "warn" ? "Check" : "Problem"}</ToneBadge>
              <span>{s.last.summary}</span>
            </div>
            {s.last.detail.map((d, i) => (
              <div key={i} className="secondary selectable health-detail">
                {d}
              </div>
            ))}
            <div className="tertiary">
              {s.last.time} · took {dur(s.last.secs)}
            </div>
          </div>
        ) : (
          <div className="tertiary">Not run yet.</div>
        )}
        <div className="toolbar">
          <Button icon={<Play />} disabled={!s.canRun} onClick={() => act({ kind: "run", step: s.id })}>
            Run {s.id === "chkdsk" ? "CHKDSK" : s.id.toUpperCase()}
          </Button>
          {s.last?.canFix && !r && (
            <Button variant="primary" icon={<Wrench />} onClick={onFix}>
              Fix at next restart
            </Button>
          )}
          {s.last?.hasLog && !r && (
            <Button variant="plain" icon={<FileText />} onClick={() => act({ kind: "open_log", step: s.id })}>
              Open log
            </Button>
          )}
        </div>
      </div>
    </Group>
  );
}

export function Health() {
  const { view, error, act } = useTab<View, Action>("health");
  const [fix, setFix] = useState(false);
  return (
    <Page title="Health" subtitle="Checks the drives and repairs Windows with its own tools (CHKDSK, DISM, SFC), showing live what they are doing.">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState />
      ) : (
        <>
          <ViewOnlyBanner admin={view.admin} text="View only: start PC Tuner as administrator to run CHKDSK, DISM and SFC." />
          <Group padded>
            <div className="hstack hero-row" style={{ flexWrap: "wrap" }}>
              <Hero value={<HeartPulse size={34} />} unit={view.overall} caption={view.lastAll ? `Last full check ${view.lastAll}` : "Run all checks to see where Windows stands"} tone={view.tone} />
            </div>
          </Group>
          <div className="toolbar">
            <Button variant="primary" icon={<Play />} disabled={!view.canRunAll} onClick={() => act({ kind: "run_all" })}>
              Run all checks
            </Button>
            {view.running && (
              <Button variant="danger" icon={<Square />} disabled={!view.canStop} title="Stops the running tool and the rest of the checks. Safe: nothing is left half-repaired." onClick={() => act({ kind: "stop" })}>
                Stop
              </Button>
            )}
            <span className="secondary">Drives, then CHKDSK → DISM → SFC. Usually 10-30 minutes; you can keep using the PC.</span>
          </div>
          {view.note && (
            <Banner kind={view.note.ok ? "ok" : "error"} action={<Button variant="plain" onClick={() => act({ kind: "dismiss_note" })}>OK</Button>}>
              {view.note.text}
            </Banner>
          )}
          {view.prep && <Banner kind="info">{view.prep}</Banner>}
          <DiskCard d={view.disk} act={act} />
          {view.steps.map((s) => (
            <StepCard key={s.id} s={s} act={act} onFix={() => setFix(true)} />
          ))}
          {fix && (
            <Sheet
              kicker="Health"
              title="Fix C: at the next restart?"
              subtitle="Windows fixes the problems CHKDSK found while C: is not in use: at the next restart, before sign-in, usually in a few seconds. Nothing changes until then."
              onClose={() => setFix(false)}
              footer={
                <>
                  <Button
                    variant="primary"
                    size="lg"
                    onClick={() => {
                      void act({ kind: "fix_chkdsk" });
                      setFix(false);
                    }}
                  >
                    Fix at next restart
                  </Button>
                  <Button size="lg" onClick={() => setFix(false)}>
                    Cancel
                  </Button>
                </>
              }
            />
          )}
          <Badge>Logs: %LOCALAPPDATA%\PCTuner\health</Badge>
        </>
      )}
    </Page>
  );
}
