// RAM LED: switch the lights on the memory sticks off or on, or set one colour - and the same for the
// RTX 5080's own LEDs in a second section.
import { RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Banner, Button, ErrorState, Group, Hero, LoadingState, Page, Row, Spinner, Stat, Switch, ViewOnlyBanner } from "../components/ui";
import { useShell } from "../lib/shell";
import type { Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/ramled.css";

export interface LedSection {
  hero: { value: string; caption: string; tone: Tone };
  busy: boolean;
  found: boolean;
  lit: boolean;
  count: number;
  canRescan: boolean;
  error: string | null;
  can: boolean;
  canColor: boolean;
  colorUnsupported: boolean;
  /** "#rrggbb" */
  color: string;
  remember: boolean;
  status: string;
  sticks: { title: string; state: string; lit: boolean; detail: string }[];
}

/** The memory sticks, plus the graphics card in `gpu`. */
export interface RamledView extends LedSection {
  gpu: LedSection;
}

export type LedTarget = "ram" | "gpu";

export type RamledAction = ({ kind: "rescan" | "off" | "on" | "rainbow" | "set_color" } | { kind: "pick"; color: string } | { kind: "remember"; on: boolean }) & { target?: LedTarget };

type Act = (a: RamledAction) => Promise<void>;

const TEXT = {
  ram: {
    unit: "RAM LEDs",
    count: "sticks with LEDs",
    remember: "Remember after restart (saves the setting in the RAM)",
    rememberTip: "On: the sticks keep the setting through power-off, with nothing running. Off: it lasts until the next power-on.",
    off: "All RAM LEDs dark.",
    on: "Back to the last lighting the sticks had (rainbow if PC Tuner has not seen them lit).",
    list: "Sticks",
  },
  gpu: {
    unit: "RTX 5080 LEDs",
    count: "LED controller",
    remember: "Remember after restart (saves the setting in the graphics card)",
    rememberTip: "On: the card keeps the setting through power-off, with nothing running. Off: it lasts until the next power-on.",
    off: "All of the graphics card's LEDs dark.",
    on: "Back to the last lighting PC Tuner set on the card (rainbow if none).",
    list: "Graphics card",
  },
};

function Section({ view, target, act }: { view: LedSection; target: LedTarget; act: Act }) {
  const t = TEXT[target];
  const send = (a: RamledAction) => act({ ...a, target });
  const [color, setColor] = useState(view.color);
  useEffect(() => setColor(view.color), [view.color]);
  return (
    <>
      <Group padded>
        <div className="hstack hero-row">
          <Hero value={view.hero.value} unit={t.unit} caption={view.hero.caption} tone={view.hero.tone} />
          {view.found && target === "ram" && <Stat value={view.count} label={t.count} />}
          <span className="spacer" />
          {view.busy && <Spinner />}
          <Button icon={<RefreshCw />} disabled={!view.canRescan} title="Look for the LED controllers again. Changes nothing." onClick={() => send({ kind: "rescan" })}>
            Rescan
          </Button>
        </div>
      </Group>
      {view.error && <Banner kind="warn">{view.error}</Banner>}
      <Group title="Lighting">
        <div className="row">
          <div className="row-main toolbar">
            <Button variant={view.lit ? "primary" : "default"} disabled={!view.can} title={t.off} onClick={() => send({ kind: "off" })}>
              Turn off
            </Button>
            <Button variant={view.lit ? "default" : "primary"} disabled={!view.can} title={t.on} onClick={() => send({ kind: "on" })}>
              Turn on
            </Button>
            <span className="ramled-divider" />
            <Button disabled={!view.can} title="The moving rainbow effect." onClick={() => send({ kind: "rainbow" })}>
              Rainbow
            </Button>
            <span className="ramled-divider" />
            <input
              className="ramled-color"
              type="color"
              aria-label={`${t.unit} colour`}
              value={color}
              disabled={!view.canColor}
              onChange={(e) => {
                setColor(e.target.value);
                void send({ kind: "pick", color: e.target.value });
              }}
            />
            <Button disabled={!view.canColor} title="One fixed colour on every LED." onClick={() => send({ kind: "set_color" })}>
              Set colour
            </Button>
            {view.colorUnsupported && <span className="tertiary">Colour is not supported on this LED controller version.</span>}
          </div>
        </div>
        <Row title={t.remember} title2={t.rememberTip} accessory={<Switch on={view.remember} label={t.remember} onChange={(on) => send({ kind: "remember", on })} />} />
        {view.status && <Row title={<span className="secondary">{view.status}</span>} />}
      </Group>
      {view.found && (
        <Group title={t.list}>
          {view.sticks.map((s) => (
            <Row key={s.title} title={s.title} subtitle={s.detail} accessory={<Badge color={s.lit ? "accent" : ""}>{s.state}</Badge>} />
          ))}
        </Group>
      )}
    </>
  );
}

export function Ramled() {
  const { admin } = useShell();
  const { view, error, act } = useTab<RamledView, RamledAction>("ramled");
  return (
    <Page title="RAM LED" subtitle="Switch the lights on your memory sticks and your RTX 5080 off or on, or pick a colour - no vendor software needed.">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState />
      ) : (
        <>
          <ViewOnlyBanner admin={admin} />
          <h2 className="ramled-heading">Memory</h2>
          <Section view={view} target="ram" act={act} />
          <div className="group-footer">Works without G.SKILL Trident Z Lighting Control. If that program is running, close it: it can overwrite these settings.</div>
          <h2 className="ramled-heading">RTX 5080</h2>
          <Section view={view.gpu} target="gpu" act={act} />
          <div className="group-footer">
            Works without Gigabyte Control Center / RGB Fusion. If it is running, close it: it can overwrite these settings. The card's controller cannot be read back, so PC Tuner shows the lighting it set last.
          </div>
        </>
      )}
    </Page>
  );
}
