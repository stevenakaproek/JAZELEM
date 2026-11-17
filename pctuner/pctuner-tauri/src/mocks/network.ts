import type { NetworkAction, NetworkRow, NetworkView } from "../tabs/Network";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import { mockBackups, restoreOp } from "./backups";
import type { MockTab } from "./types";

type Seed = [name: string, group: string, why: string, optional: boolean, now: string, target: string, set: boolean, applicable?: boolean];

const POWER = "Adapter power saving";
const LAT = "Adapter latency";
const WIN = "Windows network stack";
const OPT = "Optional";
const ADAPTER = "Ethernet";

// the board's Realtek 5GbE adapter, partly tuned
const seeds: Seed[] = [
  ["Energy-Efficient Ethernet", POWER, "Stops the link dozing between packets, which adds a wake-up delay to the next one.", false, "On", "Off", false],
  ["Advanced EEE", POWER, "Realtek's extra power saving on top of EEE.", false, "Off", "Off", true],
  ["Green Ethernet", POWER, "Lowers transmit power on short cables. No benefit on a desktop.", false, "On", "Off", false],
  ["Power Saving Mode", POWER, "Lets the adapter slow down when traffic is light - exactly the pattern of game packets.", false, "On", "Off", false],
  ["Gigabit Lite", POWER, "Drops the link to 500 Mbps to save power.", false, "Off", "Off", true],
  ["Selective Suspend", POWER, "Lets Windows put the adapter to sleep when it looks idle.", false, "", "Off", false, false],
  ["Interrupt Moderation", LAT, "Packets reach Windows at once instead of being batched. Saves well under a millisecond and costs a little CPU - the 9800X3D has plenty.", false, "On", "Off", false],
  ["Flow Control", LAT, "Stops the router or switch from pausing your adapter. Pauses are rare at home; off is the usual gaming setting.", false, "On (3)", "Off", false],
  ["Network throttling (MMCSS)", WIN, "Windows caps non-media packets while audio or video plays. Off removes the cap. Small effect, no downside.", false, "10 packets/ms", "Off", false],
  ["TCP delayed ACK", WIN, "Acknowledges every TCP packet at once. TCP only: Halo's match traffic is UDP, so this helps chat and menus, not hit registration.", false, "Every 2nd packet (default)", "Every packet", false],
  ["Nagle's algorithm", WIN, "Sends small TCP packets straight away instead of bundling them. TCP only, same note as above.", false, "On (default)", "Off", false],
  ["Tag Halo Infinite as game traffic", OPT, "Marks HaloInfinite.exe packets DSCP 46 (high priority). Only helps if your router prioritises DSCP - most home routers ignore it.", true, "Not tagged", "DSCP 46", false],
  ["UDP receive coalescing (URO)", OPT, "Windows 11 can merge incoming UDP packets to save CPU. No measured effect on games either way - an A/B test.", true, "On", "Off", false],
  ["Wake on magic packet", OPT, "The PC can no longer be woken over the network. No speed effect - just less running.", true, "On", "Off", false],
  ["Wake on pattern match", OPT, "Stops ordinary network traffic waking the PC from sleep.", true, "On", "Off", false],
  ["Shutdown Wake-on-LAN", OPT, "Keeps the network port powered while the PC is off. Off saves standby power.", true, "Off", "Off", true],
];

// rows 0-7 and 13-15 are adapter properties: changing one restarts the adapter
const isAdapterSetting = (index: number) => index <= 7 || index >= 13;

const rows: NetworkRow[] = seeds.map(([name, , why, optional, now, target, set, applicable = true], index) => ({
  index,
  name,
  why,
  optional,
  applicable,
  set,
  selectable: applicable && !set,
  selected: false,
  now,
  target,
  canSwitch: false,
}));
/** what a switched-off row shows (the stand-in's "value from before") */
const offText = seeds.map(([, , , , now, , set]) => (set ? (now === "Off" ? "On" : "Driver default") : now));

const op = new MockOp();
const state = mockParams.get("state");
/** The last applied change, for "Undo last change". */
let last: { index: number; before: string }[] = [];

const restartWarning = `"${ADAPTER}" restarts at the end: your connection drops for about 5 seconds. Don't do this during a match.`;

function undo() {
  const items = [...last].reverse();
  op.open(
    makeOp(
      "UNDO NETWORK CHANGE",
      "undo",
      "undone",
      items.map((c) => opItem(String(c.index), rows[c.index].name, seeds[c.index][1], `${rows[c.index].target} -> ${c.before}`, rows[c.index].optional)),
      {
        noun: ["setting", "settings"],
        preview_text: `Adapter: ${ADAPTER}. Each change is saved so it can be undone.`,
        running_text: "Putting the old settings back...",
        go_label: "Undo",
        reassure: "Restores the values from before that change.",
        warning: items.some((c) => isAdapterSetting(c.index)) ? restartWarning : null,
      },
    ),
    () => {
      items.forEach((c) => {
        const r = rows[c.index];
        r.set = false;
        r.selectable = true;
        r.now = c.before;
      });
      last = [];
    },
  );
}

