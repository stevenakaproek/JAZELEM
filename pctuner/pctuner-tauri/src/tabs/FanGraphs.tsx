// The two fan graphs: the curve editor (drag, click to add, right-click to remove, typed values)
// and the picture of a trigger's two steady speeds.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button, NumberField } from "../components/ui";

export interface GraphLimits {
  tMin: number;
  tMax: number;
  minPoints: number;
  maxPoints: number;
}

const PAD_X = 8;
const PAD_Y = 10;

function useWidth() {
  const ref = useRef<SVGSVGElement>(null);
  const [width, setWidth] = useState(300);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth || 300));
    ro.observe(el);
    setWidth(el.clientWidth || 300);
    return () => ro.disconnect();
  }, []);
  return { ref, width };
}

function Grid({ x, y, w, h, limits, floor }: { x: (t: number) => number; y: (d: number) => number; w: number; h: number; limits: GraphLimits; floor: number }) {
  const temps = [];
  for (let t = 30; t <= 90; t += 10) temps.push(t);
  return (
    <>
      <rect x={0} y={0} width={w} height={h} rx={8} className="graph-bg" />
      {temps.map((t) => (
        <line key={t} x1={x(t)} x2={x(t)} y1={PAD_Y} y2={h - PAD_Y} className="graph-grid" />
      ))}
      {[25, 50, 75].map((d) => (
        <line key={d} x1={PAD_X} x2={w - PAD_X} y1={y(d)} y2={y(d)} className="graph-grid" />
      ))}
      {floor > 0 && (
        <>
          <rect x={PAD_X} y={y(floor)} width={w - 2 * PAD_X} height={h - PAD_Y - y(floor)} className="graph-floor" />
          <line x1={PAD_X} x2={w - PAD_X} y1={y(floor)} y2={y(floor)} className="graph-floor-line" />
          <text x={x(90) - 4} y={y(floor) + 11} textAnchor="end" className="graph-label red">
            min {floor.toFixed(0)} %
          </text>
        </>
      )}
      <line x1={x(90)} x2={x(90)} y1={PAD_Y} y2={h - PAD_Y} className="graph-limit" />
      <text x={x(90) + 3} y={PAD_Y + 9} className="graph-label red">
        90°
      </text>
      <text x={PAD_X + 2} y={h - PAD_Y - 3} className="graph-label">
        {limits.tMin} °C
      </text>
    </>
  );
}

export interface CurveProps {
  points: [number, number][];
  floor: number;
  live: number | null;
  liveDuty: number | null;
  limits: GraphLimits;
  disabled: boolean;
  onPoint: (index: number, t: number, d: number) => void;
  onAdd: (t: number, d: number) => void;
  onRemove: (index: number) => void;
}

