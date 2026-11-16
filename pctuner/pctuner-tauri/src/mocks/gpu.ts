import type { Action, GpuCustom, GpuTile, GpuTrial, View } from "../tabs/Gpu";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import type { MockTab } from "./types";

// this RTX 5080: core offsets worked out from its own curve
const PRESETS = [
  { name: "Stock", mv: 0, max: 0, core: 0, mem: 0, power: 100, text: "Factory behaviour - no undervolt, no memory overclock. Use this to reset or to compare." },
  { name: "Eco", mv: 850, max: 2650, core: 288, mem: 0, power: 88, text: "Coolest and quietest. 2650 MHz at 850 mV, power limit 88 %." },
  { name: "MFG", mv: 850, max: 2650, core: 288, mem: 0, power: 100, text: "For DLSS Multi Frame Generation. The Eco curve (2650 MHz at 850 mV) with the full power limit." },
  { name: "Balanced", mv: 900, max: 2750, core: 185, mem: 0, power: 90, text: "Everyday gaming. 2750 MHz at 900 mV, power limit 90 %." },
  { name: "Performance", mv: 925, max: 2980, core: 318, mem: 0, power: 100, text: "High fps. 2980 MHz at 925 mV, full power limit." },
  { name: "Max", mv: 975, max: 3150, core: 330, mem: 0, power: 100, text: "Most fps. 3150 MHz at 975 mV, full power limit. Runs warmest of the undervolts." },
];
const CUSTOM = 6;
const name = (i: number) => (i === CUSTOM ? "Custom" : PRESETS[i].name);

// "?state=trial | unguarded | reverted | loading | error | afterburner | nodriver" shows the other faces of the tab
const state = mockParams.get("state");
const op = new MockOp();
let current = state === "nodriver" ? -1 : 4;
let confirmed = "Performance";
let selected: number | null = 4;
const custom = { maxMhz: 2750, core: 120, mem: 1000, power: 100 };
let status = "";
let error: string | null = state === "error" ? "Could not work out Performance for this card:\nA core offset is set on the card right now, so its stock curve cannot be read. Use Reset to stock once, then try again." : null;
let trial: { preset: string; previous: string; status: "pending" | "reverted" } | null =
  state === "trial" || state === "unguarded" ? { preset: "Max", previous: "Performance", status: "pending" } : state === "reverted" ? { preset: "Max", previous: "Performance", status: "reverted" } : null;
if (trial?.status === "pending") current = 5;
if (trial?.status === "reverted") current = 0;

type Sample = { core: number; temp: number; power: number; util: number; volt: number; vram: number; memclock: number; fan: number };
const hist: Sample[] = [];
let t = 0;
function sample() {
  t += 1;
  const wave = Math.sin(t / 7);
  hist.push({ core: 2790 + Math.round(wave * 12), temp: 61 + Math.round(wave * 3), power: 232 + wave * 18, util: 93 + Math.round(wave * 5), volt: 925 + Math.round(wave * 5), vram: 9800 + Math.round(wave * 300), memclock: 15001, fan: 48 + Math.round(wave * 3) });
  if (hist.length > 60) hist.shift();
}
for (let i = 0; i < 40; i++) sample();

function live(): GpuTile[] {
  const l = hist[hist.length - 1];
  const tile = (key: keyof Sample, label: string, unit: string, sub: string | null = null): GpuTile => ({ key, label, unit, value: l[key], sub, hist: hist.map((h) => h[key]) });
  return [
    tile("core", "Core clock", "MHz"),
    tile("temp", "Temperature", "°C"),
    tile("power", "Power draw", "W"),
    tile("util", "GPU load", "%"),
    tile("volt", "Core voltage", "mV"),
    tile("vram", "Memory used", "MB", "of 16303 MB"),
    tile("memclock", "Memory clock", "MHz"),
    tile("fan", "Fan speed", "%"),
  ];
}
let powerPct = 100;

function tuning(i: number) {
  return i === CUSTOM ? { max: custom.maxMhz, core: custom.core, mem: custom.mem, power: custom.power } : { ...PRESETS[i] };
}

function describe(i: number): string {
  if (i === 0) return "no offsets, no clock limit, power 100%";
  const x = tuning(i);
  return `max ${x.max} MHz, core +${x.core} MHz, memory +${x.mem} MHz, power ${x.power}%`;
}

function customView(): GpuCustom {
  return { ...custom, powerWatts: Math.round((360 * custom.power) / 100), limits: { maxMhz: [1500, 3210], core: [-500, 450], mem: [0, 3000], power: [70, 125] }, summary: describe(CUSTOM) };
}

function trialView(): GpuTrial | null {
  if (!trial) return null;
  if (trial.status === "pending") return { kind: "pending", preset: trial.preset, previous: trial.previous, appliedAt: "21:20", unguarded: state === "unguarded", canKeep: mockAdmin, canRevert: true };
  return {
    kind: "reverted",
    preset: trial.preset,
    text: `Windows logged an unexpected shutdown during the ${trial.preset} trial, so the GPU was set back to Stock at sign-in. If it was not a power cut, try a milder preset.`,
    reason: "event 41 (Microsoft-Windows-Kernel-Power) at 2026-11-11 21:47:03",
    lastError: "",
  };
}

