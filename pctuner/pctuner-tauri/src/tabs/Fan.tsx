// Fan Tuner: live speeds and temperatures, profiles, and each fan's BIOS / Manual / Curve / Trigger setting.
import { ChevronRight, RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Banner, Button, ErrorState, Group, Hero, LoadingState, NumberField, Page, Progress, Segmented, Slider, Sparkline, Stat, Switch, ToneBadge, cx } from "../components/ui";
import type { Tone } from "../lib/types";
import { toneClass } from "../lib/types";
import { useTab } from "../lib/useTab";
import { CurveEditor, TriggerEditor, type GraphLimits } from "./FanGraphs";

export type FanSource = "cpu" | "gpu" | "hottest";

export type FanSetting =
  | { mode: "bios" }
  | { mode: "manual"; percent: number }
  | { mode: "curve"; source: FanSource; points: [number, number][] }
  | { mode: "trigger"; source: FanSource; idle_temp: number; load_temp: number; idle: number; load: number };

export interface FanCardView {
  id: string;
  name: string;
  hw: string;
  rpm: number | null;
  percent: number | null;
  controllable: boolean;
  pump: boolean;
  gpu: boolean;
  pinned: boolean;
  chip: { text: string; tone: Tone };
  hist: number[];
  floor: number;
  setting: FanSetting;
  notice: string | null;
  follow: string;
  live: number | null;
  liveDuty: number | null;
  afterburner: boolean;
}

interface Temp {
  value: number;
  tone: Tone;
}

export interface FanView {
  can: boolean;
  viewOnly: boolean;
  error: string | null;
  anyControlled: boolean;
  profile: { names: string[]; active: string | null; modified: boolean; builtin: boolean; note: string | null; newName: string; newNameOk: boolean; newNameWhy: string | null };
  sources: { key: FanSource; label: string }[];
  graph: GraphLimits;
  stale?: { head: string; sub: string };
  reading?: boolean;
  status?: {
    emergency: boolean;
    blind: boolean;
    msg: { seq: number; text: string } | null;
    hottest: (Temp & { which: string }) | null;
    cpu: Temp | null;
    gpu: Temp | null;
    fansFound: number;
    controllable: number;
    cpuHist: number[];
    gpuHist: number[];
    fans: FanCardView[];
    empty: FanCardView[];
    temps: { hw: string; name: string; value: number; tone: Tone }[];
  };
}

export type FanAction =
  | { kind: "give_back" | "save_changes" | "reset_builtin" | "delete_profile" | "save_as_new" }
  | { kind: "dismiss"; seq: number }
  | { kind: "load_profile"; name: string }
  | { kind: "new_name"; name: string }
  | { kind: "mode"; id: string; mode: FanSetting["mode"] }
  | { kind: "manual"; id: string; percent: number }
  | { kind: "source"; id: string; source: FanSource }
  | { kind: "curve_point"; id: string; index: number; t: number; d: number }
  | { kind: "curve_add"; id: string; t: number; d: number }
  | { kind: "curve_remove"; id: string; index: number }
  | { kind: "trigger"; id: string; idle_temp: number; load_temp: number; idle: number; load: number }
  | { kind: "follow"; id: string }
  | { kind: "pump"; id: string; on: boolean };

type Act = (a: FanAction) => Promise<void>;

const toneVar: Record<Tone, string> = { Ok: "var(--green)", Accent: "var(--accent)", Text: "var(--text)", Dim: "var(--text-secondary)", Faint: "var(--text-tertiary)", Off: "var(--text-tertiary)", Warn: "var(--yellow)", Junk: "var(--orange)", Error: "var(--red)" };
const deg = (t: Temp | null) => (t ? `${t.value.toFixed(0)} °C` : "-");

const MODES: { value: FanSetting["mode"]; label: string; tip: string }[] = [
  { value: "bios", label: "BIOS", tip: "Hands off: the BIOS (or graphics driver) curve decides." },
  { value: "manual", label: "Manual", tip: "A fixed speed you choose with the slider." },
  { value: "curve", label: "Curve", tip: "Speed follows a temperature: drag the points on the graph." },
  { value: "trigger", label: "Trigger", tip: "Two steady speeds for gaming: quiet until the temperature reaches the gaming point, then one fixed speed until it falls back to the idle point. No revving on short temperature spikes." },
];

