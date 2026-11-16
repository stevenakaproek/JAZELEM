// RAM LED: switch the lights on the memory sticks off or on, or set one colour.
import { RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Banner, Button, ErrorState, Group, Hero, LoadingState, Page, Row, Spinner, Stat, Switch, ViewOnlyBanner } from "../components/ui";
import { useShell } from "../lib/shell";
import type { Tone } from "../lib/types";
import { useTab } from "../lib/useTab";
import "../styles/ramled.css";

export interface RamledView {
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

export type RamledAction = { kind: "rescan" | "off" | "on" | "rainbow" | "set_color" } | { kind: "pick"; color: string } | { kind: "remember"; on: boolean };

const REMEMBER_TIP = "On: the sticks keep the setting through power-off, with nothing running. Off: it lasts until the next power-on.";

function Body({ view, act }: { view: RamledView; act: (a: RamledAction) => Promise<void> }) {
  const [color, setColor] = useState(view.color);
  useEffect(() => setColor(view.color), [view.color]);
  return (
    <>
      <Group padded>
        <div className="hstack hero-row">
          <Hero value={view.hero.value} unit="RAM LEDs" caption={view.hero.caption} tone={view.hero.tone} />
          {view.found && <Stat value={view.count} label="sticks with LEDs" />}
          <span className="spacer" />
          {view.busy && <Spinner />}
          <Button icon={<RefreshCw />} disabled={!view.canRescan} title="Read the LED controllers again. Changes nothing." onClick={() => act({ kind: "rescan" })}>
            Rescan
          </Button>
        </div>
      </Group>
      {view.error && <Banner kind="warn">{view.error}</Banner>}
      <Group title="Lighting">
        <div className="row">
          <div className="row-main toolbar">
            <Button variant={view.lit ? "primary" : "default"} disabled={!view.can} title="All RAM LEDs dark." onClick={() => act({ kind: "off" })}>
              Turn off
            </Button>
            <Button variant={view.lit ? "default" : "primary"} disabled={!view.can} title="Back to the last lighting the sticks had (rainbow if PC Tuner has not seen them lit)." onClick={() => act({ kind: "on" })}>
              Turn on
            </Button>
            <span className="ramled-divider" />
            <Button disabled={!view.can} title="The moving rainbow effect." onClick={() => act({ kind: "rainbow" })}>
              Rainbow
            </Button>
            <span className="ramled-divider" />
            <input
              className="ramled-color"
              type="color"
              aria-label="Colour"
              value={color}
              disabled={!view.canColor}
              onChange={(e) => {
                setColor(e.target.value);
                void act({ kind: "pick", color: e.target.value });
              }}
            />
            <Button disabled={!view.canColor} title="One fixed colour on every LED." onClick={() => act({ kind: "set_color" })}>
              Set colour
            </Button>
            {view.colorUnsupported && <span className="tertiary">Colour is not supported on this LED controller version.</span>}
          </div>
        </div>
        <Row title="Remember after restart (saves the setting in the RAM)" title2={REMEMBER_TIP} accessory={<Switch on={view.remember} label="Remember after restart (saves the setting in the RAM)" onChange={(on) => act({ kind: "remember", on })} />} />
        {view.status && <Row title={<span className="secondary">{view.status}</span>} />}
      </Group>
      {view.found && (
        <Group title="Sticks">
          {view.sticks.map((s) => (
            <Row key={s.title} title={s.title} subtitle={s.detail} accessory={<Badge color={s.lit ? "accent" : ""}>{s.state}</Badge>} />
          ))}
        </Group>
      )}
      <div className="group-footer">Works without G.SKILL Trident Z Lighting Control. If that program is running, close it: it can overwrite these settings.</div>
    </>
  );
}

export function Ramled() {
  const { admin } = useShell();
  const { view, error, act } = useTab<RamledView, RamledAction>("ramled");
  return (
    <Page title="RAM LED" subtitle="Switch the lights on your memory sticks off or on, or pick a colour - no vendor software needed.">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState />
      ) : (
        <>
          <ViewOnlyBanner admin={admin} />
          <Body view={view} act={act} />
        </>
      )}
    </Page>
  );
}
