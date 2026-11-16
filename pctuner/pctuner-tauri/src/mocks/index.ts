// Stand-in data for running the page in a normal browser (npm run dev, no Tauri).
import type { ShellInfo } from "../lib/api";
import { mockAdmin, mockParams } from "./env";
import { power } from "./power";
import type { MockTab } from "./types";

export const shell: ShellInfo = { admin: mockAdmin, version: "1.3.2", closePending: mockParams.has("closing"), notice: mockParams.get("notice") };

export const tabs: Record<string, MockTab> = {
  power: power as MockTab,
};