function openApply(idx: number, kicker: string) {
  const n = name(idx);
  const startsTrial = idx !== 0 && (confirmed !== n || idx === CUSTOM);
  const endsTrial = !startsTrial && trial?.status === "pending";
  const items = [];
  if (startsTrial) items.push(opItem("trial", "Arm the safety trial (auto-revert after a crash)", ""));
  items.push(opItem("set", "Set it on the GPU", "", describe(idx)));
  items.push(opItem("confirm", "Read it back from the GPU", ""));
  if (endsTrial) items.push(opItem("endtrial", "End the safety trial", ""));
  items.push(opItem("save", idx === 0 ? "Stop setting a tune at sign-in" : "Set it again at every sign-in", ""));
  const warn: string[] = [];
  if (n === "Performance" || n === "Max") warn.push("Runs warmest - test stability before long sessions.");
  if (idx === CUSTOM) warn.push("Custom values are not tested by PC Tuner - test stability before long sessions.");
  if (state === "afterburner") warn.push("MSI Afterburner is running and may overwrite these settings - close it first.");
  op.open(
    makeOp(kicker, "run", "done", items, {
      noun: ["step", "steps"],
      fail_phrase: "did not complete",
      preview_text: idx === CUSTOM ? `Custom: ${describe(idx)}.` : `${n}: ${PRESETS[idx].text}`,
      running_text: `Applying ${n}...`,
      go_label: idx === 0 ? "Reset to stock" : `Apply ${n}`,
      warning: warn.length ? warn.join("\n") : null,
      reassure: startsTrial
        ? `Starts as a trial: Keep it or Revert to ${confirmed} afterwards. After a crash the GPU goes back to Stock at your next sign-in.`
        : idx === 0
          ? "Stock is the factory behaviour: no undervolt, no memory overclock."
          : "This is the tune you already confirmed.",
    }),
    () => {
      current = idx;
      trial = startsTrial ? { preset: n, previous: confirmed, status: "pending" } : null;
      if (op.op) op.op.can_undo = false;
      if (startsTrial) op.op?.notes.push("Trial running - use Keep or Revert at the top of this tab once you have tested it.");
    },
  );
}

const clamp = (v: number, [lo, hi]: [number, number]) => Math.min(hi, Math.max(lo, v));

export const gpu: MockTab<View, Action> = {
  view() {
    const noDriver = state === "nodriver";
    const cur = current >= 0 ? tuning(current) : null;
    const busy = op.op?.stage === "Running";
    return {
      trial: trialView(),
      hero:
        state === "loading"
          ? null
          : noDriver
            ? { name: "Unknown", unit: "", caption: "The GPU could not be read through the NVIDIA driver.", tone: "Off", chips: [{ text: "The NVIDIA driver could not be read", tone: "Off" }], error: "The NVIDIA driver's management library (nvml.dll) was not found - is the NVIDIA driver installed?", live: null }
            : {
                name: name(current),
                unit: current === CUSTOM ? "settings" : "preset",
                caption: current === 0 ? "Factory settings: no offsets, no clock limit." : describe(current),
                tone: "Accent",
                chips: [
                  { text: `Driver reports: core +${cur?.core ?? 0} MHz, memory +${cur?.mem ?? 0} MHz`, tone: current === 0 ? "Dim" : "Ok" },
                  { text: `Memory ${15001 + (cur?.mem ?? 0)} MHz (+${cur?.mem ?? 0} over stock 15001)`, tone: "Dim" },
                ],
                error: null,
                live: live(),
              },
      canReset: !busy,
      presets: PRESETS.map((p, index) => ({
        index,
        name: p.name,
        detail: index === 0 ? "no offsets, no clock limit" : `${p.mv} mV · max ${p.max} MHz · core +${p.core} · power ${p.power} %`,
        text: p.text,
        selected: selected === index,
        current: current === index,
      })),
      custom: noDriver ? null : customView(),
      customSelected: selected === CUSTOM,
      customCurrent: current === CUSTOM,
      power: noDriver ? null : { pct: powerPct, min: 70, max: 125, defaultW: 360, can: mockAdmin && !busy },
      afterburner: { running: state === "afterburner", autostart: state === "afterburner", canClose: true },
      applyLabel: selected === null ? "Apply preset" : `Apply ${name(selected)}`,
      canApply: !busy && selected !== null && !noDriver,
      canEdit: !busy,
      canRefresh: true,
      status,
      error,
      op: op.op,
      opCan: mockAdmin,
    };
  },
  act(a) {
    switch (a.kind) {
      case "select":
        selected = a.index;
        break;
      case "power":
        powerPct = Math.min(125, Math.max(70, a.pct));
        status = `Power limit set to ${powerPct} % (${Math.round((360 * powerPct) / 100)} W).`;
        break;
      case "custom": {
        const l = customView().limits;
        if (a.field === "max_mhz") custom.maxMhz = clamp(a.value, l.maxMhz);
        if (a.field === "core") custom.core = clamp(a.value, l.core);
        if (a.field === "mem") custom.mem = clamp(a.value, l.mem);
        if (a.field === "power") custom.power = clamp(a.value, l.power);
        selected = CUSTOM;
        break;
      }
      case "dismiss_error":
        error = null;
        break;
      case "op":
        op.event(a.ev);
        break;
      case "click":
        switch (a.click) {
          case "Reset":
            openApply(0, "RESET GPU TO STOCK");
            break;
          case "Apply":
            if (selected !== null) openApply(selected, `APPLY ${name(selected).toUpperCase()}`);
            break;
          case "Revert": {
            const idx = Math.max(0, PRESETS.findIndex((p) => p.name === trial?.previous));
            openApply(idx, `REVERT TO ${PRESETS[idx].name.toUpperCase()}`);
            break;
          }
          case "Refresh":
          case "CloseAfterburner":
            break;
          case "Keep":
            if (mockAdmin && trial?.status === "pending") {
              status = `${trial.preset} kept as your confirmed tune.`;
              confirmed = trial.preset;
              trial = null;
            }
            break;
          case "Dismiss":
            if (trial?.status === "reverted") trial = null;
            break;
        }
        break;
    }
  },
  tick() {
    sample();
    op.tick();
    return true;
  },
};
