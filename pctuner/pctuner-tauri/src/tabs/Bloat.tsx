// Remove Bloat: tick apps by hand, one button removes them. Results and the log live in the operation sheet.
import { RefreshCw } from "lucide-react";
import { OpSheet } from "../components/OpSheet";
import { Badge, Button, ErrorState, Group, LoadingState, Page, Row, Sheet, Spinner, ViewOnlyBanner } from "../components/ui";
import { useShell } from "../lib/shell";
import type { OpEvent, OpWindow } from "../lib/types";
import { useTab } from "../lib/useTab";

export interface BloatRow {
  index: number;
  name: string;
  tip: string;
  optional: boolean;
  /** true = installed, false = removed, null = not known (still scanning, or not a Store app). */
  installed: boolean | null;
  picked: boolean;
}

export interface View {
  scanning: boolean;
  apps: number;
  gone: number | null;
  rows: BloatRow[];
  selected: number;
  canRemove: boolean;
  removeTip: string;
  canClear: boolean;
  canRescan: boolean;
  op: OpWindow | null;
  opCan: boolean;
  log: string | null;
}

export type Action = { kind: "toggle"; index: number } | { kind: "clear" | "remove" | "rescan" | "close_log" } | { kind: "op"; ev: OpEvent };

function AppRow({ r, onToggle }: { r: BloatRow; onToggle: () => void }) {
  return (
    <Row
      checked={r.picked}
      onClick={onToggle}
      title={r.name}
      title2={r.tip}
      accessory={
        <>
          {r.optional && <Badge>optional</Badge>}
          {r.installed === true && <Badge color="orange">installed</Badge>}
          {r.installed === false && <Badge color="green">removed</Badge>}
        </>
      }
    />
  );
}

export function Bloat() {
  const { admin } = useShell();
  const { view, error, act } = useTab<View, Action>("bloat");
  const safe = view?.rows.filter((r) => !r.optional) ?? [];
  const optional = view?.rows.filter((r) => r.optional) ?? [];
  return (
    <Page title="Remove Bloat" subtitle="Removes apps in depth (all users, leftovers, reinstall blocks). Never touches Xbox, Game Pass, Store, Security or Edge.">
      {error ? (
        <ErrorState text={error} />
      ) : !view ? (
        <LoadingState title="Reading the installed apps" />
      ) : (
        <>
          <ViewOnlyBanner admin={admin} />
          <div className="toolbar">
            <Button variant="primary" disabled={!view.canRemove} title={view.removeTip} onClick={() => act({ kind: "remove" })}>
              Remove selected ({view.selected})
            </Button>
            {view.selected > 0 && (
              <Button disabled={!view.canClear} title="Untick everything." onClick={() => act({ kind: "clear" })}>
                Clear selection
              </Button>
            )}
            <span className="spacer" />
            <span className="secondary" title="Store apps from the Safe and Optional lists that are gone from this PC">
              {view.gone === null ? "Reading the installed apps" : `${view.gone} of ${view.apps} apps removed`}
            </span>
            <Button icon={view.scanning ? <Spinner /> : <RefreshCw />} disabled={!view.canRescan || view.scanning} title="Read the installed apps again." onClick={() => act({ kind: "rescan" })}>
              Rescan
            </Button>
          </div>
          <Group title="Targets" count="tick the apps you want removed">
            {safe.map((r) => (
              <AppRow key={r.index} r={r} onToggle={() => act({ kind: "toggle", index: r.index })} />
            ))}
          </Group>
          <Group title="Optional - each costs a convenience">
            {optional.map((r) => (
              <AppRow key={r.index} r={r} onToggle={() => act({ kind: "toggle", index: r.index })} />
            ))}
          </Group>
          {view.log !== null ? (
            <Sheet
              wide
              title="Technical details"
              onClose={() => act({ kind: "close_log" })}
              footer={
                <Button variant="primary" size="lg" onClick={() => act({ kind: "close_log" })}>
                  OK
                </Button>
              }
            >
              <pre className="mono secondary selectable" style={{ margin: 0, whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
                {view.log}
              </pre>
            </Sheet>
          ) : (
            view.op && <OpSheet op={view.op} can={view.opCan} admin={admin} onEvent={(ev) => act({ kind: "op", ev })} />
          )}
        </>
      )}
    </Page>
  );
}
