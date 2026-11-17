// Optimization > Performance: pick settings, apply them, and the Defender game-folder card.
import { ArrowRight, FolderPlus, RefreshCw } from "lucide-react";
import { useState } from "react";
import { OpSheet } from "../components/OpSheet";
import { BackupsButton, TabBackupsSheet, type BackupAction, type TabBackupsView } from "../components/TabBackups";
import { Badge, Banner, Button, ErrorState, Group, LoadingState, Row, Spinner, Switch } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow } from "../lib/types";
import { useTab } from "../lib/useTab";

export interface PerfRow {
  index: number;
  name: string;
  why: string;
  optional: boolean;
  applicable: boolean;
  blocked: string;
  set: boolean;
  selectable: boolean;
  selected: boolean;
  now: string;
  target: string;
  extra: string | null;
  /** the on / off switch can be used now */
  canSwitch: boolean;
}

export interface DefenderView {
  scanning: boolean;
  error?: string;
  passive?: boolean;
  readable?: boolean;
  rows?: { kind: string; path: string; done: boolean; on: boolean; canTick: boolean }[];
  customErr?: string | null;
  todo?: number;
  canExclude?: boolean;
  canUndo?: boolean;
  canRescan?: boolean;
  op: OpWindow | null;
  opCan: boolean;
}

export interface PerfView {
  loading: boolean;
  refreshing: boolean;
  error: string | null;
  groups: { name: string; rows: PerfRow[] }[] | null;
  pending: number;
  canApply: boolean;
  canUndo: boolean;
  canExplorer: boolean;
  note: string | null;
  op: OpWindow | null;
  opCan: boolean;
  defender: DefenderView;
  backups: TabBackupsView | null;
}

export type PerfAction =
  | { kind: "toggle"; index: number }
  | { kind: "switch"; index: number; on: boolean }
  | { kind: "clear" | "apply" | "undo" | "explorer" | "refresh" }
  | { kind: "op"; ev: OpEvent }
  | { kind: "defender_tick"; path: string; on: boolean }
  | { kind: "defender_add"; path: string }
  | { kind: "defender_exclude" | "defender_undo" | "defender_rescan" }
  | { kind: "defender_op"; ev: OpEvent }
  | BackupAction;

function SettingRow({ r, onToggle, onSwitch }: { r: PerfRow; onToggle: () => void; onSwitch: (on: boolean) => void }) {
  const on = r.set;
  const canSwitch = r.canSwitch;
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
      subtitle={
        <>
          {r.why}
          {r.extra && <div className="tertiary">{r.extra}</div>}
        </>
      }
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
      accessory={
        !r.applicable ? (
          <Badge title={r.blocked}>Not available</Badge>
        ) : (
          <span className="hstack opt-switch">
            <span className={on ? "tone-ok" : "tertiary"}>{on ? "Applied" : "Not applied"}</span>
            <Switch on={on} disabled={!canSwitch} label={on ? "Switch this setting off" : "Switch this setting on"} onChange={onSwitch} />
          </span>        )
      }
    />
  );
}

