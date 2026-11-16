// The tab registry: sidebar order, icons, and which backends each page listens to.
import { CircuitBoard, Cpu, Fan, Lightbulb, Rocket, ScrollText, Settings2, Trash2, Usb, Zap, type LucideIcon } from "lucide-react";
import type { ComponentType } from "react";
import { ComingSoon } from "./ComingSoon";
import { Fan as FanPage } from "./Fan";
import { Optimize } from "./Optimize";

export interface TabDef {
  id: string;
  title: string;
  icon: LucideIcon;
  /** Backends whose views this page shows (the app only builds views for these). */
  backends: string[];
  page: ComponentType;
}

const soon = (title: string) => () => <ComingSoon title={title} />;

export const TABS: TabDef[] = [
  { id: "services", title: "Services", icon: Settings2, backends: ["services"], page: soon("Services") },
  { id: "startup", title: "Startup Cleaner", icon: Rocket, backends: ["startup"], page: soon("Startup Cleaner") },
  { id: "bloat", title: "Remove Bloat", icon: Trash2, backends: ["bloat"], page: soon("Remove Bloat") },
  { id: "optimize", title: "Optimization", icon: Zap, backends: ["perf", "network", "tweaks"], page: Optimize },
  { id: "gpolicy", title: "Group Policy", icon: ScrollText, backends: ["gpolicy"], page: soon("Group Policy") },
  { id: "scheduler", title: "CPU Scheduler", icon: Cpu, backends: ["scheduler"], page: soon("CPU Scheduler") },
  { id: "gpu", title: "RTX 5080 Profiles", icon: CircuitBoard, backends: ["gpu"], page: soon("RTX 5080 Profiles") },
  { id: "fan", title: "Fan Tuner", icon: Fan, backends: ["fan"], page: FanPage },
  { id: "ramled", title: "RAM LED", icon: Lightbulb, backends: ["ramled"], page: soon("RAM LED") },
  { id: "usb", title: "USB Port Finder", icon: Usb, backends: ["usb"], page: soon("USB Port Finder") },
];
