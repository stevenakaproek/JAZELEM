import type { RamledAction, RamledView } from "../tabs/Ramled";
import { mockAdmin, mockParams } from "./env";
import type { MockTab } from "./types";

// two G.SKILL Trident Z5 RGB sticks in slots A2 / B2 of the MSI MAG X870E TOMAHAWK WIFI
type Mode = "off" | "rainbow" | "static";
const state = mockParams.get("state");
const sticks = state === "empty" ? [] : [{ name: "AUDA0-E6K5-0101", addr: 0x71 }, { name: "AUDA0-E6K5-0101", addr: 0x73 }];
let mode: Mode = "rainbow";
let lastOn: { mode: Mode; shown: string } = { mode: "rainbow", shown: "" };
let color = "#ffffff";
let shown = "#ffffff";
let remember = true;
let status = "";
let scanning = state === "loading" ? Infinity : 2;
let job: { ticks: number; done: () => void } | null = null;

function describe(): string {
  if (mode === "off") return "Off";
  if (mode === "rainbow") return "Rainbow";
  return shown === "#000000" ? "Off (black)" : `Static colour ${shown.toUpperCase()}`;
}

function start(what: string, done: () => void) {
  status = what;
  job = {
    ticks: 2,
    done: () => {
      done();
      if (mode !== "off" && !(mode === "static" && shown === "#000000")) lastOn = { mode, shown };
      status = remember ? "Done, and saved in the RAM: it stays like this after a restart." : "Done. Not saved: the LEDs go back to their stored setting at the next power-on.";
    },
  };
}

export const ramled: MockTab<RamledView, RamledAction> = {
  view() {
    const busy = mockAdmin && (scanning > 0 || job !== null);
    const found = mockAdmin && scanning <= 0 && sticks.length > 0;
    const lit = found && mode !== "off" && !(mode === "static" && shown === "#000000");
    const error = state === "error" && mockAdmin && scanning <= 0 ? "The PawnIO driver is not installed." : null;
    const hero: RamledView["hero"] = !mockAdmin
      ? { value: "-", caption: "Start PC Tuner as administrator to read the RAM LEDs.", tone: "Faint" }
      : scanning > 0
        ? { value: "-", caption: "Looking for RAM LED controllers...", tone: "Faint" }
        : !found
          ? { value: "-", caption: "No supported RAM LED controller found.", tone: "Faint" }
          : lit
            ? { value: "On", caption: "The RAM LEDs are lit.", tone: "Accent" }
            : { value: "Off", caption: "The RAM LEDs are dark.", tone: "Dim" };
    const can = mockAdmin && !busy && found;
    return {
      hero,
      busy,
      found,
      lit,
      count: found ? sticks.length : 0,
      canRescan: mockAdmin && !busy,
      error,
      can,
      canColor: can,
      colorUnsupported: false,
      color,
      remember,
      status,
      sticks: found ? sticks.map((s, i) => ({ title: `Stick ${i + 1}`, state: describe(), lit, detail: `8 LEDs  ·  controller ${s.name}  ·  SMBus address 0x${s.addr.toString(16).toUpperCase()}` })) : [],
    };
  },
  act(a) {
    if (a.kind === "remember") {
      remember = a.on;
      return;
    }
    if (!mockAdmin || job || scanning > 0) return;
    switch (a.kind) {
      case "rescan":
        status = "";
        scanning = 2;
        break;
      case "off":
        start("Turning the RAM LEDs off...", () => (mode = "off"));
        break;
      case "on":
        start("Turning the RAM LEDs on...", () => {
          mode = lastOn.mode;
          shown = lastOn.shown || shown;
        });
        break;
      case "rainbow":
        start("Setting rainbow...", () => (mode = "rainbow"));
        break;
      case "pick":
        color = a.color;
        break;
      case "set_color":
        start("Setting the colour...", () => {
          mode = "static";
          shown = color;
        });
        break;
    }
  },
  tick() {
    if (!mockAdmin) return false;
    if (scanning > 0) {
      scanning -= 1;
      return true;
    }
    if (job) {
      job.ticks -= 1;
      if (job.ticks <= 0) {
        job.done();
        job = null;
      }
      return true;
    }
    return false;
  },
};
