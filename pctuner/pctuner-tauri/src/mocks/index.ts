// Stand-in data for running the page in a normal browser (npm run dev, no Tauri).
import type { ShellInfo } from "../lib/api";
import { bloat } from "./bloat";
import { mockAdmin, mockParams } from "./env";
import { fan } from "./fan";
import { gpolicy } from "./gpolicy";
import { gpu } from "./gpu";
import { network } from "./network";
import { perf } from "./perf";
import { power } from "./power";
import { ramled } from "./ramled";
import { scheduler } from "./scheduler";
import { services } from "./services";
import { startup } from "./startup";
import { tweaks } from "./tweaks";
import type { MockTab } from "./types";
import { usb } from "./usb";

export const shell: ShellInfo = { admin: mockAdmin, version: "1.3.2", closePending: mockParams.has("closing"), notice: mockParams.get("notice") };

export const tabs: Record<string, MockTab> = {
  bloat: bloat as MockTab,
  fan: fan as MockTab,
  gpolicy: gpolicy as MockTab,
  gpu: gpu as MockTab,
  network: network as MockTab,
  perf: perf as MockTab,
  power: power as MockTab,
  ramled: ramled as MockTab,
  scheduler: scheduler as MockTab,
  services: services as MockTab,
  startup: startup as MockTab,
  tweaks: tweaks as MockTab,
  usb: usb as MockTab,
};
