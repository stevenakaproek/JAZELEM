import type { Tone } from "../lib/types";
import type { FanAction, FanCardView, FanSetting, FanTools, FanView } from "../tabs/Fan";
import { mockAdmin, mockParams } from "./env";
import type { MockTab } from "./types";

const tone = (t: number): Tone => (t >= 85 ? "Error" : t >= 70 ? "Warn" : "Accent");
const curve = (source: "cpu" | "gpu" | "hottest"): FanSetting => ({ mode: "curve", source, points: [[30, 30], [50, 40], [65, 60], [75, 80], [85, 100]] });

interface MockFan {
  id: string;
  name: string;
  hw: string;
  base: number;
  gpu: boolean;
  pump: boolean;
  controllable: boolean;
  floor: number;
  setting: FanSetting;
  pinned: boolean;
  hist: number[];
}

const board = "MSI MAG X870E TOMAHAWK WIFI (NCT6687D)";
const fans: MockFan[] = [
  { id: "cpu", name: "CPU Fan", hw: board, base: 900, gpu: false, pump: false, controllable: true, floor: 0, setting: { mode: "manual", percent: 45 }, pinned: true, hist: [] },
  { id: "pump", name: "Pump Fan", hw: board, base: 2400, gpu: false, pump: true, controllable: true, floor: 0, setting: curve("cpu"), pinned: false, hist: [] },
  { id: "sys1", name: "System Fan #1", hw: board, base: 780, gpu: false, pump: false, controllable: true, floor: 0, setting: { mode: "trigger", source: "hottest", idle_temp: 55, load_temp: 65, idle: 30, load: 70 }, pinned: false, hist: [] },
  { id: "sys2", name: "System Fan #2", hw: board, base: 760, gpu: false, pump: false, controllable: true, floor: 0, setting: curve("hottest"), pinned: false, hist: [] },
  { id: "gpu", name: "GPU Fan", hw: "NVIDIA GeForce RTX 5080", base: 1150, gpu: true, pump: false, controllable: true, floor: 30, setting: { mode: "bios" }, pinned: false, hist: [] },
  { id: "sys5", name: "System Fan #5", hw: board, base: 0, gpu: false, pump: false, controllable: true, floor: 0, setting: { mode: "bios" }, pinned: false, hist: [] },
];
let note: string | null = null;
let tick = 0;
let cpu = 58;
let gpu = 52;
const cpuHist: number[] = [];
const gpuHist: number[] = [];
let dismissed = 0;

function step() {
  tick += 1;
  cpu = 58 + 6 * Math.sin(tick / 9) + 2 * Math.sin(tick / 2.3);
  gpu = 52 + 8 * Math.sin(tick / 13 + 1);
  cpuHist.push(cpu);
  gpuHist.push(gpu);
  if (cpuHist.length > 60) cpuHist.shift();
  if (gpuHist.length > 60) gpuHist.shift();
  for (const f of fans) {
    f.hist.push(f.base ? f.base + 40 * Math.sin(tick / 5 + f.base) : 0);
    if (f.hist.length > 60) f.hist.shift();
  }
}
for (let i = 0; i < 60; i++) step();

function duty(points: [number, number][], t: number): number {
  if (t <= points[0][0]) return points[0][1];
  for (let i = 1; i < points.length; i++) {
    const [t0, d0] = points[i - 1];
    const [t1, d1] = points[i];
    if (t <= t1) return d0 + ((d1 - d0) * (t - t0)) / (t1 - t0);
  }
  return points[points.length - 1][1];
}

const PRESETS: Record<string, [number, number][]> = {
  Silent: [[30, 20], [50, 25], [65, 40], [75, 60], [85, 100]],
  Balanced: [[30, 30], [50, 40], [65, 60], [75, 80], [85, 100]],
  Performance: [[30, 40], [50, 55], [65, 75], [75, 90], [80, 100]],
};
const presetCurve = (f: MockFan, n: string): FanSetting => ({ mode: "curve", source: f.gpu ? "gpu" : f.pump || f.name.startsWith("CPU") ? "cpu" : "hottest", points: PRESETS[n].map(([t, d]) => [t, Math.max(d, f.floor)] as [number, number]) });