function SourcePicker({ value, sources, disabled, onChange }: { value: FanSource; sources: FanView["sources"]; disabled: boolean; onChange: (s: FanSource) => void }) {
  return (
    <select className="input select" value={value} disabled={disabled} onChange={(e) => onChange(e.target.value as FanSource)} aria-label="Temperature this fan follows">
      {sources.map((s) => (
        <option key={s.key} value={s.key}>
          {s.label}
        </option>
      ))}
    </select>
  );
}

function ManualControl({ f, disabled, act }: { f: FanCardView; disabled: boolean; act: Act }) {
  const percent = f.setting.mode === "manual" ? Math.max(f.setting.percent, f.floor) : f.floor;
  const [local, setLocal] = useState(percent);
  useEffect(() => setLocal(percent), [percent]);
  const send = (v: number) => {
    setLocal(v);
    void act({ kind: "manual", id: f.id, percent: v });
  };
  return (
    <div className="stack" style={{ gap: 4 }}>
      <div className="hstack" style={{ gap: 12 }}>
        <Slider label={`${f.name} speed`} value={local} min={f.floor} max={100} disabled={disabled} onChange={send} />
        <NumberField label={`${f.name} speed in percent`} value={local} min={f.floor} max={100} suffix="%" disabled={disabled} onCommit={send} />
      </div>
      <div className="tertiary" style={{ fontSize: "var(--text-sm)" }}>
        Lowest allowed: {f.floor.toFixed(0)} %
      </div>
    </div>
  );
}

