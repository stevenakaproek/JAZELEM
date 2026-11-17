// CPU Scheduler: processor scheduling and the multimedia scheduler, each edited and applied on its own.
import { useState } from "react";
import { Badge, Button, Dot, ErrorState, Group, LoadingState, NumberField, Page, Row, Segmented, ToneText, ViewOnlyBanner } from "../components/ui";
import { useShell } from "../lib/shell";
import type { Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/scheduler.css";

export type SchedulerSection = "Processor" | "Multimedia";

interface Status {
  text: string;
  tone: Tone;
}

/** One choice of a group: `on` = picked on the page, `now` = what the registry holds. */
export interface SchedulerOption {
  value: number;
  label: string;
  on: boolean;
  now: boolean;
}

export interface SchedulerRow {
  task: string;
  name: string;
  label: string;
  text: string;
  status: Status;
  control: "number" | "affinity" | "choice";
  number?: number;
  min?: number;
  max?: number;
  all?: boolean;
  cpus?: boolean[];
  selected?: number;
  options?: string[];
}

export interface SchedulerView {
  can: boolean;
  section: SchedulerSection;
  sections: { id: SchedulerSection; name: string; dirty: boolean }[];
  applied: string[];
  processor: {
    interval: SchedulerOption[];
    length: SchedulerOption[];
    boost: SchedulerOption[];
    boostEnabled: boolean;
    foreground: number;
    foregroundMax: number;
    foregroundStatus: Status;
    bitmask: string;
    bitmaskStatus: Status;
  } | null;
  multimedia: { system: SchedulerRow[]; tasks: string[]; task: number; rows: SchedulerRow[] } | null;
  pending: string[];
  canApply: boolean;
  canCancel: boolean;
  canUndo: boolean;
  report: Status[];
}

export type SchedulerAction =
  | { kind: "section"; section: SchedulerSection }
  | { kind: "task"; index: number }
  | { kind: "interval" | "length" | "boost" | "foreground"; value: number }
  | { kind: "restore_defaults" | "apply" | "cancel" | "undo" | "backup" | "reload" }
  | { kind: "choose"; task: string; name: string; index: number }
  | { kind: "number"; task: string; name: string; value: number }
  | { kind: "affinity_all"; task: string; on: boolean }
  | { kind: "affinity_cpu"; task: string; cpu: number; on: boolean };

type Act = (a: SchedulerAction) => Promise<void>;

function StatusText({ s }: { s: Status }) {
  return s.text ? <ToneText tone={s.tone}>{s.text}</ToneText> : null;
}

/** A group of either-or rows; the marks show what is applied now and what is picked but not applied. */
function Choices({ title, options, disabled, onPick }: { title?: string; options: SchedulerOption[]; disabled: boolean; onPick: (v: number) => void }) {
  return (
    <Group title={title}>
      {options.map((o) => (
        <Row
          key={o.value}
          checked={o.on}
          disabled={disabled}
          onClick={() => onPick(o.value)}
          title={o.label}
          accessory={o.now ? <Badge color="green">Applied</Badge> : o.on ? <Badge color="accent">Not applied yet</Badge> : undefined}
        />
      ))}
    </Group>
  );
}

function SettingRow({ r, can, act }: { r: SchedulerRow; can: boolean; act: Act }) {
  const [open, setOpen] = useState(false);
  const status = <StatusText s={r.status} />;
  if (r.control === "number") {
    return (
      <Row
        title={r.label}
        subtitle={status}
        accessory={
          <span className="sched-number">
            <NumberField label={r.label} value={r.number ?? 0} min={r.min ?? 0} max={r.max ?? 0} disabled={!can} onCommit={(value) => act({ kind: "number", task: r.task, name: r.name, value: Math.round(value) })} />
          </span>
        }
      />
    );
  }
  if (r.control === "affinity") {
    return (
      <>
        <Row
          title={r.label}
          subtitle={status}
          accessory={
            <span className="hstack">
              <span className="mono">{r.text}</span>
              <Button disabled={!can} aria-expanded={open} onClick={() => setOpen(!open)}>
                select ...
              </Button>
            </span>
          }
        />
        {open && can && (
          <div className="row">
            <div className="sched-cpus">
              <label className="all">
                <input type="checkbox" checked={r.all ?? false} onChange={(e) => act({ kind: "affinity_all", task: r.task, on: e.target.checked })} />
                All CPUs
              </label>
              {(r.cpus ?? []).map((on, cpu) => (
                <label key={cpu}>
                  <input type="checkbox" checked={on} onChange={(e) => act({ kind: "affinity_cpu", task: r.task, cpu, on: e.target.checked })} />
                  CPU {cpu}
                </label>
              ))}
            </div>
          </div>
        )}
      </>
    );
  }
  return (
    <Row
      title={r.label}
      subtitle={status}
      accessory={
        <select className="input select sched-control" aria-label={r.label} value={r.selected ?? 0} disabled={!can} onChange={(e) => act({ kind: "choose", task: r.task, name: r.name, index: Number(e.target.value) })}>
          {(r.options ?? []).map((o, i) => (
            <option key={i} value={i}>
              {o}
            </option>
          ))}
        </select>
      }
    />
  );
}

function Processor({ p, can, act }: { p: NonNullable<SchedulerView["processor"]>; can: boolean; act: Act }) {
  return (
    <>
      <Choices title="Interval length" options={p.interval} disabled={!can} onPick={(value) => act({ kind: "interval", value })} />
      <Choices title="Fixed / variable intervals" options={p.length} disabled={!can} onPick={(value) => act({ kind: "length", value })} />
      <Choices options={p.boost} disabled={!p.boostEnabled} onPick={(value) => act({ kind: "boost", value })} />
      <Group title="Foreground lock timeout">
        <Row
          title="Foreground lock timeout"
          subtitle={<StatusText s={p.foregroundStatus} />}
          accessory={
            <span className="sched-number">
              <NumberField label="Foreground lock timeout in milliseconds" value={p.foreground} min={0} max={p.foregroundMax} suffix="ms" disabled={!can} onCommit={(value) => act({ kind: "foreground", value: Math.round(value) })} />
            </span>
          }
        />
        <Row
          title="Bitmask (hex):"
          subtitle={<StatusText s={p.bitmaskStatus} />}
          value={<strong className="mono">{p.bitmask}</strong>}
          accessory={
            <Button disabled={!can} title="Bitmask 2 and 200000 ms. Click Apply to apply." onClick={() => act({ kind: "restore_defaults" })}>
              Restore defaults
            </Button>
          }
        />
      </Group>
    </>
  );
}

function Multimedia({ m, can, act }: { m: NonNullable<SchedulerView["multimedia"]>; can: boolean; act: Act }) {
  return (
    <>
      <Group title="Multimedia scheduler">
        {m.system.map((r) => (
          <SettingRow key={r.name} r={r} can={can} act={act} />
        ))}
      </Group>
      <Group title="Task">
        <Row
          title="Task"
          accessory={
            <select className="input select sched-control" aria-label="Task" value={m.task} onChange={(e) => act({ kind: "task", index: Number(e.target.value) })}>
              {m.tasks.map((t, i) => (
                <option key={i} value={i}>
                  {t}
                </option>
              ))}
            </select>
          }
        />
        {m.rows.map((r) => (
          <SettingRow key={`${r.task}/${r.name}`} r={r} can={can} act={act} />
        ))}
      </Group>
    </>
  );
}

export function Scheduler() {
  const { admin } = useShell();
  const { view, error, act } = useTab<SchedulerView, SchedulerAction>("scheduler");
  return (
    <Page title="CPU Scheduler">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState />
      ) : (
        <>
          <ViewOnlyBanner admin={admin} />
          <div className="toolbar">
            <Segmented
              value={view.section}
              options={view.sections.map((s) => ({
                value: s.id,
                label: (
                  <span className="hstack" title={s.dirty ? "Not applied yet" : undefined}>
                    {s.name}
                    {s.dirty && <Dot color="accent" />}
                  </span>
                ),
              }))}
              onChange={(section) => act({ kind: "section", section })}
            />
          </div>
          {view.applied.length === 0 ? (
            <Group padded>
              <span className="secondary">Everything is at the Windows default.</span>
            </Group>
          ) : (
            <Group title={`Applied now - ${view.applied.length} changed from default`} padded>
              <div className="sched-lines selectable">
                {view.applied.map((a) => (
                  <span key={a} className="tone-ok">
                    {a}
                  </span>
                ))}
              </div>
            </Group>
          )}
          {view.processor && <Processor p={view.processor} can={view.can} act={act} />}
          {view.multimedia && <Multimedia m={view.multimedia} can={view.can} act={act} />}
          <div className="toolbar">
            <Button variant="primary" disabled={!view.canApply} onClick={() => act({ kind: "apply" })}>
              Apply ({view.pending.length})
            </Button>
            <Button disabled={!view.canCancel} title="Discard the changes on this tab that are not applied yet." onClick={() => act({ kind: "cancel" })}>
              Cancel
            </Button>
            <Button disabled={!view.canUndo} onClick={() => act({ kind: "undo" })}>
              Undo last apply
            </Button>
            <Button onClick={() => act({ kind: "backup" })}>Backup registry entries</Button>
            <Button title="Read the registry again." onClick={() => act({ kind: "reload" })}>
              Reload
            </Button>
          </div>
          {view.pending.length > 0 && (
            <div className="sched-lines selectable">
              <span className="tone-accent">Not applied yet:</span>
              {view.pending.map((line) => (
                <span key={line} className="secondary">
                  {"  "}
                  {line}
                </span>
              ))}
            </div>
          )}
          {view.report.length > 0 && (
            <div className="sched-lines selectable">
              {view.report.map((l, i) => (
                <ToneText key={i} tone={l.tone}>
                  {l.text}
                </ToneText>
              ))}
            </div>
          )}
        </>
      )}
    </Page>
  );
}
