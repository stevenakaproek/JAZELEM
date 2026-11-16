// Fan Tuner: live speeds and temperatures, and each fan's BIOS / Manual / Curve / Trigger setting.
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
  chip: { text: string; tone: Tone };
  hist: number[];
  floor: number;
  setting: FanSetting;
  notice: string | null;
  /** Silent / Balanced / Performance for this fan; `active` when its curve is exactly that preset */
  presets: { name: string; active: boolean }[];
  live: number | null;
  liveDuty: number | null;
  afterburner: boolean;
  /** speed sensor pairing and calibration (null for read-only fans and pumps) */
  tools: FanTools | null;
}

export interface FanTools {
  sensors: { id: string; label: string }[];
  /** the sensor this fan's RPM comes from ("none") */
  sensor: string;
  /** chosen by hand or by Detect (not the built-in pairing) */
  manual: boolean;
  calib: { summary: string; points: [number, number][]; when: string } | null;
  test: { kind: "calibrate" | "pair"; step: number; steps: number; duty: number | null } | null;
  otherRunning: boolean;
  result: { ok: boolean; text: string } | null;
  canTest: boolean;
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
  note: string | null;
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
  | { kind: "give_back" }
  | { kind: "dismiss"; seq: number }
  | { kind: "preset"; id: string; name: string }
  | { kind: "mode"; id: string; mode: FanSetting["mode"] }
  | { kind: "manual"; id: string; percent: number }
  | { kind: "source"; id: string; source: FanSource }
  | { kind: "curve_point"; id: string; index: number; t: number; d: number }
  | { kind: "curve_add"; id: string; t: number; d: number }
  | { kind: "curve_remove"; id: string; index: number }
  | { kind: "trigger"; id: string; idle_temp: number; load_temp: number; idle: number; load: number }
  | { kind: "pump"; id: string; on: boolean }
  | { kind: "pair_sensor"; id: string; sensor: string }
  | { kind: "test"; id: string; test: "calibrate" | "pair" }
  | { kind: "cancel_test" };

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

/** The calibration result: RPM at each duty, 0-100 %. */
function CalibChart({ points }: { points: [number, number][] }) {
  const max = Math.max(1, ...points.map((p) => p[1]));
  const W = 260,
    H = 70;
  const sorted = [...points].sort((a, b) => a[0] - b[0]);
  const xy = (p: [number, number]) => `${(p[0] / 100) * W},${H - (p[1] / max) * (H - 6)}`;
  return (
    <svg className="fan-calib" viewBox={`0 0 ${W} ${H}`} role="img" aria-label="RPM at each duty">
      <polyline points={sorted.map(xy).join(" ")} fill="none" stroke="var(--accent)" strokeWidth="2" />
      {sorted.map((p) => {
        const [x, y] = xy(p).split(",").map(Number);
        return <circle key={p[0]} cx={x} cy={y} r="2.5" fill={p[1] > 0 ? "var(--accent)" : "var(--text-tertiary)"}><title>{`${p[0]} % - ${p[1].toFixed(0)} RPM`}</title></circle>;
      })}
    </svg>
  );
}

/** Which speed sensor this fan is read from, and its calibration (duty sweep). */
function FanToolsPanel({ f, t, disabled, act }: { f: FanCardView; t: FanTools; disabled: boolean; act: Act }) {
  const [open, setOpen] = useState(false);
  const busy = t.test !== null;
  return (
    <div className="fan-tools">
      <button type="button" className="fan-tools-head" aria-expanded={open || busy} onClick={() => setOpen(!open)}>
        <ChevronRight size={14} className={cx("fan-tools-chev", (open || busy) && "open")} />
        Speed sensor &amp; calibration
        <span className="spacer" />
        {t.calib ? <span className="tertiary">calibrated</span> : <span className="tertiary">not calibrated</span>}
      </button>
      {(open || busy) && (
        <div className="stack" style={{ gap: 8 }}>
          <label className="stack" style={{ gap: 4 }}>
            <span className="secondary" style={{ fontSize: "var(--text-sm)" }}>Speed sensor (where the RPM above comes from)</span>
            <span className="hstack" style={{ gap: 8 }}>
              <select className="input select" style={{ flex: 1, minWidth: 0 }} value={t.sensor} disabled={disabled || busy || t.otherRunning} aria-label={`${f.name} speed sensor`} onChange={(e) => act({ kind: "pair_sensor", id: f.id, sensor: e.target.value })}>
                {t.sensors.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label}
                  </option>
                ))}
              </select>
              <Button disabled={!t.canTest} title="Runs this fan at 30 %, then 100 % (about 12 seconds) and pairs the speed sensor that follows it." onClick={() => act({ kind: "test", id: f.id, test: "pair" })}>
                Detect
              </Button>
              {t.manual && (
                <Button variant="plain" disabled={disabled || busy} title="Back to the pairing the board reports." onClick={() => act({ kind: "pair_sensor", id: f.id, sensor: "auto" })}>
                  Use default
                </Button>
              )}
            </span>
          </label>
          <div className="hstack" style={{ gap: 8, flexWrap: "wrap" }}>
            <Button disabled={!t.canTest || t.sensor === "none"} title={t.sensor === "none" ? "Pair a speed sensor first." : "Steps this fan from 100 % down to 0 % and back up (about 1-2 minutes), recording the RPM. Stops at once if anything reaches 90 °C."} onClick={() => act({ kind: "test", id: f.id, test: "calibrate" })}>
              {t.calib ? "Calibrate again" : "Calibrate"}
            </Button>
            {t.otherRunning && <span className="tertiary" style={{ fontSize: "var(--text-sm)" }}>Another fan is being tested.</span>}
          </div>
          {t.test && (
            <div className="stack" style={{ gap: 4 }}>
              <div className="hstack" style={{ gap: 8 }}>
                <span className="secondary" style={{ fontSize: "var(--text-sm)" }}>
                  {t.test.kind === "pair" ? "Detecting the speed sensor" : "Calibrating"} - step {t.test.step + 1} of {t.test.steps}
                  {t.test.duty !== null ? `, running at ${t.test.duty.toFixed(0)} %` : ""}
                </span>
                <span className="spacer" />
                <Button variant="plain" onClick={() => act({ kind: "cancel_test" })}>
                  Cancel
                </Button>
              </div>
              <Progress value={t.test.steps ? t.test.step / t.test.steps : null} />
            </div>
          )}
          {t.result && !busy && <div className={t.result.ok ? "tone-ok" : "tone-warn"} style={{ fontSize: "var(--text-sm)" }}>{t.result.text}</div>}
          {t.calib && (
            <div className="stack" style={{ gap: 4 }}>
              <div style={{ fontSize: "var(--text-sm)" }}>{t.calib.summary}</div>
              <CalibChart points={t.calib.points} />
              <div className="tertiary" style={{ fontSize: "var(--text-xs)" }}>Measured {t.calib.when}</div>
            </div>
          )}
        </div>
      )}
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
          {f.presets.length > 0 && (
            <div className="hstack fan-presets">
              <span className="secondary" style={{ fontSize: "var(--text-sm)" }}>Preset</span>
              {f.presets.map((p) => (
                <button key={p.name} type="button" className={cx("pill", p.active && "on")} disabled={disabled} title={`Sets this fan to the ${p.name} curve now.`} onClick={() => act({ kind: "preset", id: f.id, name: p.name })}>
                  {p.name}
                </button>
              ))}
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
          {f.tools && <FanToolsPanel f={f} t={f.tools} disabled={disabled} act={act} />}
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
      {view.note && <div className="secondary selectable">{view.note}</div>}
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
          title={!view || view.can ? (view?.anyControlled ? "Every fan goes back to its BIOS / graphics-driver curve. Their settings here are cleared, so nothing is applied at startup until you set a fan again." : "No fan is controlled by PC Tuner.") : "Run PC Tuner as administrator to control fans."}
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

