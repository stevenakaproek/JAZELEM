// USB Port Finder: which USB controller every device is plugged into, the live plug-in test, and switching an empty
// USB controller off (and back on).
import { Power, PowerOff, RefreshCw } from "lucide-react";
import { useState } from "react";
import { Badge, Button, ErrorState, Sheet, Group, Hero, LoadingState, Page, Progress, Spinner, Stat, ToneBadge, cx } from "../components/ui";
import type { Tone } from "../lib/types";
import { toneBadge, toneClass } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/usb.css";

export interface UsbSummary {
  hero: { value: string; unit: string; caption: string; tone: Tone };
  stats: { value: string; label: string; tone: Tone }[];
}

export interface UsbController {
  id: string;
  tier: string;
  tone: Tone;
  name: string;
  router: boolean;
  note: string;
  empty: boolean;
  disabled: boolean;
  /** an empty controller that may be switched off (administrator, nothing running) */
  canDisable: boolean;
  canEnable: boolean;
  /** why it cannot be switched off */
  whyNot: string | null;
  hover: string;
}

export interface UsbMap {
  time: string;
  key: { tier: string; tone: Tone; name: string; verdict: string }[];
  controllers: UsbController[];
  devices: { tier: string; tone: Tone; name: string; kind: string; keyKind: boolean; hubs: string | null; onboard: boolean; where: string; hover: string }[];
}

export interface UsbHit {
  n: number;
  name: string;
  tier: string;
  tone: Tone;
  detail: string;
  fresh: boolean;
}

export interface View {
  scanning: boolean;
  scanSecs: number;
  error: string | null;
  summary: UsbSummary | null;
  map: UsbMap | null;
  /** the plug-in test's status line while it runs */
  watch: string | null;
  tested: string | null;
  watchError: string | null;
  log: UsbHit[];
  /** "Disabling AMD USB ..." while pnputil runs */
  switching: string | null;
  switchNote: { ok: boolean; text: string } | null;
  admin: boolean;
}

export type Action = { kind: "rescan" | "start_watch" | "stop_watch" | "dismiss_note" } | { kind: "controller"; id: string; enable: boolean };

type Act = (a: Action) => Promise<void>;

/** Hero card: the tier your controller and headset sit on. */
function HeroCard({ view }: { view: View }) {
  const s = view.summary;
  if (!s) {
    return (
      <Group padded>
        {view.error ? (
          <div className="stack" style={{ gap: 6 }}>
            <Hero value="?" unit="controller and headset" caption="The USB tree could not be read" tone="Error" />
            <div className="tone-error selectable">{view.error}</div>
          </div>
        ) : (
          <div className="stack">
            <Hero value="..." unit="controller and headset" caption={`Reading the USB tree... ${view.scanSecs} s (usually a few seconds)`} tone="Faint" />
            <Progress value={null} />
          </div>
        )}
      </Group>
    );
  }
  return (
    <Group padded>
      <div className="hstack hero-row" style={{ flexWrap: "wrap" }}>
        <Hero value={s.hero.value} unit={s.hero.unit} caption={s.hero.caption} tone={s.hero.tone} />
        {s.stats.map((st, i) => (
          <Stat key={i} value={st.value} label={st.label} tone={st.tone} />
        ))}
      </div>
    </Group>
  );
}

/** Plug-in test: instructions, start / stop, and one highlighted row per port tried. */
function PlugInTest({ view, act }: { view: View; act: Act }) {
  const watching = view.watch !== null;
  return (
    <Group title="Plug-in test" padded>
      <div className="stack">
        <div className="secondary">Find out which physical socket is which tier: plug any USB device (a flash drive or your controller) into a port, wait until its row appears below, then move it to the next port.</div>
        <div className="toolbar">
          {watching ? (
            <Button onClick={() => act({ kind: "stop_watch" })}>Stop plug-in test</Button>
          ) : (
            <Button variant="primary" onClick={() => act({ kind: "start_watch" })}>
              Start plug-in test
            </Button>
          )}
          {watching ? (
            <>
              <Spinner />
              <span className="secondary">{view.watch}</span>
            </>
          ) : (
            view.tested && <span className="secondary">{view.tested}</span>
          )}
        </div>
        {view.watchError && <div className="tone-error selectable">{view.watchError}</div>}
        {view.log.map((l) => (
          <div key={l.n} className={cx("usb-hit", l.fresh && "fresh", l.fresh && toneBadge(l.tone))}>
            <div className="hstack">
              <strong className="secondary">#{l.n}</strong>
              <strong>{l.name}</strong>
              <span className="spacer" />
              <strong className={toneClass(l.tone)}>{l.tier}</strong>
            </div>
            <div className="secondary usb-small">{l.detail}</div>
          </div>
        ))}
      </div>
    </Group>
  );
}

function TierChip({ tier, tone, wide }: { tier: string; tone: Tone; wide?: boolean }) {
  return (
    <span className={cx("usb-tier", wide && "wide")}>
      <ToneBadge tone={tone}>{tier}</ToneBadge>
    </span>
  );
}