function DefenderCard({ d, admin, act }: { d: DefenderView; admin: boolean; act: (a: PerfAction) => Promise<void> }) {
  const [custom, setCustom] = useState("");
  const add = async () => {
    await act({ kind: "defender_add", path: custom });
    setCustom("");
  };
  const footer =
    "Excluded folders are not scanned by Microsoft Defender: games can load and patch a little faster, but Defender will not catch anything harmful inside them. Real-time protection stays on everywhere else.";
  if (d.scanning) {
    return (
      <Group title="Defender: game folders" count="optional" footer={footer}>
        <Row title="Looking for game folders..." accessory={<Spinner />} />
      </Group>
    );
  }
  const rows = d.rows ?? [];
  return (
    <>
      {d.error && <Banner kind="warn">{d.error}</Banner>}
      {d.passive && <Banner kind="warn">Another antivirus is active - Defender is in passive mode, so its exclusions do nothing. Use that antivirus' own exclusions.</Banner>}
      {!d.readable && !d.error && <Banner kind="locked">Run PC Tuner as administrator to see and change Defender exclusions.</Banner>}
      <Group title="Defender: game folders" count="optional" footer={footer}>
        {rows.length === 0 && <Row title={<span className="secondary">No game folders found. Add one below.</span>} />}
        {rows.map((r) => (
          <Row
            key={r.path}
            checked={r.done ? "none" : r.on}
            onClick={r.canTick ? () => act({ kind: "defender_tick", path: r.path, on: !r.on }) : undefined}
            title={<span className="selectable">{r.path}</span>}
            subtitle={r.kind}
            accessory={r.done ? <Badge color="green">Excluded</Badge> : r.on && d.readable ? <Badge color="orange">Will exclude</Badge> : <Badge>Scanned</Badge>}
          />
        ))}
        <div className="row">
          <span className="row-main hstack">
            <input className="input" style={{ flex: 1, maxWidth: 320 }} value={custom} placeholder="Other games folder, e.g. D:\Games" spellCheck={false} onChange={(e) => setCustom(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
            <Button icon={<FolderPlus />} onClick={add}>
              Add folder
            </Button>
            {d.customErr && <span className="tone-error">{d.customErr}</span>}
          </span>
        </div>
      </Group>
      <div className="toolbar">
        <Button variant="primary" disabled={!d.canExclude} onClick={() => act({ kind: "defender_exclude" })}>
          {d.todo === 1 ? "Exclude 1 folder" : d.todo ? `Exclude ${d.todo} folders` : "Exclude"}
        </Button>
        <Button disabled={!d.canUndo} title="Removes only the exclusions PC Tuner added." onClick={() => act({ kind: "defender_undo" })}>
          Undo exclusions
        </Button>
        <Button disabled={!d.canRescan} onClick={() => act({ kind: "defender_rescan" })}>
          Rescan
        </Button>
        {!admin && <span className="secondary">View only</span>}
      </div>
    </>
  );
}

export function Performance() {
  const { admin } = useShell();
  const { view, error, act } = useTab<PerfView, PerfAction>("perf");
  if (error) return <ErrorState text={error} />;
  if (!view || view.loading) return <LoadingState title="Reading your settings..." />;
  if (view.error) {
    return (
      <ErrorState
        title="The settings could not be read"
        text={
          <>
            {view.error}
            <br />
            Nothing can be changed until the settings are read correctly. Press Refresh to try again.
          </>
        }
        action={
          <Button icon={<RefreshCw />} disabled={view.refreshing} onClick={() => act({ kind: "refresh" })}>
            Refresh
          </Button>
        }
      />
    );
  }
  return (
    <>
      <div className="toolbar">
        <Button variant="primary" disabled={!view.canApply} title={admin ? "Click the settings you want to apply." : "Run PC Tuner as administrator to change settings."} onClick={() => act({ kind: "apply" })}>
          Apply selected ({view.pending})
        </Button>
        {view.pending > 0 && <Button onClick={() => act({ kind: "clear" })}>Clear selection</Button>}
        <Button disabled={!view.canUndo} onClick={() => act({ kind: "undo" })}>
          Undo last change
        </Button>
        <Button disabled={!view.canExplorer} title="Shows taskbar and right-click changes. Shows a preview first. Not during a game: the taskbar disappears for a moment." onClick={() => act({ kind: "explorer" })}>
          Restart Explorer
        </Button>
        <span className="spacer" />
        {view.note && <span className="secondary">{view.note}</span>}
        <BackupsButton act={act} />
        <Button icon={view.refreshing ? <Spinner /> : <RefreshCw />} disabled={view.refreshing} onClick={() => act({ kind: "refresh" })}>
          Refresh
        </Button>
      </div>
      {(view.groups ?? []).map((g) => (
        <Group key={g.name} title={g.name} count={`${g.rows.filter((r) => r.set).length} of ${g.rows.length} applied`}>
          {g.rows.map((r) => (
            <SettingRow key={r.index} r={r} onToggle={() => act({ kind: "toggle", index: r.index })} onSwitch={(on) => act({ kind: "switch", index: r.index, on })} />
          ))}
        </Group>
      ))}
      <DefenderCard d={view.defender} admin={admin} act={act} />
      {view.op ? (
        <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} />
      ) : view.backups ? (
        <TabBackupsSheet b={view.backups} kicker="Optimization · Performance" act={act} />
      ) : (
        view.defender.op && <OpSheet op={view.defender.op} can={view.defender.opCan} admin={admin} onEvent={(ev) => act({ kind: "defender_op", ev })} />
      )}
    </>
  );
}