function FanCard({ f, view, act }: { f: FanCardView; view: FanView; act: Act }) {
  const disabled = !view.can;
  const s = f.setting;
  return (
    <div className="card fan-card" title={disabled && f.controllable ? "Run PC Tuner as administrator to control fans." : undefined}>
      <div className="hstack">
        <div className="fan-name">{f.name}</div>
        <span className="spacer" />
        {f.pinned && <Badge color="yellow">Own setting</Badge>}
        {f.pump && <Badge>Pump</Badge>}
        <ToneBadge tone={f.chip.tone}>{f.chip.text}</ToneBadge>
      </div>
      <div className="tertiary" style={{ fontSize: "var(--text-sm)" }}>
        {f.hw}
      </div>
      <div className="hstack" style={{ gap: 20, alignItems: "flex-end", marginTop: 4 }}>
        <Stat value={f.rpm === null ? "-" : f.rpm.toFixed(0)} unit="RPM" label="speed" />
        {f.percent !== null && <Stat value={f.percent.toFixed(0)} unit="%" label="duty" />}
      </div>
      <Sparkline values={f.hist} />
      {!f.controllable ? (
        <div className="tertiary" style={{ fontSize: "var(--text-sm)" }}>
          Shown only - this header cannot be controlled by the library on this board.
        </div>
      ) : (
        <div className="stack" style={{ gap: 10 }}>
          {f.notice && (
            <div className="tone-warn" style={{ fontSize: "var(--text-sm)" }}>
              {f.notice}
            </div>
          )}
          {f.pinned ? (
            <div className="hstack" style={{ flexWrap: "wrap" }}>
              <span className="tone-warn" style={{ fontSize: "var(--text-sm)" }}>
                Own setting - profiles leave this fan alone.
              </span>
              <Button variant="plain" disabled={disabled} title={f.follow} onClick={() => act({ kind: "follow", id: f.id })}>
                Follow profile
              </Button>
            </div>
          ) : (
            <div className="tertiary" style={{ fontSize: "var(--text-sm)" }}>
              {f.follow}
            </div>
          )}
          <Segmented value={s.mode} disabled={disabled} options={MODES.map((m) => ({ value: m.value, label: <span title={m.tip}>{m.label}</span> }))} onChange={(mode) => act({ kind: "mode", id: f.id, mode })} />
          {s.mode === "manual" && <ManualControl f={f} disabled={disabled} act={act} />}
          {s.mode === "curve" && (
            <>
              <SourcePicker value={s.source} sources={view.sources} disabled={disabled} onChange={(source) => act({ kind: "source", id: f.id, source })} />
              <CurveEditor
                points={s.points}
                floor={f.floor}
                live={f.live}
                liveDuty={f.liveDuty}
                limits={view.graph}
                disabled={disabled}
                onPoint={(index, t, d) => act({ kind: "curve_point", id: f.id, index, t, d })}
                onAdd={(t, d) => act({ kind: "curve_add", id: f.id, t, d })}
                onRemove={(index) => act({ kind: "curve_remove", id: f.id, index })}
              />
            </>
          )}
          {s.mode === "trigger" && (
            <>
              <SourcePicker value={s.source} sources={view.sources} disabled={disabled} onChange={(source) => act({ kind: "source", id: f.id, source })} />
              <TriggerEditor v={s} floor={f.floor} live={f.live} liveDuty={f.liveDuty} limits={view.graph} disabled={disabled} onChange={(v) => act({ kind: "trigger", id: f.id, idle_temp: v.idle_temp, load_temp: v.load_temp, idle: v.idle, load: v.load })} />
            </>
          )}
          {!f.gpu && (
            <label className="hstack" title="Its default curve follows the CPU temperature.">
              <Switch on={f.pump} disabled={disabled} label="This is a pump" onChange={(on) => act({ kind: "pump", id: f.id, on })} />
              <span>This is a pump</span>
            </label>
          )}
          {f.afterburner && (
            <div className="tone-warn" style={{ fontSize: "var(--text-sm)" }}>
              MSI Afterburner is running. Turn off its fan control (Settings &gt; Fan &gt; untick "Enable user defined software automatic fan control") so the two don't fight.
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ProfileBar({ view, act }: { view: FanView; act: Act }) {
  const p = view.profile;
  const disabled = !view.can;
  const [name, setName] = useState(p.newName);
  useEffect(() => {
    if (p.newName === "") setName("");
  }, [p.newName]);
  return (
    <Group title="Profile" padded footer={p.note ?? undefined}>
      <div className="stack">
        <div className="profile-pills">
          {p.names.map((n) => (
            <button key={n} type="button" className={cx("pill", p.active === n && "on")} disabled={disabled} onClick={() => act({ kind: "load_profile", name: n })}>
              {n}
            </button>
          ))}
        </div>
        <div className="toolbar">
          {p.active ? (
            <>
              <strong>{p.active}</strong>
              {p.modified ? <Badge color="yellow">Changed - not saved</Badge> : <Badge>Saved</Badge>}
              <Button disabled={disabled || !p.modified} title={`Store the fans as they are now in ${p.active}.`} onClick={() => act({ kind: "save_changes" })}>
                Save changes
              </Button>
              {p.builtin ? (
                <Button disabled={disabled} title={`Throw away your edits to ${p.active} and use PC Tuner's default curves.`} onClick={() => act({ kind: "reset_builtin" })}>
                  Reset to default
                </Button>
              ) : (
                <Button variant="danger" disabled={disabled} onClick={() => act({ kind: "delete_profile" })}>
                  Delete
                </Button>
              )}
            </>
          ) : (
            <span className="secondary">No profile - pick one, or set fans below and save them as a profile.</span>
          )}
          <span className="spacer" />
          <input
            className="input"
            style={{ width: 170 }}
            value={name}
            placeholder="New profile name"
            disabled={disabled}
            spellCheck={false}
            onChange={(e) => {
              setName(e.target.value);
              void act({ kind: "new_name", name: e.target.value });
            }}
          />
          <Button disabled={disabled || !p.newNameOk} title={p.newNameWhy ?? (p.newNameOk ? undefined : "Type a name for the new profile.")} onClick={() => act({ kind: "save_as_new" })}>
            Save as new
          </Button>
        </div>
        {p.newNameWhy && name.trim() !== "" && (
          <div className="tone-warn" style={{ fontSize: "var(--text-sm)" }}>
            {p.newNameWhy}
          </div>
        )}
      </div>
    </Group>
  );
}

function Body({ view, act }: { view: FanView; act: Act }) {
  const [showEmpty, setShowEmpty] = useState(false);
  if (view.stale) {
    return (
      <Group padded>
        <div className="stack" style={{ gap: 4 }}>
          <div style={{ fontSize: "var(--text-lg)", fontWeight: 600 }}>{view.stale.head}</div>
          <div className="secondary">{view.stale.sub}</div>
        </div>
      </Group>
    );
  }
  const s = view.status;
  if (!s) {
    return (
      <Group padded>
        <div className="stack">
          <div style={{ fontSize: "var(--text-lg)", fontWeight: 600 }}>Reading sensors...</div>
          <Progress value={null} />
        </div>
      </Group>
    );
  }
  return (
    <>
      {s.blind && <Banner kind="warn">Temperatures cannot be read - every fan is on BIOS control until they can be read again. Your settings come back by themselves.</Banner>}
      {s.msg && (
        <Banner
          kind="warn"
          action={
            <Button variant="plain" onClick={() => act({ kind: "dismiss", seq: s.msg!.seq })}>
              Dismiss
            </Button>
          }
        >
          {s.msg.text}
        </Banner>
      )}
      {s.emergency && <Banner kind="error">Over 90 °C - every controlled fan is at 100 % until the CPU and GPU are back under 80 °C.</Banner>}
      <Group padded>
        <div className="stack" style={{ gap: 14 }}>
          <div className="hstack hero-row">
            {s.hottest ? (
              <Hero value={`${s.hottest.value.toFixed(0)} °C`} unit="hottest" caption={`${s.hottest.which} is the warmest right now - 90 °C sends every fan to 100 %`} tone={s.hottest.tone} />
            ) : (
              <Hero value="-" unit="no temperature" caption="CPU and GPU temperatures could not be read" tone="Dim" />
            )}
            <Stat value={deg(s.cpu)} label="CPU" tone={s.cpu?.tone ?? "Dim"} />
            <Stat value={deg(s.gpu)} label="GPU" tone={s.gpu?.tone ?? "Dim"} />
            <Stat value={s.fansFound} label="fans found" />
            <Stat value={s.controllable} label="can be controlled" />
          </div>
          <div className="grid" style={{ gridTemplateColumns: "1fr 1fr", gap: 16 }}>
            <div>
              <div className="stat-label">CPU temperature</div>
              <Sparkline values={s.cpuHist} color={toneVar[s.cpu?.tone ?? "Accent"]} />
            </div>
            <div>
              <div className="stat-label">GPU temperature</div>
              <Sparkline values={s.gpuHist} color={toneVar[s.gpu?.tone ?? "Accent"]} />
            </div>
          </div>
        </div>
      </Group>
      <ProfileBar view={view} act={act} />
      <section className="group-wrap">
        <div className="group-title">
          <span>Fans</span>
        </div>
        {s.fans.length + s.empty.length === 0 && (
          <div className="card">
            <span className="secondary">No fans were found.</span>
          </div>
        )}
        <div className="fan-grid">
          {s.fans.map((f) => (
            <FanCard key={f.id} f={f} view={view} act={act} />
          ))}
        </div>
        {s.empty.length > 0 && (
          <>
            <button type="button" className="disclosure" aria-expanded={showEmpty} onClick={() => setShowEmpty(!showEmpty)}>
              <ChevronRight style={{ transform: showEmpty ? "rotate(90deg)" : undefined }} />
              {s.empty.length} headers at 0 RPM - probably nothing plugged in
            </button>
            {showEmpty && (
              <>
                <div className="tertiary" style={{ fontSize: "var(--text-sm)", padding: "0 var(--s4)" }}>
                  {s.empty.map((f) => f.name).join(", ")}
                </div>
                <div className="fan-grid">
                  {s.empty.map((f) => (
                    <FanCard key={f.id} f={f} view={view} act={act} />
                  ))}
                </div>
              </>
            )}
          </>
        )}
      </section>
      <Group title="Temperatures" footer="Sensors: LibreHardwareMonitor with the PawnIO driver, updated every second. Speeds go as low as you set them; at 90 °C every controlled fan goes to 100 %.">
        {s.temps.map((t, i) => (
          <div className="row" key={i} style={{ minHeight: 34, padding: "6px var(--s4)" }}>
            <span className="secondary" style={{ width: 220, flexShrink: 0 }}>
              {t.hw}
            </span>
            <span className="row-main">{t.name}</span>
            <strong className={toneClass(t.tone)}>{t.value.toFixed(0)} °C</strong>
          </div>
        ))}
      </Group>
    </>
  );
}

export function Fan() {
  const { view, error, act } = useTab<FanView, FanAction>("fan");
  return (
    <Page
      title="Fan Tuner"
      subtitle="Live fan speeds and temperatures. Fans always go back to the BIOS when PC Tuner closes."
      actions={
        <Button
          variant="danger"
          icon={<RotateCcw />}
          disabled={!view || !view.can || !view.anyControlled}
          title={!view || view.can ? (view?.anyControlled ? "Every fan goes back to its BIOS / graphics-driver curve. Own settings are cleared and no profile is applied at startup until you pick one." : "No fan is controlled by PC Tuner.") : "Run PC Tuner as administrator to control fans."}
          onClick={() => act({ kind: "give_back" })}
        >
          Give all fans back to BIOS
        </Button>
      }
    >
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState title="Starting the fan helper..." />
      ) : (
        <>
          {view.viewOnly && <Banner kind="locked">Run PC Tuner as administrator to see the motherboard fans and the CPU temperature, and to control fans (PawnIO driver). Only the graphics card is visible now.</Banner>}
          {view.error && <Banner kind="error">{view.error}</Banner>}
          <Body view={view} act={act} />
        </>
      )}
    </Page>
  );
}

