// RTX 5080 Profiles: which tune is applied right now (with live graphs), the four presets, custom values and
// the safety trial. Everything is set straight through the NVIDIA driver - no MSI Afterburner needed.
import { Plus, RefreshCw, RotateCcw, SlidersHorizontal, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { OpSheet } from "../components/OpSheet";
import { Badge, Banner, Button, ContextMenu, ErrorState, Group, Hero, LoadingState, NumberField, Page, Sheet, Slider, Sparkline, Spinner, Stat, ToneBadge, ViewOnlyBanner, cx } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow, Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/gpu.css";

export type GpuClick = "Reset" | "Apply" | "Refresh" | "Keep" | "Revert" | "Dismiss" | "CloseAfterburner";
export type GpuField = "max_mhz" | "core" | "mem" | "power";

export type GpuTrial =
  | { kind: "pending"; preset: string; previous: string; appliedAt: string; unguarded: boolean; canKeep: boolean; canRevert: boolean }
  | { kind: "reverted"; preset: string; text: string; reason: string; lastError: string };

/** One live reading, shown as a tile (the page keeps which ones and their order). */
export interface GpuTile {
  key: string;
  label: string;
  unit: string;
  value: number | null;
  /** "of 16303 MB" */
  sub: string | null;
  hist: number[];
}

export interface GpuHero {
  name: string;
  unit: string;
  caption: string;
  tone: Tone;
  chips: { text: string; tone: Tone }[];
  /** the NVIDIA driver could not be read */
  error: string | null;
  live: GpuTile[] | null;
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
  /** the power slider (null until the card has been read) */
  power: { pct: number; min: number; max: number; defaultW: number; can: boolean } | null;
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
  | { kind: "power"; pct: number }
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

const TILE_KEY = "pctuner.gpu.tiles";
const DEFAULT_TILES = ["core", "temp", "power", "util", "volt", "vram"];

function loadTiles(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(TILE_KEY) ?? "null");
    if (Array.isArray(v) && v.every((x) => typeof x === "string")) return v;
  } catch {
    // no saved layout (or storage blocked): the default
  }
  return DEFAULT_TILES;
}

function saveTiles(keys: string[]) {
  try {
    localStorage.setItem(TILE_KEY, JSON.stringify(keys));
  } catch {
    // the layout just is not remembered
  }
}

/** Live readings as tiles: drag a tile to move it, the cross removes it, "Add" brings one back. */
function TelemetryTiles({ tiles }: { tiles: GpuTile[] }) {
  const [order, setOrder] = useState<string[]>(loadTiles);
  const [dragging, setDragging] = useState<string | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const byKey = new Map(tiles.map((t) => [t.key, t]));
  const shown = order.filter((k) => byKey.has(k));
  const hidden = tiles.filter((t) => !shown.includes(t.key));
  const update = (next: string[]) => {
    setOrder(next);
    saveTiles(next);
  };
  const moveTo = (key: string, target: string) => {
    if (key === target) return;
    const next = shown.filter((k) => k !== key);
    next.splice(next.indexOf(target), 0, key);
    update(next);
  };
  return (
    <div className="gpu-tiles" onDragEnd={() => (setDragging(null), setOver(null))}>
      {shown.map((k) => {
        const t = byKey.get(k)!;
        return (
          <div
            key={k}
            className={cx("gpu-tile", dragging === k && "dragging", over === k && dragging !== k && "drop-target")}
            draggable
            title="Drag to move"
            onDragStart={(e) => {
              setDragging(k);
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/plain", k);
            }}
            onDragOver={(e) => {
              e.preventDefault();
              setOver(k);
            }}
            onDrop={(e) => {
              e.preventDefault();
              const from = e.dataTransfer.getData("text/plain") || dragging;
              if (from) moveTo(from, k);
              setDragging(null);
              setOver(null);
            }}
          >
            <button type="button" className="gpu-tile-remove" title={`Remove ${t.label}`} aria-label={`Remove ${t.label}`} onClick={() => update(shown.filter((x) => x !== k))}>
              <X size={12} />
            </button>
            <Stat value={t.value === null ? "-" : t.value.toFixed(0)} unit={t.unit} label={t.sub ? `${t.label} ${t.sub}` : t.label} />
            <Sparkline values={t.hist} />
          </div>
        );
      })}
      {hidden.length > 0 && (
        <button type="button" className="gpu-tile gpu-tile-add" onClick={(e) => setMenu({ x: e.clientX, y: e.clientY })}>
          <Plus size={16} />
          Add reading
        </button>
      )}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={hidden.map((t) => ({ label: t.label, onClick: () => update([...shown, t.key]) }))} onClose={() => setMenu(null)} />}
    </div>
  );
}

/** Power limit, on its own: set at once for whatever tune is on, and kept after restarts. */
function PowerSlider({ p, act }: { p: NonNullable<View["power"]>; act: Act }) {
  const [v, setV] = useState(p.pct);
  useEffect(() => setV(p.pct), [p.pct]);
  const watts = Math.round((p.defaultW * v) / 100);
  return (
    <div className="gpu-power">
      <div className="hstack" style={{ gap: 8 }}>
        <span className="gpu-power-label">Power limit</span>
        <span className="gpu-power-value">
          {v} % <span className="secondary">({watts} W)</span>
        </span>
        <span className="spacer" />
        {v !== 100 && (
          <Button variant="plain" disabled={!p.can} onClick={() => act({ kind: "power", pct: 100 })}>
            Default (100 %)
          </Button>
        )}
      </div>
      <Slider label="Power limit" value={v} min={p.min} max={p.max} disabled={!p.can} onChange={setV} onCommit={(x) => x !== p.pct && act({ kind: "power", pct: x })} />
      <div className="tertiary gpu-small">
        {p.min} % - {p.max} % of the card's {p.defaultW} W. Set at once for the tune that is on and kept after restarts; applying a preset sets its own power limit.
      </div>
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
        {h.live && <TelemetryTiles tiles={h.live} />}
        {view.power && <PowerSlider p={view.power} act={act} />}
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
