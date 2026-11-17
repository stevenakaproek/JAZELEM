// The tab registry: sidebar order, icons, and which backends each page listens to.
import { CircuitBoard, Cpu, Fan as FanIcon, HeartPulse, Lightbulb, ListChecks, Rocket, ScrollText, Settings2, Trash2, Usb as UsbIcon, Zap, type LucideIcon } from "lucide-react";
import type { ComponentType } from "react";
import { Bloat } from "./Bloat";
import { Fan } from "./Fan";
import { Gpolicy } from "./Gpolicy";
import { Gpu } from "./Gpu";
import { Health } from "./Health";
import { Optimize } from "./Optimize";
import { Processes } from "./Processes";
import { Ramled } from "./Ramled";
import { Scheduler } from "./Scheduler";
import { Services } from "./Services";
import { Startup } from "./Startup";
import { Usb } from "./Usb";

export interface TabDef {
  id: string;
  title: string;
  icon: LucideIcon;
  /** Backends whose views this page shows (the app only builds views for these). */
  backends: string[];
  page: ComponentType;
}

export const TABS: TabDef[] = [
  { id: "services", title: "Services", icon: Settings2, backends: ["services"], page: Services },
  { id: "startup", title: "Startup Cleaner", icon: Rocket, backends: ["startup"], page: Startup },
  { id: "bloat", title: "Remove Bloat", icon: Trash2, backends: ["bloat"], page: Bloat },
  { id: "optimize", title: "Optimization", icon: Zap, backends: ["perf", "network", "tweaks"], page: Optimize },
  { id: "gpolicy", title: "Group Policy", icon: ScrollText, backends: ["gpolicy"], page: Gpolicy },
  { id: "scheduler", title: "CPU Scheduler", icon: Cpu, backends: ["scheduler"], page: Scheduler },
  { id: "procs", title: "Processes", icon: ListChecks, backends: ["procs"], page: Processes },
  { id: "health", title: "Health", icon: HeartPulse, backends: ["health"], page: Health },
  { id: "gpu", title: "RTX 5080 Profiles", icon: CircuitBoard, backends: ["gpu"], page: Gpu },
  { id: "fan", title: "Fan Tuner", icon: FanIcon, backends: ["fan"], page: Fan },
  { id: "ramled", title: "RAM LED", icon: Lightbulb, backends: ["ramled"], page: Ramled },
  { id: "usb", title: "USB Port Finder", icon: UsbIcon, backends: ["usb"], page: Usb },
];