function card(f: MockFan): FanCardView {
  const s = f.setting;
  const src = s.mode === "curve" || s.mode === "trigger" ? s.source : null;
  const live = src === "cpu" ? cpu : src === "gpu" ? gpu : src ? Math.max(cpu, gpu) : null;
  const liveDuty = live === null ? null : s.mode === "curve" ? Math.max(duty(s.points, live), f.floor) : s.mode === "trigger" ? (live >= s.load_temp ? s.load : s.idle) : null;
  const ours = s.mode !== "bios";
  const text = !mockAdmin ? "BIOS" : s.mode === "curve" ? "Curve" : s.mode === "trigger" ? "Trigger" : ours ? "PC Tuner" : "BIOS";
  return {
    id: f.id,
    name: f.name,
    hw: f.hw,
    rpm: f.hist[f.hist.length - 1] ?? 0,
    percent: s.mode === "manual" ? s.percent : (liveDuty ?? (f.base ? 40 : 0)),
    controllable: f.controllable,
    pump: f.pump,
    gpu: f.gpu,
    chip: { text, tone: text === "BIOS" ? "Off" : "Accent" },
    hist: f.hist,
    floor: f.floor,
    setting: s,
    notice: !mockAdmin && ours ? "Saved setting below - not applied (view-only)." : null,
    presets: f.controllable ? Object.keys(PRESETS).filter((n) => !(f.gpu && n === "Silent")).map((n) => ({ name: n, active: JSON.stringify(f.setting) === JSON.stringify(presetCurve(f, n)) })) : [],
    live,
    liveDuty,
    afterburner: f.gpu && ours,
    tools: tools(f),
  };
}

const state = mockParams.get("state");

// speed sensor pairing and calibration: the CPU fan is calibrated, a test can be run on the others
const sensors = [
  { id: "none", label: "None (no speed reading)" },
  ...fans.filter((f) => !f.gpu).map((f, i) => ({ id: `/lpc/nct6687d/0/fan/${i}`, label: `${f.name} (${board})${f.base ? ` - ${f.base} RPM` : ""}` })),
  { id: "/gpu-nvidia/0/fan/1", label: "GPU Fan (NVIDIA GeForce RTX 5080) - 1150 RPM" },
];
const pairs: Record<string, string> = Object.fromEntries(fans.map((f, i) => [f.id, f.gpu ? "/gpu-nvidia/0/fan/1" : `/lpc/nct6687d/0/fan/${i}`]));
const manual = new Set<string>();
const calib: Record<string, NonNullable<FanTools["calib"]>> = {
  cpu: { summary: "1650 RPM at 100 %, stops below 20 %, starts again at 30 %", points: [[100, 1650], [90, 1540], [80, 1420], [70, 1290], [60, 1150], [50, 1010], [40, 860], [30, 700], [20, 520], [10, 0], [0, 0]], when: "2026-11-16 10:20:41" },
};
let test: { id: string; kind: "calibrate" | "pair"; step: number; steps: number } | null = state === "calibrating" ? { id: "sys1", kind: "calibrate", step: 4, steps: 24 } : null;
const results: Record<string, { ok: boolean; text: string }> = {};
const DUTIES = [100, 90, 80, 70, 60, 50, 40, 30, 20, 10, 0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60];

function tools(f: MockFan): FanTools | null {
  if (!f.controllable || f.pump) return null;
  const mine = test && test.id === f.id ? test : null;
  return {
    sensors,
    sensor: pairs[f.id] ?? "none",
    manual: manual.has(f.id),
    calib: calib[f.id] ?? null,
    test: mine ? { kind: mine.kind, step: mine.step, steps: mine.steps, duty: mine.kind === "pair" ? (mine.step === 0 ? 30 : 100) : DUTIES[Math.min(mine.step, DUTIES.length - 1)] } : null,
    otherRunning: test !== null && !mine,
    result: results[f.id] ?? null,
    canTest: mockAdmin && test === null,
  };
}

