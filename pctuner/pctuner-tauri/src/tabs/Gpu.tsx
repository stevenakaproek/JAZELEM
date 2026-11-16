// RTX 5080 Profiles: which tune is applied right now (with live graphs), the four presets, custom values and
// the safety trial. Everything is set straight through the NVIDIA driver - no MSI Afterburner needed.
import { RefreshCw, RotateCcw, SlidersHorizontal, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { OpSheet } from "../components/OpSheet";
import { Badge, Banner, Button, ErrorState, Group, Hero, LoadingState, NumberField, Page, Sheet, Slider, Sparkline, Spinner, Stat, ToneBadge, ViewOnlyBanner, cx } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow, Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/gpu.css";

export type GpuClick = "Reset" | "Apply" | "Refresh" | "Keep" | "Revert" | "Dismiss" | "CloseAfterburner";
export type GpuField = "max_mhz" | "core" | "mem" | "power";

export type GpuTrial =
  | { kind: "pending"; preset: string; previous: string; appliedAt: string; unguarded: boolean; canKeep: boolean; canRevert: boolean }
  | { kind: "reverted"; preset: string; text: string; reason: string; lastError: string };

export interface GpuLive {
  core: number;
  temp: number;
  power: number;
  util: number;
  coreHist: number[];
  tempHist: number[];
  powerHist: number[];
  utilHist: number[];
}

export interface GpuHero {
  name: string;
  unit: string;
  caption: string;
  tone: Tone;
  chips: { text: string; tone: Tone }[];
  /** the NVIDIA driver could not be read */
  error: string | null;
  live: GpuLive | null;
}

export interface GpuPreset {
  index: number;
  name: string;
  /** "max 2800 MHz · core +138 · memory +1500" */
  detail: string;
  text: string;
  selected: boolean;
  current: boolean;
}

export interface GpuCustom {
  maxMhz: number;
  core: number;
  mem: number;
  power: number;
  powerWatts: number;
  limits: { maxMhz: [number, number]; core: [number, number]; mem: [number, number]; power: [number, number] };
  summary: string;
}

export interface View {
  trial: GpuTrial | null;
  /** null while the GPU is read for the first time */
  hero: GpuHero | null;
  canReset: boolean;
  presets: GpuPreset[];
  /** null until the card's limits are known */
  custom: GpuCustom | null;
  customSelected: boolean;
  customCurrent: boolean;
  afterburner: { running: boolean; autostart: boolean; canClose: boolean };
  applyLabel: string;
  canApply: boolean;
  canEdit: boolean;
  canRefresh: boolean;
  status: string;
  error: string | null;
  op: OpWindow | null;
  opCan: boolean;
}

export type Action =
  | { kind: "click"; click: GpuClick }
  | { kind: "select"; index: number }
  | { kind: "custom"; field: GpuField; value: number }
  | { kind: "dismiss_error" }
  | { kind: "op"; ev: OpEvent };

type Act = (a: Action) => Promise<void>;

/** Index of the Custom card (after the four presets). */
const CUSTOM = 6;

function TrialNotice({ t, act }: { t: GpuTrial; act: Act }) {
  if (t.kind === "pending") {
    return (
      <div className="card gpu-notice warn">
        <div className="kicker">Safety trial</div>
        <div className="gpu-notice-title">Testing {t.preset} - keep this tune?</div>
        {t.unguarded ? (
          <>
            <div className="secondary">Applied at {t.appliedAt}.</div>
            <Banner kind="error">Automatic revert is NOT armed (the sign-in check could not be registered) - revert manually if it is unstable.</Banner>
          </>
        ) : (
          <div className="secondary">Applied at {t.appliedAt}. If the PC crashes while you test it, the GPU is set back to Stock at your next sign-in.</div>
        )}
        <div className="toolbar">
          <Button variant="primary" disabled={!t.canKeep} title="Stable: make this the confirmed tune and end the trial." onClick={() => act({ kind: "click", click: "Keep" })}>
            Keep
          </Button>
          <Button disabled={!t.canRevert} title={`Go back to ${t.previous}.`} onClick={() => act({ kind: "click", click: "Revert" })}>
            Revert now
          </Button>
          <span className="tertiary gpu-small">Revert goes back to {t.previous}.</span>
        </div>
      </div>
    );
  }
  return (
    <div className="card gpu-notice error">
      <div className="kicker">Trial reverted</div>
      <div className="gpu-notice-title">Unexpected shutdown during the {t.preset} trial</div>
      <div className="secondary selectable">{t.text}</div>
      {t.reason && <div className="tertiary gpu-small selectable">Windows logged: {t.reason}</div>}
      {t.lastError && <div className="tertiary gpu-small selectable">Last sign-in check: {t.lastError}</div>}
      <div className="toolbar">
        <Button onClick={() => act({ kind: "click", click: "Dismiss" })}>Dismiss</Button>
      </div>
    </div>
  );
}

function Tile({ value, unit, label, series }: { value: number; unit: string; label: string; series: number[] }) {
  return (
    <div>
      <Stat value={value.toFixed(0)} unit={unit} label={label} />
      <Sparkline values={series} />
    </div>
  );
}

/** The most important thing on this screen: which tune is applied, with live graphs. */
function HeroCard({ view, act }: { view: View; act: Act }) {
  const h = view.hero;
  if (!h) {
    return (
      <Group title="Currently applied" padded>
        <div className="hstack">
          <Spinner />
          <span className="secondary">Reading the GPU...</span>
        </div>
      </Group>
    );
  }
  return (
    <Group title="Currently applied" padded>
      <div className="stack" style={{ gap: 14 }}>
        <div className="hstack hero-row" style={{ alignItems: "flex-start" }}>
          <Hero value={h.name} unit={h.unit} caption={h.caption} tone={h.tone} />
          <Button variant="danger" icon={<RotateCcw />} disabled={!view.canReset} title="Puts the RTX 5080 back on the Stock preset: no undervolt, no memory overclock." onClick={() => act({ kind: "click", click: "Reset" })}>
            Reset to stock
          </Button>
        </div>
        <div className="toolbar">
          {h.chips.map((c) => (
            <ToneBadge key={c.text} tone={c.tone}>
              {c.text}
            </ToneBadge>
          ))}
        </div>
        {h.error && <Banner kind="error">{h.error}</Banner>}
        {h.live && (
          <div className="gpu-tiles">
            <Tile value={h.live.core} unit="MHz" label="Core clock" series={h.live.coreHist} />
            <Tile value={h.live.temp} unit="°C" label="Temperature" series={h.live.tempHist} />
            <Tile value={h.live.power} unit="W" label="Power draw" series={h.live.powerHist} />
            <Tile value={h.live.util} unit="%" label="GPU load" series={h.live.utilHist} />
          </div>
        )}
      </div>
    </Group>
  );
}

function TuneCard({ name, detail, text, selected, current, icon, onPick }: { name: string; detail: string; text: string; selected: boolean; current: boolean; icon?: ReactNode; onPick: () => void }) {
  return (
    <button type="button" className={cx("card clickable gpu-preset", selected && "selected")} aria-pressed={selected} onClick={onPick}>
      <span className="hstack">
        {icon}
        <span className="gpu-preset-name">{name}</span>
        <span className="spacer" />
        {current && <Badge color="green">current</Badge>}
      </span>
      <span className="tertiary gpu-small">{detail}</span>
      <span className="secondary">{text}</span>
    </button>
  );
}

const sign = (v: number) => (v > 0 ? `+${v}` : `${v}`);

/** One custom value: slider plus number field. The slider shows its value while dragging and sends it on release. */
function CustomRow({ label, help, value, range, step, unit, show, disabled, onCommit }: { label: string; help: string; value: number; range: [number, number]; step: number; unit: string; show: (v: number) => string; disabled: boolean; onCommit: (v: number) => void }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  return (
    <div className="row gpu-custom-row">
      <span className="row-main">
        <div className="row-title">
          {label} <span className="gpu-custom-value">{show(v)}</span>
        </div>
        <div className="row-sub">{help}</div>
        <Slider value={v} min={range[0]} max={range[1]} step={step} disabled={disabled} label={label} onChange={setV} onCommit={onCommit} />
      </span>
      <span className="row-accessory">
        <NumberField value={value} min={range[0]} max={range[1]} disabled={disabled} suffix={unit} label={label} onCommit={onCommit} />
      </span>
    </div>
  );
}

function CustomGroup({ c, disabled, act }: { c: GpuCustom; disabled: boolean; act: Act }) {
  const set = (field: GpuField) => (value: number) => void act({ kind: "custom", field, value });
  return (
    <Group title="Custom values" footer="Applied like a preset: a safety trial first, set again at every sign-in. A lower clock limit with a higher core offset is an undervolt.">
      <CustomRow
        label="Core clock limit"
        help="The highest clock the card may run. Above it the card stays on the same voltage instead of climbing."
        value={c.maxMhz}
        range={c.limits.maxMhz}
        step={15}
        unit="MHz"
        show={(v) => `${v} MHz`}
        disabled={disabled}
        onCommit={set("max_mhz")}
      />
      <CustomRow
        label="Core offset"
        help="Moves the whole voltage/frequency curve: more MHz at every voltage. Too much crashes - raise it in small steps."
        value={c.core}
        range={c.limits.core}
        step={5}
        unit="MHz"
        show={(v) => `${sign(v)} MHz`}
        disabled={disabled}
        onCommit={set("core")}
      />
      <CustomRow
        label="Memory offset"
        help="Faster video memory, in the MHz nvidia-smi and Afterburner show. Too much shows as artefacts or crashes."
        value={c.mem}
        range={c.limits.mem}
        step={50}
        unit="MHz"
        show={(v) => `${sign(v)} MHz`}
        disabled={disabled}
        onCommit={set("mem")}
      />
      <CustomRow
        label="Power limit"
        help="How much power the card may draw, as a share of its default."
        value={c.power}
        range={c.limits.power}
        step={1}
        unit="%"
        show={(v) => `${v}% (${Math.round((c.powerWatts / Math.max(c.power, 1)) * v)} W)`}
        disabled={disabled}
        onCommit={set("power")}
      />
    </Group>
  );
}

function AfterburnerBanner({ ab, act }: { ab: View["afterburner"]; act: Act }) {
  if (!ab.running && !ab.autostart) return null;
  return (
    <>
      {ab.running && (
        <Banner
          kind="warn"
          action={
            <Button icon={<X />} disabled={!ab.canClose} onClick={() => act({ kind: "click", click: "CloseAfterburner" })}>
              Close it
            </Button>
          }
        >
          MSI Afterburner or NV-UV-Play is running. It sets the GPU too and can overwrite what PC Tuner sets - close it while you use this tab.
        </Banner>
      )}
      {ab.autostart && (
        <Banner
          kind="warn"
          action={
            <Button onClick={() => (location.hash = "#startup")} title="Switch it off there (it can be undone).">
              Open Startup Cleaner
            </Button>
          }
        >
          MSI Afterburner starts with Windows and applies its own settings. PC Tuner no longer needs it - switch it off in Startup Cleaner.
        </Banner>
      )}
    </>
  );
}

export function Gpu() {
  const { admin } = useShell();
  const { view, error, act } = useTab<View, Action>("gpu");
  return (
    <Page title="RTX 5080 Profiles" subtitle="Undervolt presets or your own values, set straight through the NVIDIA driver, and what is applied right now.">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState title="Reading the GPU..." />
      ) : (
        <>
          <ViewOnlyBanner admin={admin} />
          <AfterburnerBanner ab={view.afterburner} act={act} />
          {view.trial && <TrialNotice t={view.trial} act={act} />}
          <HeroCard view={view} act={act} />
          <section className="group-wrap">
            <div className="group-title">
              <span>Presets</span>
            </div>
            <div className="gpu-presets">
              {view.presets.map((p) => (
                <TuneCard key={p.index} name={p.name} detail={p.detail} text={p.text} selected={p.selected} current={p.current} onPick={() => act({ kind: "select", index: p.index })} />
              ))}
              <TuneCard
                name="Custom"
                icon={<SlidersHorizontal size={16} />}
                detail={view.custom?.summary ?? "your own values"}
                text="Your own clock limit, core and memory offsets and power limit."
                selected={view.customSelected}
                current={view.customCurrent}
                onPick={() => act({ kind: "select", index: CUSTOM })}
              />
            </div>
          </section>
          {view.customSelected && view.custom && <CustomGroup c={view.custom} disabled={!view.canEdit} act={act} />}
          <div className="toolbar">
            <Button variant="primary" disabled={!view.canApply} onClick={() => act({ kind: "click", click: "Apply" })}>
              {view.applyLabel}
            </Button>
            <Button icon={<RefreshCw />} disabled={!view.canRefresh} onClick={() => act({ kind: "click", click: "Refresh" })}>
              Refresh
            </Button>
            {view.status && <span className="secondary selectable">{view.status}</span>}
          </div>
          <div className="secondary gpu-small">
            A tune you have not kept yet starts as a trial. The automatic revert runs at your next sign-in after a crash (it cannot act during a freeze). The driver forgets these settings at every restart, so PC Tuner sets them again 30 seconds after you sign in.
          </div>
          {view.op && <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} />}
          {view.error && (
            <Sheet
              title="RTX 5080 Profiles"
              subtitle={<span className="gpu-message selectable">{view.error}</span>}
              onClose={() => act({ kind: "dismiss_error" })}
              footer={
                <Button variant="primary" onClick={() => act({ kind: "dismiss_error" })}>
                  OK
                </Button>
              }
            />
          )}
        </>
      )}
    </Page>
  );
}
