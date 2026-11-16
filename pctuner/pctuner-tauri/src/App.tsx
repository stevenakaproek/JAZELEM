import { Power, RotateCcw, ShieldCheck, ShieldOff } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Badge, Button, Sheet, Spinner, cx } from "./components/ui";
import { shellAction } from "./lib/api";
import { runSelftest } from "./lib/selftest";
import { ShellProvider, useShell } from "./lib/shell";
import { TABS } from "./tabs";
import { PowerSheet } from "./tabs/Power";
import { TelemetryBand } from "./components/TelemetryBand";

/** "#fan" in the address opens that tab (used for screenshots). */
function initialTab(): string {
  const id = location.hash.replace("#", "");
  return TABS.some((t) => t.id === id) ? id : TABS[0].id;
}

function Frame() {
  const shell = useShell();
  const [active, setActive] = useState(initialTab);
  const [power, setPower] = useState(false);
  const tab = TABS.find((t) => t.id === active) ?? TABS[0];

  // tell the app which views to build (the open tab, plus the Power sheet while it is open)
  useEffect(() => {
    void shellAction({ kind: "active", tabs: [...tab.backends, ...(power ? ["power"] : [])] });
  }, [tab, power]);

  useEffect(() => {
    const onHash = () => setActive(initialTab());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const go = (id: string) => {
    setActive(id);
    history.replaceState(null, "", `#${id}`);
  };

  // self-test: visit every tab, log what was rendered, quit
  const selftest = shell.selftest ?? null;
  const started = useRef(false);
  useEffect(() => {
    if (selftest === null || started.current) return;
    started.current = true;
    void runSelftest(selftest, TABS.map((t) => t.id), go);
  }, [selftest]);
  const Page = tab.page;

  return (
    <div className="app-frame">
      <TelemetryBand />
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-name">PC Tuner</div>
          <div className="brand-sub">gaming-safe Windows tuning</div>
        </div>
        <nav className="nav" aria-label="Tabs">
          {TABS.map((t) => (
            <button key={t.id} type="button" className={cx("nav-item", t.id === tab.id && "active")} aria-current={t.id === tab.id ? "page" : undefined} onClick={() => go(t.id)}>
              <t.icon />
              {t.title}
            </button>
          ))}
        </nav>
        <div className="sidebar-foot">
          <Button block icon={<Power />} title="Restart into the BIOS, or shut down so the next start opens the BIOS." onClick={() => setPower(true)}>
            Power / BIOS
          </Button>
          <Button
            block
            variant="danger"
            icon={<RotateCcw />}
            title="Puts the RTX 5080 back on the Stock preset: no undervolt, no memory overclock."
            onClick={() => {
              void shellAction({ kind: "reset_gpu" });
              go("gpu");
            }}
          >
            Reset GPU to stock
          </Button>
          <div style={{ padding: "2px 2px 0" }}>
            {shell.admin ? (
              <Badge color="green" icon={<ShieldCheck />}>
                Administrator
              </Badge>
            ) : (
              <Badge color="red" icon={<ShieldOff />}>
                View only (not admin)
              </Badge>
            )}
          </div>
        </div>
      </aside>
      <main className="content">
        <Page key={tab.id} />
      </main>

      {power && <PowerSheet onClose={() => setPower(false)} />}

      {shell.notice && (
        <Sheet
          kicker="PC Tuner"
          title="Something needs your attention"
          onClose={() => shellAction({ kind: "dismiss_notice" })}
          footer={
            <Button variant="primary" size="lg" onClick={() => shellAction({ kind: "dismiss_notice" })}>
              OK
            </Button>
          }
        >
          <p className="selectable" style={{ whiteSpace: "pre-wrap" }}>
            {shell.notice}
          </p>
        </Sheet>
      )}

      {shell.closePending && (
        <Sheet
          title={
            <span className="hstack" style={{ gap: 10 }}>
              <Spinner large />A change is still being applied
            </span>
          }
          subtitle="PC Tuner will close when it finishes, so the change is recorded and can be undone."
          footer={
            <>
              <Button variant="primary" size="lg" onClick={() => shellAction({ kind: "keep_open" })}>
                Keep PC Tuner open
              </Button>
              <Button size="lg" onClick={() => shellAction({ kind: "close_anyway" })}>
                Close anyway (the change may not be recorded)
              </Button>
            </>
          }
        />
      )}
    </div>
    </div>
  );
}

export function App() {
  return (
    <ShellProvider>
      <Frame />
    </ShellProvider>
  );
}
