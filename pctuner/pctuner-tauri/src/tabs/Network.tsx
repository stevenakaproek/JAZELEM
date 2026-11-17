// Optimization > Network: pick adapter and Windows network settings, apply them, undo the last change.
import { ArrowRight, RefreshCw } from "lucide-react";
import { OpSheet } from "../components/OpSheet";
import { BackupsButton, TabBackupsSheet, type BackupAction, type TabBackupsView } from "../components/TabBackups";
import { Badge, Button, ErrorState, Group, LoadingState, Row, Spinner } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow } from "../lib/types";
import { useTab } from "../lib/useTab";

export interface NetworkRow {
  index: number;
  name: string;
  why: string;
  optional: boolean;
  /** The setting exists on this adapter. */
  applicable: boolean;
  set: boolean;
  selectable: boolean;
  selected: boolean;
  now: string;
  target: string;
}

export interface NetworkView {
  /** Nothing was read yet. */
  loading: boolean;
  refreshing: boolean;
  /** Set when no adapter could be read; the settings are not shown then. */
  error: string | null;
  groups: { name: string; rows: NetworkRow[] }[] | null;
  pending: number;
  canApply: boolean;
  canUndo: boolean;
  op: OpWindow | null;
  opCan: boolean;
  backups: TabBackupsView | null;
}

export type NetworkAction = { kind: "toggle"; index: number } | { kind: "clear" | "apply" | "undo" | "refresh" } | { kind: "op"; ev: OpEvent } | BackupAction;

function SettingRow({ r, onToggle }: { r: NetworkRow; onToggle: () => void }) {
  return (
    <Row
      checked={r.applicable && !r.set ? r.selected : "none"}
      disabled={!r.applicable}
      onClick={r.selectable ? onToggle : undefined}
      title={
        <span className="hstack">
          {r.name}
          {r.optional && <span className="tertiary" style={{ fontWeight: 400 }}>optional</span>}
        </span>
      }
      subtitle={r.why}
      value={
        r.applicable && !r.set ? (
          <span className="hstack" style={{ gap: 6 }}>
            {r.now}
            <ArrowRight size={12} />
            <span className={r.selected ? "tone-accent" : undefined}>{r.target}</span>
          </span>
        ) : r.applicable ? (
          r.now
        ) : undefined
      }
      accessory={!r.applicable ? <Badge>Not on this adapter</Badge> : r.set ? <Badge color="green">Applied</Badge> : <Badge>Not applied</Badge>}
    />
  );
}

export function Network() {
  const { admin } = useShell();
  const { view, error, act } = useTab<NetworkView, NetworkAction>("network");
  if (error) return <ErrorState text={error} />;
  if (!view || view.loading) return <LoadingState title="Reading your network adapter..." />;
  const refresh = (
    <Button icon={view.refreshing ? <Spinner /> : <RefreshCw />} disabled={view.refreshing} onClick={() => act({ kind: "refresh" })}>
      Refresh
    </Button>
  );
  if (view.error !== null) {
    return (
      <>
        <ErrorState title="The network adapter could not be read" text={view.error} action={refresh} />
        {view.op && <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} />}
      </>
    );
  }
  return (
    <>
      <div className="toolbar">
        <Button
          variant="primary"
          disabled={!view.canApply}
          title={view.canApply ? undefined : admin ? "Click the settings you want to apply." : "Run PC Tuner as administrator to change settings."}
          onClick={() => act({ kind: "apply" })}
        >
          Apply selected ({view.pending})
        </Button>
        {view.pending > 0 && <Button onClick={() => act({ kind: "clear" })}>Clear selection</Button>}
        <Button disabled={!view.canUndo} onClick={() => act({ kind: "undo" })}>
          Undo last change
        </Button>
        <span className="spacer" />
        <BackupsButton act={act} />
        {refresh}
      </div>
      {(view.groups ?? []).map((g) => (
        <Group key={g.name} title={g.name} count={`${g.rows.filter((r) => r.set).length} of ${g.rows.length} applied`}>
          {g.rows.map((r) => (
            <SettingRow key={r.index} r={r} onToggle={() => act({ kind: "toggle", index: r.index })} />
          ))}
        </Group>
      ))}
      {view.op ? <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} /> : view.backups && <TabBackupsSheet b={view.backups} kicker="Optimization · Network" act={act} />}
    </>
  );
}
