import type { PowerAction, PowerView } from "../tabs/Power";
import { mockAdmin } from "./env";
import type { MockTab } from "./types";

let stage: PowerView["stage"] = { kind: "choose" };
let since = 0;

export const power: MockTab<PowerView, PowerAction> = {
  view: () => ({
    stage,
    choices: [
      { id: "RestartToBios", title: "Restart into BIOS", text: "Windows restarts and opens your motherboard's BIOS (UEFI) setup straight away." },
      { id: "ShutdownToBios", title: "Shut down, BIOS on next start", text: "The PC turns off. The next time you press the power button it goes straight into the BIOS." },
    ],
    blocked: mockAdmin ? null : "Run PC Tuner as administrator to use this.",
    canPick: mockAdmin,
    checking: false,
    error: null,
  }),
  act(a) {
    if (a.kind === "open" || a.kind === "back") stage = { kind: "choose" };
    if (a.kind === "pick") {
      stage = { kind: "working", title: a.choice === "RestartToBios" ? "Restarting into the BIOS..." : "Shutting down..." };
      since = Date.now();
    }
  },
  tick() {
    if (stage.kind === "working" && Date.now() - since > 4000) {
      stage = { kind: "timed_out", title: "Windows did not restart" };
      return true;
    }
    return false;
  },
};