export function CurveEditor({ points, floor, live, liveDuty, limits, disabled, onPoint, onAdd, onRemove }: CurveProps) {
  const { ref, width: w } = useWidth();
  const h = 150;
  const [sel, setSel] = useState<number | null>(null);
  const [hover, setHover] = useState<number | null>(null);
  // while a point is dragged the graph follows the pointer at once; the app's answer replaces it
  const [drag, setDrag] = useState<{ index: number; points: [number, number][] } | null>(null);
  const pts = drag?.points ?? points;
  const selected = sel !== null && sel < pts.length ? sel : null;
  const pendingAdd = useRef<number | null>(null);

  useEffect(() => {
    // a point was just added: select it once the app has answered
    if (pendingAdd.current !== null) {
      const i = points.findIndex((p) => p[0] === pendingAdd.current);
      if (i >= 0) setSel(i);
      pendingAdd.current = null;
    }
  }, [points]);

  const span = limits.tMax - limits.tMin;
  const x = (t: number) => PAD_X + ((w - 2 * PAD_X) * (t - limits.tMin)) / span;
  const y = (d: number) => h - PAD_Y - ((h - 2 * PAD_Y) * d) / 100;
  const fromScreen = (e: { clientX: number; clientY: number }) => {
    const r = ref.current!.getBoundingClientRect();
    const t = limits.tMin + ((e.clientX - r.left - PAD_X) / (w - 2 * PAD_X)) * span;
    const d = ((h - PAD_Y - (e.clientY - r.top)) / (h - 2 * PAD_Y)) * 100;
    return [t, d] as const;
  };
  const range = (list: [number, number][], i: number) => {
    const lo = i > 0 ? list[i - 1][0] + 2 : limits.tMin;
    const hi = i + 1 < list.length ? list[i + 1][0] - 2 : limits.tMax;
    return [lo, Math.max(hi, lo)] as const;
  };
  const clampPoint = (list: [number, number][], i: number, t: number, d: number): [number, number] => {
    const [lo, hi] = range(list, i);
    return [Math.min(hi, Math.max(lo, Math.round(t))), Math.min(100, Math.max(Math.min(floor, 100), Math.round(d)))];
  };

  const first = pts[0]?.[1] ?? 100;
  const last = pts[pts.length - 1]?.[1] ?? 100;
  const line = [`${x(limits.tMin)},${y(Math.max(first, floor))}`, ...pts.map((p) => `${x(p[0])},${y(Math.max(p[1], floor))}`), `${x(limits.tMax)},${y(Math.max(last, floor))}`].join(" ");
  const full = pts.length >= limits.maxPoints;

  return (
    <div className="stack" style={{ gap: 6 }}>
      <svg
        ref={ref}
        className="graph"
        height={h}
        onClick={(e) => {
          if (disabled || drag || (e.target as Element).closest(".graph-point")) return;
          const [t, d] = fromScreen(e);
          pendingAdd.current = Math.min(limits.tMax, Math.max(limits.tMin, Math.round(t)));
          onAdd(t, d);
        }}
      >
        <Grid x={x} y={y} w={w} h={h} limits={limits} floor={floor} />
        <text x={PAD_X + 2} y={PAD_Y + 9} className="graph-label">
          100 %
        </text>
        <polyline points={line} className="graph-line" />
        {live !== null && liveDuty !== null && (
          <>
            <circle cx={x(Math.min(limits.tMax, Math.max(limits.tMin, live)))} cy={y(liveDuty)} r={5} className="graph-live" />
            <text x={PAD_X + 2} y={PAD_Y + 24} className="graph-label live">
              now {live.toFixed(0)} °C → {liveDuty.toFixed(0)} %
            </text>
          </>
        )}
        {pts.map((p, i) => {
          const cx = x(p[0]);
          const cy = y(Math.max(p[1], floor));
          const hot = drag?.index === i;
          return (
            <g
              key={i}
              className="graph-point"
              onPointerDown={(e) => {
                if (disabled || e.button !== 0) return;
                (e.currentTarget as Element).setPointerCapture(e.pointerId);
                setSel(i);
                setDrag({ index: i, points: pts.map((q) => [q[0], q[1]]) });
              }}
              onPointerMove={(e) => {
                if (!drag || drag.index !== i) return;
                const [t, d] = fromScreen(e);
                const next = clampPoint(drag.points, i, t, d);
                if (next[0] !== drag.points[i][0] || next[1] !== drag.points[i][1]) {
                  const copy = drag.points.map((q) => [q[0], q[1]] as [number, number]);
                  copy[i] = next;
                  setDrag({ index: i, points: copy });
                  onPoint(i, next[0], next[1]);
                }
              }}
              onPointerUp={() => setDrag(null)}
              onPointerCancel={() => setDrag(null)}
              onPointerEnter={() => setHover(i)}
              onPointerLeave={() => setHover(null)}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                if (disabled) return;
                setSel(null);
                onRemove(i);
              }}
            >
              <circle cx={cx} cy={cy} r={11} fill="transparent" />
              <circle cx={cx} cy={cy} r={hot ? 7 : 5.5} className={selected === i ? "graph-dot selected" : "graph-dot"} />
              {(hot || hover === i) && (
                <text x={cx} y={cy - 12} textAnchor="middle" className="graph-label strong">
                  {p[0].toFixed(0)} °C → {p[1].toFixed(0)} %
                </text>
              )}
            </g>
          );
        })}
      </svg>
      <div className="hstack" style={{ flexWrap: "wrap", minHeight: 28 }}>
        {selected !== null ? (
          <>
            <span className="secondary">
              Point {selected + 1} of {pts.length}
            </span>
            <NumberField label="Temperature of this point" value={pts[selected][0]} min={range(pts, selected)[0]} max={range(pts, selected)[1]} suffix="°C" disabled={disabled} onCommit={(t) => onPoint(selected, t, pts[selected][1])} />
            <NumberField label="Fan speed at that temperature" value={Math.max(pts[selected][1], floor)} min={floor} max={100} suffix="%" disabled={disabled} onCommit={(d) => onPoint(selected, pts[selected][0], d)} />
            <Button
              disabled={disabled || pts.length <= limits.minPoints}
              title={pts.length <= limits.minPoints ? "A curve needs at least 2 points." : undefined}
              onClick={() => {
                onRemove(selected);
                setSel(null);
              }}
            >
              Remove point
            </Button>
          </>
        ) : (
          <span className="tertiary">Click a point to type its exact temperature and speed.</span>
        )}
      </div>
      <div className="tertiary" style={{ fontSize: "var(--text-sm)" }}>
        {full ? "Right-click a point to remove it (the curve has the most points it can hold)." : "Click the graph to add a point, right-click a point to remove it."}
      </div>
    </div>
  );
}

