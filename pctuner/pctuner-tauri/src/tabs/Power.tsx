// The Power sheet: restart into the BIOS, or shut down so the next start opens the BIOS.
import { ArrowDownToLine, RotateCcw } from "lucide-react";
import { useEffect } from "react";
import { Banner, Button, Sheet, Spinner } from "../components/ui";
import { useTab } from "../lib/useTab";

export type PowerChoice = "RestartToBios" | "ShutdownToBios";

export interface PowerView {
  stage: { kind: "choose" } | { kind: "working"; title: string } | { kind: "timed_out"; title: string };
  choices: { id: PowerChoice; title: string; text: string }[];
  blocked: string | null;
  canPick: boolean;
  checking: boolean;
  error: string | null;
}

export type PowerAction = { kind: "open" } | { kind: "pick"; choice: PowerChoice } | { kind: "back" };

export function PowerSheet({ onClose }: { onClose: () => void }) {
  const { view, act } = useTab<PowerView, PowerAction>("power");
  useEffect(() => {
    void act({ kind: "open" });
  }, [act]);
  if (!view) return null;

  if (view.stage.kind === "working") {
    return (
      <Sheet
        kicker="Power"
        title={
          <span className="hstack" style={{ gap: 10 }}>
            <Spinner large />
            {view.stage.title}
          </span>
        }
        subtitle="Windows is closing your programs."
      />
    );
  }

  if (view.stage.kind === "timed_out") {
    return (
      <Sheet
        kicker="Power"
        title={view.stage.title}
        subtitle="The restart or shut down was cancelled or blocked (for example by a program that would not close)."
        onClose={onClose}
        footer={
          <>
            <Button variant="primary" size="lg" onClick={onClose}>
              Close
            </Button>
            <Button size="lg" onClick={() => act({ kind: "back" })}>
              Back
            </Button>
          </>
        }
      >
        <Banner kind="warn">Windows may still remember the BIOS request: the next restart can open the BIOS.</Banner>
      </Sheet>
    );
  }

  return (
    <Sheet
      kicker="Power"
      title="Go into the BIOS"
      subtitle="No need to press Del at the right moment - Windows tells the motherboard to open its setup."
      onClose={onClose}
      footer={
        <>
          <Button size="lg" onClick={onClose}>
            Cancel
          </Button>
          {view.checking && (
            <span className="hstack secondary">
              <Spinner />
              Checking the firmware type...
            </span>
          )}
        </>
      }
    >
      {view.error && <Banner kind="error">{view.error}</Banner>}
      {view.blocked && <Banner kind="locked">{view.blocked}</Banner>}
      {view.choices.map((c) => (
        <button key={c.id} type="button" className="choice" disabled={!view.canPick} onClick={() => act({ kind: "pick", choice: c.id })}>
          {c.id === "RestartToBios" ? <RotateCcw /> : <ArrowDownToLine />}
          <span>
            <div className="row-title">{c.title}</div>
            <div className="row-sub">{c.text}</div>
          </span>
        </button>
      ))}
      <p className="tone-warn" style={{ fontSize: "var(--text-sm)" }}>
        Clicking one acts at once: every open program closes without saving.
      </p>
    </Sheet>
  );
}
