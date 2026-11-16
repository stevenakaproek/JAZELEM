import type { Tone } from "../lib/types";
import type { Action, UsbHit, UsbMap, View } from "../tabs/Usb";
import { mockAdmin, mockParams } from "./env";
import type { MockTab } from "./types";

// "?state=loading | error | empty" shows the other faces of the tab
const state = mockParams.get("state");

const CPU = "AMD USB 3.10 eXtensible Host Controller - 1.20 (Microsoft)";
const CPU2 = "AMD USB 2.0 eXtensible Host Controller - 1.20 (Microsoft)";
const USB4 = "ASMedia USB 3.20 eXtensible Host Controller - 1.20 (Microsoft)";
const CHIP = "AMD USB 3.20 eXtensible Host Controller - 1.20 (Microsoft)";

const tone = (t: number): Tone => (t === 1 ? "Ok" : t === 2 ? "Accent" : t === 3 ? "Warn" : t === 4 ? "Junk" : "Dim");
const SHORT = ["", "Tier 1  ·  CPU", "Tier 2  ·  CPU lanes", "Tier 3  ·  Chipset", "Tier 4  ·  2nd chipset"];
const LONG = ["", "Tier 1  CPU (built into the CPU)", "Tier 2  CPU lanes (via add-on USB4 chip)", "Tier 3  Chipset", "Tier 4  Second chipset (furthest)"];

type Dev = [tier: number, name: string, kind: string, controller: string, port: string, hubs: number, onboard: boolean, id: string, functions: string];

const devs: Dev[] = [
  [1, "Xbox Wireless Adapter for Windows", "GAME CONTROLLER", CPU, "Port_#0003.Hub_#0002", 0, false, "USB\\VID_045E&PID_02FE\\000000000001", "Xbox Wireless Adapter for Windows"],
  [1, "Razer Viper V3 Pro", "mouse", CPU, "Port_#0001.Hub_#0002", 0, false, "USB\\VID_1532&PID_00C1\\000000000000", "USB Input Device, HID-compliant mouse"],
  [3, "Arctis Nova Pro Wireless", "AUDIO", CHIP, "Port_#0002.Hub_#0005", 1, false, "USB\\VID_1038&PID_12E0\\5&2A1B3C4D&0&2", "USB Audio Device, USB Input Device"],
  [3, "Wooting 60HE", "keyboard", CHIP, "Port_#0005.Hub_#0004", 0, false, "USB\\VID_31E3&PID_1310\\A02B2301W031H00123", "USB Input Device, HID Keyboard Device"],
  [3, "MYSTIC LIGHT", "", CHIP, "Port_#0011.Hub_#0004", 0, true, "USB\\VID_0DB0&PID_0076\\A02024100900", "USB Input Device"],
  [3, "Qualcomm FastConnect 7800 Bluetooth Adapter", "bluetooth", CHIP, "Port_#0013.Hub_#0004", 0, true, "USB\\VID_0489&PID_E10A\\6&1F2E3D4C&0&13", ""],
];

const controllers: [tier: number, name: string, id: string, path: string][] = [
  [1, CPU2, "PCI\\VEN_1022&DEV_15B8&SUBSYS_7E511462&REV_00\\4&2B5C0E7A&0&0041", "PCIROOT(0)#PCI(0801)#PCI(0005)"],
  [1, CPU, "PCI\\VEN_1022&DEV_15B6&SUBSYS_7E511462&REV_00\\4&2B5C0E7A&0&0341", "PCIROOT(0)#PCI(0801)#PCI(0003)"],
  [2, USB4, "PCI\\VEN_1B21&DEV_2426&SUBSYS_7E511462&REV_00\\8&1A2B3C4D&0&00000019", "PCIROOT(0)#PCI(0103)#PCI(0000)#PCI(0000)#PCI(0000)"],
  [2, "USB4(TM) Host Router (Microsoft)", "PCI\\VEN_1B21&DEV_2425&SUBSYS_7E511462&REV_00\\8&1A2B3C4D&0&00100019", "PCIROOT(0)#PCI(0103)#PCI(0000)#PCI(0200)#PCI(0000)"],
  [3, CHIP, "PCI\\VEN_1022&DEV_43FD&SUBSYS_11421B21&REV_01\\6&3A4B5C6D&0&00600011", "PCIROOT(0)#PCI(0201)#PCI(0000)#PCI(0C00)#PCI(0000)"],
];

// controllers switched off in the stand-in data (the 2.0 controller has nothing plugged in)
const off = new Set<string>();
let switchNote: View["switchNote"] = null;

const key = (d: Dev) => d[2] === "GAME CONTROLLER" || d[2] === "AUDIO";
const verdict = (d: Dev) => (d[0] === 1 ? "already on a CPU port - nothing to do" : d[0] === 2 ? "on CPU lanes - fine" : "on a chipset port - move it to a Tier 1 port if one is free") + (d[5] > 0 ? "; it is going through a hub - plug it into the PC directly" : "");