export interface TriggerValues {
  idle_temp: number;
  load_temp: number;
  idle: number;
  load: number;
}

export function TriggerEditor({ v, floor, live, liveDuty, limits, disabled, onChange }: { v: TriggerValues; floor: number; live: number | null; liveDuty: number | null; limits: GraphLimits; disabled: boolean; onChange: (v: TriggerValues) => void }) {
  const { ref, width: w } = useWidth();
  const h = 110;
  const span = limits.tMax - limits.tMin;
  const x = (t: number) => PAD_X + ((w - 2 * PAD_X) * (Math.min(limits.tMax, Math.max(limits.tMin, t)) - limits.tMin)) / span;
  const y = (d: number) => h - PAD_Y - ((h - 2 * PAD_Y) * Math.min(100, Math.max(0, d))) / 100;
  const { idle_temp: it, load_temp: lt, idle, load } = v;
  return (
    <div className="stack" style={{ gap: 8 }}>
      <svg ref={ref} className="graph" height={h}>
        <rect x={0} y={0} width={w} height={h} rx={8} className="graph-bg" />
        {/* between the two temperatures the fan keeps whichever speed it had */}
        <rect x={x(it)} y={PAD_Y} width={Math.max(0, x(lt) - x(it))} height={h - 2 * PAD_Y} className="graph-band" />
        {[25, 50, 75].map((d) => (
          <line key={d} x1={PAD_X} x2={w - PAD_X} y1={y(d)} y2={y(d)} className="graph-grid" />
        ))}
        <line x1={x(90)} x2={x(90)} y1={PAD_Y} y2={h - PAD_Y} className="graph-limit" />
        <polyline points={`${PAD_X},${y(idle)} ${x(lt)},${y(idle)} ${x(lt)},${y(load)} ${w - PAD_X},${y(load)}`} className="graph-line" />
        <polyline points={`${x(lt)},${y(load)} ${x(it)},${y(load)} ${x(it)},${y(idle)}`} className="graph-line dashed" />
        <text x={x(lt) + 4} y={y(load) - 4} className="graph-label">
          gaming {load.toFixed(0)} %
        </text>
        <text x={PAD_X + 2} y={y(idle) - 4} className="graph-label">
          idle {idle.toFixed(0)} %
        </text>
        <text x={x(90) + 3} y={PAD_Y + 9} className="graph-label red">
          90°
        </text>
        {live !== null && liveDuty !== null && (
          <>
            <circle cx={x(live)} cy={y(liveDuty)} r={5} className="graph-live" />
            <text x={PAD_X + 2} y={PAD_Y + 9} className="graph-label live">
              now {live.toFixed(0)} °C → {liveDuty.toFixed(0)} %
            </text>
          </>
        )}
      </svg>
      <div className="trigger-grid">
        <span className="secondary">Gaming from</span>
        <NumberField label="At this temperature the fan steps up to the gaming speed." value={lt} min={limits.tMin + 2} max={limits.tMax} suffix="°C" disabled={disabled} onCommit={(n) => onChange({ ...v, load_temp: n })} />
        <span className="secondary">at</span>
        <NumberField label="Gaming speed" value={load} min={floor} max={100} suffix="%" disabled={disabled} onCommit={(n) => onChange({ ...v, load: n })} />
        <span className="secondary">Idle below</span>
        <NumberField label="The fan drops back to the idle speed only when the temperature falls to this point (at least 2 °C under the gaming point)." value={it} min={limits.tMin} max={limits.tMax - 2} suffix="°C" disabled={disabled} onCommit={(n) => onChange({ ...v, idle_temp: n })} />
        <span className="secondary">at</span>
        <NumberField label="Idle speed" value={idle} min={floor} max={100} suffix="%" disabled={disabled} onCommit={(n) => onChange({ ...v, idle: n })} />
      </div>
      <div className="tertiary" style={{ fontSize: "var(--text-sm)" }}>
        Steady speeds: short temperature spikes in games do not rev the fan up and down.
      </div>
    </div>
  );
}