export const fan: MockTab<FanView, FanAction> = {
  view() {
    const base: FanView = {
      can: mockAdmin,
      viewOnly: !mockAdmin,
      error: null,
      anyControlled: fans.some((f) => f.setting.mode !== "bios"),
      note,
      sources: [
        { key: "cpu", label: "CPU temperature" },
        { key: "gpu", label: "GPU temperature" },
        { key: "hottest", label: "Hotter of CPU and GPU" },
      ],
      graph: { tMin: 20, tMax: 100, minPoints: 2, maxPoints: 16 },
    };
    if (state === "reading") return { ...base, reading: true };
    if (state === "stale") return { ...base, stale: { head: "The fan helper is not running", sub: "Fans are on BIOS control. Restart PC Tuner to try again." } };
    const hottest = Math.max(cpu, gpu);
    const live = fans.filter((f) => f.base > 0 || f.setting.mode !== "bios");
    return {
      ...base,
      status: {
        emergency: state === "hot",
        blind: false,
        msg: dismissed === 0 && state === "msg" ? { seq: 1, text: "System Fan #3: the controller refused 20 % and uses 30 %." } : null,
        hottest: { value: hottest, which: gpu >= cpu ? "GPU" : "CPU", tone: tone(hottest) },
        cpu: mockAdmin ? { value: cpu, tone: tone(cpu) } : null,
        gpu: { value: gpu, tone: tone(gpu) },
        fansFound: fans.length,
        controllable: fans.filter((f) => f.controllable).length,
        cpuHist: mockAdmin ? cpuHist : [],
        gpuHist,
        fans: live.map(card),
        empty: fans.filter((f) => !live.includes(f)).map(card),
        temps: [
          { hw: "AMD Ryzen 7 9800X3D", name: "Core (Tctl/Tdie)", value: cpu, tone: tone(cpu) },
          { hw: "NVIDIA GeForce RTX 5080", name: "GPU Core", value: gpu, tone: tone(gpu) },
          { hw: board, name: "System", value: 38, tone: tone(38) },
          { hw: board, name: "VRM MOS", value: 47, tone: tone(47) },
        ],
      },
    };
  },
  act(a) {
    if (a.kind === "dismiss") {
      dismissed = a.seq;
      return;
    }
    if (!mockAdmin) return;
    const f = "id" in a ? fans.find((x) => x.id === a.id) : undefined;
    const own = (s: FanSetting) => {
      if (f) {
        f.setting = s;
        f.pinned = true;
      }
    };
    switch (a.kind) {
      case "give_back":
        fans.forEach((x) => {
          x.setting = { mode: "bios" };
          x.pinned = false;
        });
        note = "Every fan is back on BIOS control.";
        break;
      case "preset":
        if (f && PRESETS[a.name] && !(f.gpu && a.name === "Silent")) own(presetCurve(f, a.name));
        break;
      case "mode":
        if (!f || f.setting.mode === a.mode) break;
        if (a.mode === "bios") own({ mode: "bios" });
        if (a.mode === "manual") own({ mode: "manual", percent: 50 });
        if (a.mode === "curve") own(curve(f.gpu ? "gpu" : "hottest"));
        if (a.mode === "trigger") own({ mode: "trigger", source: f.gpu ? "gpu" : "hottest", idle_temp: 55, load_temp: 65, idle: 30, load: 70 });
        break;
      case "manual":
        own({ mode: "manual", percent: Math.max(f?.floor ?? 0, Math.round(a.percent)) });
        break;
      case "source":
        if (f && (f.setting.mode === "curve" || f.setting.mode === "trigger")) own({ ...f.setting, source: a.source });
        break;
      case "curve_point":
        if (f && f.setting.mode === "curve") {
          const points = f.setting.points.map((p) => [p[0], p[1]] as [number, number]);
          points[a.index] = [Math.round(a.t), Math.round(a.d)];
          own({ ...f.setting, points });
        }
        break;
      case "curve_add":
        if (f && f.setting.mode === "curve") {
          const t = Math.round(a.t);
          if (f.setting.points.some((p) => Math.abs(p[0] - t) < 2)) break;
          const points = [...f.setting.points, [t, Math.max(f.floor, Math.round(a.d))] as [number, number]].sort((p, q) => p[0] - q[0]);
          own({ ...f.setting, points });
        }
        break;
      case "curve_remove":
        if (f && f.setting.mode === "curve" && f.setting.points.length > 2) own({ ...f.setting, points: f.setting.points.filter((_, i) => i !== a.index) });
        break;
      case "trigger":
        if (f && f.setting.mode === "trigger") own({ ...f.setting, idle_temp: Math.min(a.idle_temp, a.load_temp - 2), load_temp: a.load_temp, idle: a.idle, load: a.load });
        break;
      case "pump":
        if (f) f.pump = a.on;
        break;
      case "pair_sensor":
        if (a.sensor === "auto") {
          manual.delete(a.id);
          pairs[a.id] = sensors[fans.findIndex((x) => x.id === a.id) + 1]?.id ?? "none";
        } else {
          manual.add(a.id);
          pairs[a.id] = a.sensor;
        }
        delete calib[a.id];
        break;
      case "test":
        if (mockAdmin && !test) {
          test = { id: a.id, kind: a.test, step: 0, steps: a.test === "pair" ? 2 : 24 };
          delete results[a.id];
        }
        break;
      case "cancel_test":
        if (test) results[test.id] = { ok: false, text: "Cancelled." };
        test = null;
        break;
      default:
        break;
    }
  },
  tick() {
    if (test && tick % 2 === 0 && state !== "calibrating") {
      test.step += 1;
      if (test.step >= test.steps) {
        const t = test;
        test = null;
        if (t.kind === "pair") {
          manual.add(t.id);
          results[t.id] = { ok: true, text: `Paired with ${sensors.find((s) => s.id === pairs[t.id])?.label.split(" (")[0] ?? "its sensor"}.` };
        } else {
          calib[t.id] = { summary: "1180 RPM at 100 %, stops below 30 %, starts again at 35 %", points: [[100, 1180], [90, 1100], [80, 1010], [70, 920], [60, 820], [50, 720], [40, 600], [30, 470], [20, 0], [10, 0], [0, 0]], when: "2026-11-16 10:31:05" };
          results[t.id] = { ok: true, text: "Calibration finished." };
        }
      }
    }
    if (tick % 2 === 0) step();
    else tick += 1;
    return tick % 2 === 1;
  },
};
