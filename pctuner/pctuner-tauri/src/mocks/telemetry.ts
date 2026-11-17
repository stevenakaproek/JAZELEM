import type { TeleItem, Telemetry } from "../lib/api";

let t = 0;
const it = (key: string, label: string, unit: string, value: number, digits: number, max: number | null): TeleItem => ({ key, label, unit, text: value.toFixed(digits), value, max });

/** Stand-in readings for the browser preview (the numbers in the user's OCCT screenshot, gently moving). */
export function sample(): Telemetry {
  t += 1;
  const w = Math.sin(t / 5);
  return {
    groups: [
      { kind: "cpu", name: "AMD Ryzen 7 9800X3D", items: [it("temp", "Temperature", "°C", 51.3 + w * 2, 1, 100), it("volt", "Core voltage", "V", 1.26, 2, null), it("power", "Package power", "W", 64.8 + w * 6, 1, 170), it("load", "Load", "%", 19.1 + w * 5, 1, 100), it("clock", "Clock", "MHz", 3778 + Math.round(w * 40), 0, 5600)] },
      { kind: "memory", name: "Memory", items: [it("used_pct", "In use", "%", 41.7 + w, 1, 100), it("used", "In use", "MB", 13444 + Math.round(w * 200), 0, 32000)] },
      { kind: "gpu", name: "NVIDIA GeForce RTX 5080", items: [it("temp", "Temperature", "°C", 37.5 + w, 1, 100), it("volt", "Core voltage", "V", 1.02, 2, null), it("power", "Power draw", "W", 84.5 + w * 8, 1, 360), it("load", "Load", "%", 44 + w * 6, 1, 100), it("clock", "Core clock", "MHz", 2962, 0, 3210), it("vram", "Memory used", "MB", 4459, 0, 16303)] },
    ],
  };
}
