import type { Action, GpuLive, GpuTrial, View } from "../tabs/Gpu";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import type { MockTab } from "./types";

const PRESETS = [
  { name: "Stock", slot: 3, mem: 0, text: "Factory behaviour - no undervolt, no memory overclock. Use this to reset or to compare." },
  { name: "Efficiency", slot: 2, mem: 1000, text: "Coolest and quietest. 2700 MHz at 900 mV, memory +1000." },
  { name: "Balanced", slot: 1, mem: 1500, text: "Everyday gaming. 2800 MHz at 925 mV, memory +1500." },
  { name: "Performance", slot: 4, mem: 2000, text: "Most fps. 2900 MHz at 950 mV, memory +2000. Runs warmest of the three undervolts." },
];

// "?state=trial | unguarded | reverted | loading | error | empty" shows the other faces of the tab
const state = mockParams.get("state");
const op = new MockOp();
let current = 2;
let confirmed = "Balanced";
let selected: number | null = 2;
const slotOk = [true, true, true, false];
let status = "";
let error: string | null = state === "error" ? "Could not apply Performance:\nAfterburner has not saved this card's voltage/frequency curve yet. Open Afterburner, leave every setting at stock, click Save and then any profile slot (1-5), close Afterburner and retry." : null;
let trial: { preset: string; previous: string; status: "pending" | "reverted" } | null =
  state === "trial" || state === "unguarded" ? { preset: "Performance", previous: "Balanced", status: "pending" } : state === "reverted" ? { preset: "Performance", previous: "Balanced", status: "reverted" } : null;
if (trial?.status === "pending") current = 3;
if (trial?.status === "reverted") current = 0;

const hist: { core: number; temp: number; power: number; util: number }[] = [];
let t = 0;
function sample() {
  t += 1;
  const wave = Math.sin(t / 7);
  hist.push({ core: 2790 + Math.round(wave * 12), temp: 61 + Math.round(wave * 3), power: 232 + wave * 18, util: 93 + Math.round(wave * 5) });
  if (hist.length > 60) hist.shift();
}
for (let i = 0; i < 40; i++) sample();

function live(): GpuLive {
  const l = hist[hist.length - 1];
  return { ...l, coreHist: hist.map((h) => h.core), tempHist: hist.map((h) => h.temp), powerHist: hist.map((h) => h.power), utilHist: hist.map((h) => h.util) };
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
  const p = PRESETS[idx];
  const write = !slotOk[idx];
  const startsTrial = idx !== 0 && confirmed !== p.name;
  const endsTrial = !startsTrial && trial?.status === "pending";
  const items = [];
  if (write) items.push(opItem("write", `Write preset into slot ${p.slot} (backup first)`, ""));
  if (startsTrial) items.push(opItem("trial", "Arm the safety trial (auto-revert after a crash)", ""));
  items.push(opItem("start", "Start MSI Afterburner with the preset", "", `-Profile${p.slot}`));
  items.push(opItem("confirm", "Confirm on the GPU (live memory offset)", ""));
  if (endsTrial) items.push(opItem("endtrial", "End the safety trial (remove the sign-in check)", ""));
  const warn: string[] = [];
  if (write) warn.push(`Afterburner slot ${p.slot} does not hold the ${p.name} preset yet. PC Tuner closes Afterburner, backs up its profile file to C:\\Users\\e\\AppData\\Local\\PCTuner\\gpu-backups and rewrites the slot.`);
  if (p.name === "Performance") warn.push("Runs warmest - test stability before long sessions.");
  op.open(
    makeOp(kicker, "run", "done", items, {
      noun: ["step", "steps"],
      fail_phrase: "did not complete",
      preview_text: `${p.name}: ${p.text}`,
      running_text: `Applying ${p.name}...`,
      go_label: idx === 0 ? "Reset to stock" : `Apply ${p.name}`,
      warning: warn.length ? warn.join("\n") : null,
      reassure: startsTrial
        ? `Starts as a trial: Keep it or Revert to ${confirmed} afterwards. After a crash the GPU goes back to Stock at your next sign-in.`
        : idx === 0
          ? "Stock is the factory behaviour: no undervolt, no memory overclock."
          : "This is the preset you already confirmed.",
    }),
    () => {
      current = idx;
      slotOk[idx] = true;
      trial = startsTrial ? { preset: p.name, previous: confirmed, status: "pending" } : null;
      if (op.op) op.op.can_undo = false;
      if (startsTrial) op.op?.notes.push("Trial running - use Keep or Revert at the top of this tab once you have tested it.");
    },
  );
}

export const gpu: MockTab<View, Action> = {
  view() {
    const mem = PRESETS[current].mem;
    const noCfg = state === "empty";
    return {
      trial: trialView(),
      hero:
        state === "loading"
          ? null
          : {
              name: noCfg ? "Not managed" : PRESETS[current].name,
              unit: noCfg ? "by Afterburner" : "preset",
              caption: noCfg ? "No Afterburner profile - the card runs its factory settings (confirmed under load)" : `Afterburner saved settings: ${PRESETS[current].name}`,
              tone: noCfg ? "Off" : "Accent",
              chips: noCfg
                ? [
                    { text: "Live check: GPU idle in P8 - confirms under load", tone: "Off" },
                    { text: "Afterburner not running - presets only hold while it runs", tone: "Junk" },
                  ]
                : [
                    { text: `Live check: memory offset matches ${PRESETS[current].name}`, tone: "Ok" },
                    { text: "Afterburner running", tone: "Ok" },
                    { text: `Memory ${15000 + mem} MHz (${mem >= 0 ? "+" : ""}${mem} over stock 15000)`, tone: "Dim" },
                  ],
              noCfg: noCfg ? "No RTX 5080 profile file (VEN_10DE&DEV_2C02*.cfg) in C:\\Program Files (x86)\\MSI Afterburner\\Profiles." : null,
              live: live(),
            },
      canReset: op.op?.stage !== "Running",
      presets: PRESETS.map((p, index) => ({ index, name: p.name, slot: p.slot, text: p.text, selected: selected === index, current: !noCfg && current === index, needsWrite: !noCfg && !slotOk[index] })),
      applyLabel: selected === null ? "Apply preset" : `Apply ${PRESETS[selected].name}`,
      canApply: selected !== null,
      canRestore: true,
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
            if (selected !== null) openApply(selected, `APPLY ${PRESETS[selected].name.toUpperCase()}`);
            break;
          case "Revert": {
            const idx = Math.max(0, PRESETS.findIndex((p) => p.name === trial?.previous));
            openApply(idx, `REVERT TO ${PRESETS[idx].name.toUpperCase()}`);
            break;
          }
          case "Restore":
            op.open(
              makeOp(
                "RESTORE LAST BACKUP",
                "run",
                "done",
                [opItem("copy", "Close Afterburner and put the backup back", "", "20261111-214512-VEN_10DE&DEV_2C02&SUBSYS_53521458&REV_A1&BUS_1&DEV_0&FN_0.cfg"), opItem("start", "Restart MSI Afterburner", "")],
                {
                  noun: ["step", "steps"],
                  fail_phrase: "did not complete",
                  preview_text: "Puts back the Afterburner profile file saved on 2026-11-11 21:45:12.",
                  running_text: "Restoring the backup...",
                  go_label: "Restore",
                  warning: "All Afterburner profile slots go back to what they held when this backup was made.",
                },
              ),
              () => {
                if (op.op) op.op.can_undo = false;
              },
            );
            break;
          case "Refresh":
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
