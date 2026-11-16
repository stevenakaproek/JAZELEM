import type { LedSection, LedTarget, RamledAction, RamledView } from "../tabs/Ramled";
import { mockAdmin, mockParams } from "./env";
import type { MockTab } from "./types";

// two G.SKILL Trident Z5 RGB sticks in slots A2 / B2 of the MSI MAG X870E TOMAHAWK WIFI, and the
// Gigabyte RTX 5080 XTREME WATERFORCE (its controller cannot be read back: unknown until set)
// "?state=empty | loading | error | gpuerror" shows the other faces of the tab
type Mode = "off" | "rainbow" | "static" | "unknown";
const state = mockParams.get("state");

function makeSection(target: LedTarget) {
  const gpu = target === "gpu";
  const devices = state === "empty" ? [] : gpu ? [{ name: "Gigabyte RGB Fusion 2 (Blackwell)", addr: 0x75 }] : [{ name: "AUDA0-E6K5-0101", addr: 0x71 }, { name: "AUDA0-E6K5-0101", addr: 0x73 }];
  let mode: Mode = gpu ? "unknown" : "rainbow";
  let lastOn: { mode: Mode; shown: string } = { mode: "rainbow", shown: "" };
  let color = "#ffffff";
  let shown = "#ffffff";
  let remember = true;
  let status = "";
  let scanning = state === "loading" ? Infinity : gpu ? 3 : 2;
  let job: { ticks: number; done: () => void } | null = null;
  const what = gpu ? "graphics card LEDs" : "RAM LEDs";

  const describe = () => (mode === "unknown" ? "Not set by PC Tuner yet" : mode === "off" ? "Off" : mode === "rainbow" ? "Rainbow" : shown === "#000000" ? "Off (black)" : `Static colour ${shown.toUpperCase()}`);

  function start(text: string, done: () => void) {
    status = text;
    job = {
      ticks: 2,
      done: () => {
        done();
        if (mode !== "off" && mode !== "unknown" && !(mode === "static" && shown === "#000000")) lastOn = { mode, shown };
        status = remember
          ? `Done, and saved in the ${gpu ? "graphics card" : "RAM"}: it stays like this after a restart.`
          : "Done. Not saved: the LEDs go back to their stored setting at the next power-on.";
      },
    };
  }

  return {
    view(): LedSection {
      const busy = mockAdmin && (scanning > 0 || job !== null);
      const gpuError = gpu && state === "gpuerror";
      const found = mockAdmin && scanning <= 0 && devices.length > 0 && !gpuError;
      const lit = found && mode !== "off" && mode !== "unknown" && !(mode === "static" && shown === "#000000");
      const error =
        state === "error" && mockAdmin && scanning <= 0 && !gpu
          ? "The PawnIO driver is not installed."
          : gpuError && scanning <= 0
            ? "The Gigabyte RTX 5080's LED controller did not answer (NVIDIA driver status -1). If RGB Fusion or Gigabyte Control Center is running, close it and try again."
            : null;
      const hero: LedSection["hero"] = !mockAdmin
        ? { value: "-", caption: gpu ? "Start PC Tuner as administrator to reach the graphics card's LEDs." : "Start PC Tuner as administrator to read the RAM LEDs.", tone: "Faint" }
        : scanning > 0
          ? { value: "-", caption: gpu ? "Looking for the graphics card's LED controller..." : "Looking for RAM LED controllers...", tone: "Faint" }
          : !found
            ? { value: "-", caption: gpu ? "No supported graphics card LED controller found." : "No supported RAM LED controller found.", tone: "Faint" }
            : mode === "unknown"
              ? { value: "Found", caption: "The controller cannot be read back: the lighting is shown once PC Tuner has set it.", tone: "Dim" }
              : lit
                ? { value: "On", caption: `The ${what} are lit.`, tone: "Accent" }
                : { value: "Off", caption: `The ${what} are dark.`, tone: "Dim" };
      const can = mockAdmin && !busy && found;
      return {
        hero,
        busy,
        found,
        lit,
        count: found ? devices.length : 0,
        canRescan: mockAdmin && !busy,
        error,
        can,
        canColor: can,
        colorUnsupported: false,
        color,
        remember,
        status,
        sticks: found
          ? devices.map((d, i) => ({
              title: gpu ? "Graphics card" : `Stick ${i + 1}`,
              state: describe(),
              lit,
              detail: gpu ? `3 zones  ·  ${d.name}  ·  I2C address 0x75` : `8 LEDs  ·  controller ${d.name}  ·  SMBus address 0x${d.addr.toString(16).toUpperCase()}`,
            }))
          : [],
      };
    },
    act(a: RamledAction) {
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
          start(`Turning the ${what} off...`, () => (mode = "off"));
          break;
        case "on":
          start(`Turning the ${what} on...`, () => {
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
    tick(): boolean {
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
}

const ram = makeSection("ram");
const gpu = makeSection("gpu");

export const ramled: MockTab<RamledView, RamledAction> = {
  view: () => ({ ...ram.view(), gpu: gpu.view() }),
  act: (a) => (a.target === "gpu" ? gpu : ram).act(a),
  tick: () => {
    const r = ram.tick();
    const g = gpu.tick();
    return r || g;
  },
};
