// Optimization > Windows tweaks: the post-install tweaks, their status and the last run's log.
import { RefreshCw } from "lucide-react";
import { useEffect, useRef } from "react";
import { OpSheet } from "../components/OpSheet";
import { Badge, Button, ErrorState, Group, LoadingState, Row, Spinner, Switch, cx } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/tweaks.css";

export interface TweakRow {
  index: number;
  name: string;
  text: string;
  /** Cannot be undone by "Undo tweaks" (Remove OneDrive). */
  danger: boolean;
  /** null while the status is being read. */
  applied: boolean | null;
  selectable: boolean;
  selected: boolean;
  /** The current value; null while the status is being read. */
  now: string | null;
}

export interface TweaksLastRun {
  title: string;
  /** A script ran (or runs) in this session. */
  ran: boolean;
  running?: boolean;
  exit?: number | null;
  chips?: { text: string; ok: boolean }[];
  showDetails?: boolean;
  lines?: { text: string; fail: boolean }[];
}

export interface TweaksView {
  rows: TweakRow[];
  pending: number;
  canApply: boolean;
  canUndo: boolean;
  canRefresh: boolean;
  lastRun: TweaksLastRun;
  op: OpWindow | null;
  opCan: boolean;
}

export type TweaksAction = { kind: "toggle"; index: number } | { kind: "clear" | "apply" | "undo" | "refresh" } | { kind: "details"; on: boolean } | { kind: "op"; ev: OpEvent };

function TweakRowView({ r, onToggle }: { r: TweakRow; onToggle: () => void }) {
  return (
    <Row
      checked={r.applied === false ? r.selected : "none"}
      onClick={r.selectable ? onToggle : undefined}
      title={<span className={r.danger ? "tone-error" : undefined}>{r.name}</span>}
      subtitle={r.text}
      value={r.now !== null ? <span className={r.selected ? "tone-accent" : undefined}>{r.now}</span> : undefined}
      accessory={r.applied === null ? <Badge>Checking...</Badge> : r.applied ? <Badge color="green">Applied</Badge> : <Badge>Not applied</Badge>}
    />
  );
}

/** The script's raw output; stays at the newest line while it grows. */
function Log({ lines }: { lines: { text: string; fail: boolean }[] }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [lines.length]);
  return (
    <div className="tweaks-log mono selectable" ref={ref}>
      {lines.map((l, i) => (
        <div key={i} className={cx(l.fail && "tone-error")}>
          {l.text}
        </div>
      ))}
    </div>
  );
}

function LastRun({ run, onDetails }: { run: TweaksLastRun; onDetails: (on: boolean) => void }) {
  if (!run.ran) {
    return (
      <Group title={run.title}>
        <Row title={<span className="secondary">Nothing has run in this session yet.</span>} />
      </Group>
    );
  }
  const chips = run.chips ?? [];
  return (
    <Group title={run.title}>
      <Row
        title={
          <span className="hstack" style={{ flexWrap: "wrap" }}>
            {run.running ? (
              <>
                <Spinner />
                Running...
              </>
            ) : run.exit === 0 ? (
              <Badge color="green">Finished</Badge>
            ) : (
              <Badge color="red">Stopped, exit code {run.exit}</Badge>
            )}
            {chips.map((c) => (
              <Badge key={c.text} color={c.ok ? "green" : "orange"}>
                {c.text}
              </Badge>
            ))}
          </span>
        }
      />
      <Row title="Show technical details" accessory={<Switch on={!!run.showDetails} onChange={onDetails} label="Show technical details" />} />
      {run.showDetails && <Log lines={run.lines ?? []} />}
    </Group>
  );
}

export function Tweaks() {
  const { admin } = useShell();
  const { view, error, act } = useTab<TweaksView, TweaksAction>("tweaks");
  if (error) return <ErrorState text={error} />;
  if (!view) return <LoadingState />;
  return (
    <>
      <div className="toolbar">
        <Button
          variant="primary"
          disabled={!view.canApply}
          title={view.canApply ? undefined : admin ? "Click the tweaks you want to apply." : "Run PC Tuner as administrator to apply tweaks."}
          onClick={() => act({ kind: "apply" })}
        >
          Apply selected ({view.pending})
        </Button>
        {view.pending > 0 && <Button onClick={() => act({ kind: "clear" })}>Clear selection</Button>}
        <Button disabled={!view.canUndo} onClick={() => act({ kind: "undo" })}>
          Undo tweaks
        </Button>
        <span className="spacer" />
        <Button icon={<RefreshCw />} disabled={!view.canRefresh} onClick={() => act({ kind: "refresh" })}>
          Refresh status
        </Button>
      </div>
      <Group title="Tweaks" count={`${view.rows.filter((r) => r.applied).length} of ${view.rows.length} applied`}>
        {view.rows.map((r) => (
          <TweakRowView key={r.index} r={r} onToggle={() => act({ kind: "toggle", index: r.index })} />
        ))}
      </Group>
      <LastRun run={view.lastRun} onDetails={(on) => act({ kind: "details", on })} />
      {view.op && <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} />}
    </>
  );
}
