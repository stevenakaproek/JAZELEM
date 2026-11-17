// The telemetry band across the top of the app, visible on every tab: processor, memory and graphics card,
// each a name and a row of readings (bold number, small unit, a thin bar that fills with the value).
import { Cpu, Gpu, MemoryStick } from "lucide-react";
import { useEffect, useState } from "react";
import { onTelemetry, type TeleItem, type Telemetry } from "../lib/api";

const ICON = { cpu: Cpu, memory: MemoryStick, gpu: Gpu };

/** Bar colour by reading: temperature goes green, orange, red as it rises. */
function barColor(i: TeleItem): string {
  const v = i.value ?? 0;
  if (i.key === "temp") return v >= 85 ? "var(--red)" : v >= 70 ? "var(--orange)" : "var(--green)";
  if (i.key === "power") return "color-mix(in srgb, var(--green), var(--yellow))";
  if (i.key === "clock") return "var(--yellow)";
  if (i.key === "load") return "var(--accent)";
  return "var(--green)";
}

function Reading({ i }: { i: TeleItem }) {
  const fill = i.max !== null && i.value !== null ? Math.min(100, Math.max(0, (i.value / i.max) * 100)) : null;
  return (
    <div className="tele-item" title={i.label}>
      <div className="tele-read">
        <span className="tele-value">{i.text}</span>
        <span className="tele-unit">{i.unit}</span>
      </div>
      <div className="tele-bar" aria-hidden>
        {fill !== null && <span style={{ width: `${fill}%`, background: barColor(i) }} />}
      </div>
    </div>
  );
}

export function TelemetryBand() {
  const [t, setT] = useState<Telemetry | null>(null);
  useEffect(() => onTelemetry(setT), []);
  if (!t) return <div className="tele-band" aria-label="Telemetry" />;
  return (
    <div className="tele-band" aria-label="Telemetry">
      {t.groups.map((g) => {
        const Icon = ICON[g.kind];
        return (
          <section className="tele-group" key={g.kind}>
            <div className="tele-name">
              <Icon size={15} />
              {g.name}
            </div>
            <div className="tele-items">
              {g.items.map((i) => (
                <Reading key={i.key} i={i} />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}