function mapView(): UsbMap {
  const list = state === "empty" ? devs.filter((d) => !key(d)) : devs;
  return {
    time: "21:47:03",
    key: list.filter((d) => key(d) && !d[6]).map((d) => ({ tier: `Tier ${d[0]}`, tone: tone(d[0]), name: d[1], verdict: verdict(d) })),
    controllers: controllers.map(([tier, name, id, path]) => {
      const n = list.filter((d) => d[3] === name).length;
      const router = name.includes("Host Router");
      const disabled = off.has(id);
      const whyNot = disabled ? null : router ? "A USB4 router carries devices that show under another controller - it is never disabled." : n > 0 ? "Something is plugged into it. Only controllers with no devices can be disabled." : null;
      return {
        id,
        tier: SHORT[tier],
        tone: tone(tier),
        name,
        router,
        note: disabled ? "disabled" : router ? "USB4 router - devices on these ports appear under the add-on chip's USB controller" : n === 1 ? "1 device" : `${n} devices`,
        empty: n === 0,
        disabled,
        canDisable: mockAdmin && !disabled && whyNot === null,
        canEnable: mockAdmin && disabled,
        whyNot,
        hover: `${id}\n${path}`,
      };
    }),
    devices: list.map((d) => ({ tier: `Tier ${d[0]}`, tone: tone(d[0]), name: d[1], kind: d[2].toLowerCase(), keyKind: key(d), hubs: d[5] > 0 ? `via ${d[5]} hub(s)` : null, onboard: d[6], where: `${d[3]}  ·  ${d[4]}`, hover: `${d[7]}\n${d[8]}` })),
  };
}

let watching = false;
let ticks = 0;
let secs = 0;
let log: (UsbHit & { age: number })[] = [];
// the ports a pretend flash drive is moved through while the test runs
const tour: [tier: number, controller: string, port: string, hubs: number][] = [
  [3, CHIP, "Port_#0002.Hub_#0005", 1],
  [1, CPU, "Port_#0003.Hub_#0002", 0],
  [2, USB4, "Port_#0001.Hub_#0007", 0],
];

export const usb: MockTab<View, Action> = {
  view() {
    const loading = state === "loading";
    const failed = state === "error";
    const m = loading || failed ? null : mapView();
    const keyed = m?.key ?? [];
    const worst = Math.max(0, ...keyed.map((k) => Number(k.tier.slice(5))));
    return {
      scanning: loading,
      scanSecs: secs,
      error: failed ? "Get-PnpDevice returned no devices." : null,
      summary: m && {
        hero: keyed.length
          ? { value: `Tier ${worst}`, unit: "controller and headset", caption: "the slowest one is on a chipset port - move it to a Tier 1 port if one is free  ·  one goes through a hub", tone: tone(worst) }
          : { value: "-", unit: "no controller or headset on USB", caption: "A Bluetooth controller does not use a USB port - only the Bluetooth adapter does.", tone: "Dim" },
        stats: [
          ...keyed.slice(0, 2).map((k) => ({ value: k.tier, label: k.name.length > 24 ? `${k.name.slice(0, 21).trimEnd()}...` : k.name, tone: k.tone })),
          { value: String(m.controllers.filter((c) => !c.router).length), label: "USB controllers", tone: "Text" as Tone },
          { value: String(m.devices.length), label: "USB devices", tone: "Text" as Tone },
        ],
      },
      map: m,
      watch: watching ? (ticks < 3 ? "Taking a snapshot of the connected devices..." : ticks % 8 === 7 ? "New device found - reading its port... keep it plugged in" : "Watching for new devices...") : null,
      tested: log.length ? `${log.length} port(s) tested` : null,
      watchError: null,
      log: [...log].reverse().map(({ age, ...l }) => ({ ...l, fresh: age < 5 })),
      switching: null,
      switchNote,
      admin: mockAdmin,
    };
  },
  act(a) {
    if (a.kind === "start_watch") {
      watching = true;
      ticks = 0;
      log = [];
    } else if (a.kind === "stop_watch") watching = false;
    else if (a.kind === "dismiss_note") switchNote = null;
    else if (a.kind === "controller" && mockAdmin) {
      const name = controllers.find((c) => c[2] === a.id)?.[1] ?? "?";
      if (a.enable) off.delete(a.id);
      else off.add(a.id);
      switchNote = { ok: true, text: `${name} ${a.enable ? "enabled" : "disabled"}.` };
    }
  },
  tick() {
    secs = Math.floor((ticks + 1) / 2);
    log.forEach((l) => (l.age += 1));
    if (!watching) return state === "loading" || log.some((l) => l.age === 5);
    ticks += 1;
    if (ticks % 8 === 0 && log.length < tour.length) {
      const [tier, controller, port, hubs] = tour[log.length];
      log.push({ n: log.length + 1, name: "SanDisk Ultra", tier: `this port = ${LONG[tier]}`, tone: tone(tier), detail: `${controller}  ·  ${port}${hubs > 0 ? `  ·  through ${hubs} hub(s) - a front-panel or monitor port` : ""}`, fresh: true, age: 0 });
    }
    return true;
  },
};
