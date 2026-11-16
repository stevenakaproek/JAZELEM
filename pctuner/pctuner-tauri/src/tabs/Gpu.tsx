// RTX 5080 Profiles: which preset is applied right now (with live graphs), the four presets and the safety trial.
import { RefreshCw, RotateCcw } from "lucide-react";
import { OpSheet } from "../components/OpSheet";
import { Badge, Banner, Button, ErrorState, Group, Hero, LoadingState, Page, Sheet, Sparkline, Spinner, Stat, ToneBadge, ViewOnlyBanner, cx } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow, Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/gpu.css";

export type GpuClick = "Reset" | "Apply" | "Restore" | "Refresh" | "Keep" | "Revert" | "Dismiss";

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
  noCfg: string | null;
  live: GpuLive | null;
}

export interface GpuPreset {
  index: number;
  name: string;
  slot: number;
  text: string;
  selected: boolean;
  current: boolean;
  needsWrite: boolean;
}

export interface View {
  trial: GpuTrial | null;
  /** null while the GPU is read for the first time */
  hero: GpuHero | null;
  canReset: boolean;
  presets: GpuPreset[];
  applyLabel: string;
  canApply: boolean;
  canRestore: boolean;
  canRefresh: boolean;
  status: string;
  error: string | null;
  op: OpWindow | null;
  opCan: boolean;
}

export type Action = { kind: "click"; click: GpuClick } | { kind: "select"; index: number } | { kind: "dismiss_error" } | { kind: "op"; ev: OpEvent };

type Act = (a: Action) => Promise<void>;

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

/** The most important thing on this screen: which preset is applied, with live graphs. */
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
        {h.noCfg && <Banner kind="error">{h.noCfg}</Banner>}
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

function PresetCard({ p, onPick }: { p: GpuPreset; onPick: () => void }) {
  return (
    <button type="button" className={cx("card clickable gpu-preset", p.selected && "selected")} aria-pressed={p.selected} onClick={onPick}>
      <span className="hstack">
        <span className="gpu-preset-name">{p.name}</span>
        <span className="spacer" />
        {p.current && <Badge color="green">current</Badge>}
      </span>
      <span className="tertiary gpu-small">Afterburner slot {p.slot}</span>
      <span className="secondary">{p.text}</span>
      {p.needsWrite && <span className="tone-warn gpu-small">Slot needs writing (backup first)</span>}
    </button>
  );
}

export function Gpu() {
  const { admin } = useShell();
  const { view, error, act } = useTab<View, Action>("gpu");
  return (
    <Page title="RTX 5080 Profiles" subtitle="Switch the card between undervolt presets stored in MSI Afterburner, and see what is applied right now.">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState title="Reading the GPU..." />
      ) : (
        <>
          <ViewOnlyBanner admin={admin} />
          {view.trial && <TrialNotice t={view.trial} act={act} />}
          <HeroCard view={view} act={act} />
          <section className="group-wrap">
            <div className="group-title">
              <span>Presets</span>
            </div>
            <div className="gpu-presets">
              {view.presets.map((p) => (
                <PresetCard key={p.index} p={p} onPick={() => act({ kind: "select", index: p.index })} />
              ))}
            </div>
          </section>
          <div className="toolbar">
            <Button variant="primary" disabled={!view.canApply} onClick={() => act({ kind: "click", click: "Apply" })}>
              {view.applyLabel}
            </Button>
            <Button disabled={!view.canRestore} onClick={() => act({ kind: "click", click: "Restore" })}>
              Restore last backup
            </Button>
            <Button icon={<RefreshCw />} disabled={!view.canRefresh} onClick={() => act({ kind: "click", click: "Refresh" })}>
              Refresh
            </Button>
            {view.status && <span className="secondary selectable">{view.status}</span>}
          </div>
          <div className="secondary gpu-small">A preset you have not kept yet starts as a trial. The automatic revert runs at your next sign-in after a crash (it cannot act during a freeze) and needs MSI Afterburner.</div>
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