const bk = mockBackups(
  "A backup saves every network setting in this list. Restore puts back only the settings that differ, with the usual preview, and can be undone. Adapter settings are only restored on the adapter the backup was made on.",
  () => rows.filter((r) => r.applicable).map((r) => ({ id: String(r.index), title: r.name, value: r.now, shown: r.now })),
  (diffs) =>
    restoreOp(op, "RESTORE NETWORK BACKUP", diffs, (d) => {
      const r = rows[Number(d.backup.id)];
      r.now = d.backup.shown;
      r.set = r.now === r.target;
      r.selectable = r.applicable && !r.set;
    }),
  (it) => ({ ...it, value: it.value === "Off" ? "On" : "Off", shown: it.value === "Off" ? "On" : "Off" }),
);

export const network: MockTab<NetworkView, NetworkAction> = {
  view() {
    const pending = rows.filter((r) => r.selected).length;
    const failed = state === "error" || state === "empty";
    return {
      loading: state === "loading",
      refreshing: false,
      error: state === "error" ? "Could not read the network settings: PowerShell returned no data (exit code 1)." : state === "empty" ? "No connected network adapter was found." : null,
      groups: failed ? null : [POWER, LAT, WIN, OPT].map((name) => ({ name, rows: rows.filter((r) => seeds[r.index][1] === name).map((r) => ({ ...r, canSwitch: mockAdmin && r.applicable && !op.op })) })),
      pending: failed ? 0 : pending,
      canApply: mockAdmin && !failed && pending > 0 && op.op?.stage !== "Running",
      canUndo: mockAdmin && last.length > 0 && op.op?.stage !== "Running",
      op: op.op,
      opCan: mockAdmin && op.op?.stage !== "Running",
      backups: bk.view(mockAdmin && !op.op),
    };
  },
  act(a) {
    if (bk.act(a)) return;
    switch (a.kind) {
      case "switch": {
        const r = rows[a.index];
        if (!mockAdmin || op.op || !r.applicable || r.set === a.on) break;
        const from = r.now;
        const to = a.on ? r.target : offText[a.index];
        op.open(
          makeOp(a.on ? "NETWORK" : "SWITCH OFF", "change", "changed", [opItem(String(r.index), r.name, "Setting", `${from} -> ${to}`)], {
            noun: ["setting", "settings"],
            go_label: a.on ? "Apply" : "Switch off",
            preview_text: a.on ? "Each change is saved so it can be undone." : "Puts back the value from before PC Tuner changed it (or the Windows default). Saved, so \"Undo last change\" switches it on again.",
            running_text: "Applying...",
          }),
          () => {
            r.set = a.on;
            r.now = to;
            r.selected = false;
            r.selectable = r.applicable && !r.set;
          },
        );
        break;
      }
      case "toggle":        if (rows[a.index]?.selectable) rows[a.index].selected = !rows[a.index].selected;
        break;
      case "clear":
        rows.forEach((r) => (r.selected = false));
        break;
      case "apply": {
        const picked = rows.filter((r) => r.selected);
        if (!mockAdmin || picked.length === 0) break;
        op.open(
          makeOp(
            "NETWORK",
            "change",
            "changed",
            picked.map((r) => opItem(String(r.index), r.name, seeds[r.index][1], `${r.now} -> ${r.target}`, r.optional)),
            {
              noun: ["setting", "settings"],
              preview_text: `Adapter: ${ADAPTER}. Each change is saved so it can be undone.`,
              running_text: "Applying network settings...",
              reassure: '"Undo last change" puts every value back.',
              warning: picked.some((r) => isAdapterSetting(r.index)) ? restartWarning : null,
            },
          ),
          () => {
            last = picked.map((r) => ({ index: r.index, before: r.now }));
            picked.forEach((r) => {
              r.set = true;
              r.selected = false;
              r.selectable = false;
              r.now = r.target;
            });
            if (op.op && picked.some((r) => isAdapterSetting(r.index))) op.op.notes = [`"${ADAPTER}" was restarted and is back online.`];
          },
        );
        break;
      }
      case "undo":
        if (mockAdmin && last.length > 0) undo();
        break;
      case "refresh":
        break;
      case "op":
        if (op.event(a.ev) === "undo" && last.length > 0) {
          undo();
          op.event("Go");
        }
        break;
    }
  },
  tick: () => op.tick(),
};