function MapCards({ m, view, act }: { m: UsbMap; view: View; act: Act }) {
  const [confirm, setConfirm] = useState<UsbController | null>(null);
  return (
    <>
      <Group title="Your game controller and audio">
        {m.key.length === 0 && (
          <div className="row">
            <span className="row-main secondary">None found on USB. (A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.)</span>
          </div>
        )}
        {m.key.map((d, i) => (
          <div className="row" key={i}>
            <TierChip tier={d.tier} tone={d.tone} />
            <span className="row-main">
              <div className="row-title">{d.name}</div>
              <div className="row-sub">{d.verdict}</div>
            </span>
          </div>
        ))}
      </Group>
      <Group
        title="USB controllers"
        count="closest to the CPU first"
        footer="A controller with nothing plugged in can be switched off (Disable) and back on (Enable). Its USB ports do not work while it is off."
      >
        {view.switching && (
          <div className="row">
            <Spinner />
            <span className="row-main secondary">{view.switching}</span>
          </div>
        )}
        {view.switchNote && (
          <div className="row">
            <span className={cx("row-main", view.switchNote.ok ? "tone-ok" : "tone-error", "selectable")}>{view.switchNote.text}</span>
            <Button variant="plain" onClick={() => act({ kind: "dismiss_note" })}>
              OK
            </Button>
          </div>
        )}
        {m.controllers.map((c, i) => (
          <div className={cx("row", c.disabled && "usb-off")} key={i} title={c.hover}>
            <TierChip tier={c.tier} tone={c.tone} wide />
            <span className="row-main">
              <div className={cx("row-title", (c.router || c.disabled) && "secondary")}>{c.name}</div>
              {c.router && <div className="row-sub">{c.note}</div>}
            </span>
            {c.disabled ? <Badge color="orange">disabled</Badge> : !c.router && <span className={cx("row-value", c.empty && "tertiary")}>{c.note}</span>}
            {c.disabled ? (
              <Button icon={<Power />} disabled={!c.canEnable} title={view.admin ? "Switch this controller back on." : "Run PC Tuner as administrator."} onClick={() => act({ kind: "controller", id: c.id, enable: true })}>
                Enable
              </Button>
            ) : (
              c.empty &&
              !c.router && (
                <Button icon={<PowerOff />} disabled={!c.canDisable} title={!view.admin ? "Run PC Tuner as administrator." : (c.whyNot ?? "Switch this empty controller off.")} onClick={() => setConfirm(c)}>
                  Disable
                </Button>
              )
            )}
          </div>
        ))}
      </Group>
      {confirm && (
        <Sheet
          kicker="USB controller"
          title={`Disable ${confirm.name}?`}
          subtitle="Nothing is plugged into it right now. While it is off, every USB port it drives stops working - something you plug in there later will not be seen until you enable it again here (or in Device Manager)."
          onClose={() => setConfirm(null)}
          footer={
            <>
              <Button
                variant="primary"
                size="lg"
                onClick={() => {
                  void act({ kind: "controller", id: confirm.id, enable: false });
                  setConfirm(null);
                }}
              >
                Disable
              </Button>
              <Button size="lg" onClick={() => setConfirm(null)}>
                Cancel
              </Button>
            </>
          }
        />
      )}
      <Group
        title="Connected devices"
        footer="Good to know: the latency difference between tiers is a few microseconds. What matters more is plugging straight into the PC (no hub, front panel or monitor port) and not sharing a port group with a busy webcam or drive."
      >
        {m.devices.map((d, i) => (
          <div className="row" key={i} title={d.hover}>
            <TierChip tier={d.tier} tone={d.tone} />
            <span className="row-main">
              <div className="row-title hstack usb-row-main">
                {d.name}
                {d.kind && <Badge color={d.keyKind ? "accent" : ""}>{d.kind}</Badge>}
                {d.hubs && <Badge color="orange">{d.hubs}</Badge>}
                {d.onboard && <Badge>built in - cannot be moved</Badge>}
              </div>
              <div className="row-sub">{d.where}</div>
            </span>
          </div>
        ))}
      </Group>
    </>
  );
}

export function Usb() {
  const { view, error, act } = useTab<View, Action>("usb");
  return (
    <Page title="USB Port Finder" subtitle="Which USB controller every device is plugged into, ranked by how close it sits to the CPU. Empty controllers can be switched off.">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState title="Reading the USB tree..." />
      ) : (
        <>
          <HeroCard view={view} />
          <div className="toolbar">
            <Button icon={<RefreshCw />} disabled={view.scanning} onClick={() => act({ kind: "rescan" })}>
              Rescan
            </Button>
            {view.scanning
              ? view.map && (
                  <>
                    {/* the first read shows its progress in the hero card */}
                    <span className="secondary">Reading the USB tree...</span>
                    <Spinner />
                  </>
                )
              : view.map && <span className="secondary">read {view.map.time}</span>}
          </div>
          <PlugInTest view={view} act={act} />
          {view.map && <MapCards m={view.map} view={view} act={act} />}
        </>
      )}
    </Page>
  );
}
