import type { Action, PolicyRow, View } from "../tabs/Gpolicy";
import { mockAdmin, mockParams } from "./env";
import { MockOp, makeOp, opItem } from "./op";
import type { MockTab } from "./types";

type Kind = "not_configured" | "enabled" | "disabled";
interface St {
  kind: Kind;
  vals: number[];
}
interface Opt {
  label: string;
  choices: [string, number][];
}
interface Policy {
  group: string;
  name: string;
  title: string;
  path: string;
  user?: boolean;
  advice: string;
  rec: St | null;
  opts: Opt[];
  cur: St;
  want: St | null;
  /** The value is in the registry although the policy is not configured. */
  inReg?: St;
  /** Saved in Group Policy, not applied by Windows yet. */
  notLive?: boolean;
}

const nc: St = { kind: "not_configured", vals: [] };
const on = (...vals: number[]): St => ({ kind: "enabled", vals });
const off: St = { kind: "disabled", vals: [] };
const hours: [string, number][] = Array.from({ length: 24 }, (_, h) => [`${h % 12 === 0 ? 12 : h % 12} ${h < 12 ? "AM" : "PM"}`, h]);

const G_AI = "AI & Copilot";
const G_SEARCH = "Widgets, Search & taskbar";
const G_PRIVACY = "Privacy & telemetry";
const G_TIPS = "Tips & promoted apps";
const G_UPDATE = "Windows Update";
const G_GAMING = "Gaming & background apps";
const G_OTHER = "Other";
const GROUPS = [G_AI, G_SEARCH, G_PRIVACY, G_TIPS, G_UPDATE, G_GAMING, G_OTHER];

const p = (group: string, name: string, title: string, path: string, advice: string, rec: St | null, cur: St, more: Partial<Policy> = {}): Policy => ({ group, name, title, path, advice, rec, cur, opts: [], want: null, ...more });

const policies: Policy[] = [
  p(G_AI, "Turn off Copilot in Windows", "Turn off Windows Copilot", "User > Windows Components > Windows Copilot", "Hides the Copilot sidebar and taskbar button. (The newer Copilot app is removed in Remove Bloat.)", on(), on(), { user: true }),
  p(G_AI, "Recall never saves snapshots", "Turn off saving snapshots for use with Recall", "Computer > Windows Components > Windows AI", "Recall never takes screenshots of what you do.", on(), on()),
  p(G_AI, "Recall cannot be switched on", "Allow Recall to be enabled", "Computer > Windows Components > Windows AI", "Disabled = the Recall feature is removed and cannot be turned on (Copilot+ PCs).", off, nc, { inReg: off }),
  p(G_SEARCH, "Widgets", "Allow widgets", "Computer > Windows Components > Widgets", "Disabled = no Widgets board and none of its background processes. Windows blocks scripts from this one; the policy file gets past that. Sign out to apply.", off, off, { notLive: true }),
  p(G_SEARCH, "No web results in Start search", "Don't search the web or display web results in Search", "Computer > Windows Components > Search", "Start search stays on your PC: apps, files and settings, no Bing.", on(), nc),
  p(G_SEARCH, "Search highlights", "Allow search highlights", "Computer > Windows Components > Search", "Disabled = no trending topics or doodles in the search box and Start search.", off, nc),
  p(G_SEARCH, "Chat (Teams) icon on the taskbar", "Configures the Chat icon on the taskbar", "Computer > Windows Components > Chat", "Enabled > Disabled removes the Chat icon from the taskbar for good.", on(3), nc, {
    opts: [{ label: "Icon", choices: [["Show", 1], ["Hide", 2], ["Disabled", 3]] }],
  }),
  p(G_PRIVACY, "Diagnostic data level", "Allow Diagnostic Data", "Computer > Windows Components > Data Collection and Preview Builds", "Required is the minimum Windows Pro allows ('Off' only works on Enterprise / Education; Pro treats it as Required).", on(1), on(3), {
    opts: [{ label: "Level", choices: [["Off (Enterprise / Education only)", 0], ["Required", 1], ["Optional", 3]] }],
  }),
  p(G_PRIVACY, "Turn off the advertising ID", "Turn off the advertising ID", "Computer > System > User Profiles", "Apps can't use an ID to track you for ads.", on(), on()),
  p(G_PRIVACY, "Clipboard sync across devices", "Allow Clipboard synchronization across devices", "Computer > System > OS Policies", "Disabled = your clipboard never syncs to the cloud. Leave it if you copy between PC and phone.", null, nc),
  p(G_TIPS, "No promoted apps", "Turn off Microsoft consumer experiences", "Computer > Windows Components > Cloud Content", "Stops Windows installing promoted apps and games. Windows Pro may ignore it (Enterprise / Education only).", on(), nc),
  p(G_TIPS, "Turn off Windows Spotlight", "Turn off all Windows spotlight features", "User > Windows Components > Cloud Content", "Enabled = no Spotlight pictures, facts and suggestions on the lock screen and desktop.", null, nc, { user: true }),
  p(G_UPDATE, "Keep your own drivers", "Do not include drivers with Windows Updates", "Computer > Windows Components > Windows Update > Manage updates offered from Windows Update", "Windows Update stops replacing your GPU / chipset drivers. Install those from NVIDIA / AMD / your board maker.", on(), on()),
  p(G_UPDATE, "No update restarts while you play", "Turn off auto-restart for updates during active hours", "Computer > Windows Components > Windows Update > Manage end user experience", "Windows never restarts for updates inside these hours (at most 18 hours).", on(8, 2), nc, {
    opts: [
      { label: "From", choices: hours },
      { label: "To", choices: hours },
    ],
  }),
  p(G_UPDATE, "Delivery Optimization", "Download Mode", "Computer > Windows Components > Delivery Optimization", "HTTP only = your PC never uploads updates to other PCs. Store / Game Pass downloads are unaffected.", on(0), nc, {
    opts: [{ label: "Mode", choices: [["HTTP only", 0], ["LAN", 1], ["Group", 2], ["Internet", 3], ["Simple", 99], ["Bypass", 100]] }],
  }),
  p(G_GAMING, "Game Bar recording", "Enables or disables Windows Game Recording and Broadcasting", "Computer > Windows Components > Windows Game Recording and Broadcasting", "Disabled = no Game Bar clips or background recording (saves a little performance). Leave it if you use Win+Alt+R.", null, nc),
  p(G_GAMING, "Windows Ink Workspace", "Allow Windows Ink Workspace", "Computer > Windows Components > Windows Ink Workspace", "Enabled > Disabled removes the pen workspace (no use without a pen).", on(0), nc, {
    opts: [{ label: "Ink", choices: [["Disabled", 0], ["On, not above lock", 1], ["On", 2]] }],
  }),
  p(G_OTHER, "Turn off AutoPlay", "Turn off Autoplay", "Computer > Windows Components > AutoPlay Policies", "Nothing pops up or runs by itself when a USB stick or disc goes in (also safer).", on(255), nc, {
    opts: [{ label: "On", choices: [["CD-ROM and removable drives", 181], ["All drives", 255]] }],
  }),
  p(G_OTHER, "OneDrive file sync", "Prevent the usage of OneDrive for file storage", "Computer > Windows Components > OneDrive", "Enabled = OneDrive can't sync at all. Don't use it if your Documents / Desktop live in OneDrive.", null, nc),
];

const same = (a: St, b: St) => a.kind === b.kind && a.vals.length === b.vals.length && a.vals.every((v, i) => v === b.vals[i]);

function label(pol: Policy, s: St): string {
  if (s.kind === "not_configured") return "Not configured";
  if (s.kind === "disabled") return "Disabled";
  if (s.vals.length === 0 || pol.opts.length === 0) return "Enabled";
  return `Enabled (${pol.opts.map((o, k) => o.choices.find((c) => c[1] === s.vals[k])?.[0] ?? "?").join(" - ")})`;
}

const defaults = (pol: Policy) => (pol.rec && pol.rec.vals.length === pol.opts.length ? [...pol.rec.vals] : pol.opts.map((o) => o.choices[0][1]));

/** What a right-click choice becomes: Enabled keeps the current drop-down values when already enabled. */
function setTarget(pol: Policy, s: St) {
  let t = s;
  if (s.kind === "enabled") t = pol.cur.kind === "enabled" ? pol.cur : on(...defaults(pol));
  pol.want = same(t, pol.cur) ? null : t;
}

const op = new MockOp();
let search = "";
let lastAction = "";
let lastBatch: { index: number; before: St }[] | null = null;
const state = mockParams.get("state");
const gpo = mockParams.get("edition") !== "home";

function row(pol: Policy, index: number): PolicyRow {
  const tgt = pol.want ?? pol.cur;
  const changed = pol.want !== null;
  const vals = tgt.vals.length === pol.opts.length ? tgt.vals : defaults(pol);
  let note: PolicyRow["note"] = null;
  if (!changed && pol.cur.kind === "not_configured" && pol.inReg) {
    note = { text: `registry: ${label(pol, pol.inReg)}`, color: "", tip: "Not set in Group Policy, but this value is already in the registry (set by a tweak or another tool). Setting it here makes it permanent and visible in gpedit." };
  } else if (!changed && pol.notLive && pol.cur.kind !== "not_configured") {
    note = { text: "not applied yet", color: "orange", tip: "Saved in Group Policy, but Windows has not applied it yet. Restart, or Apply again." };
  }
  return {
    index,
    name: pol.name,
    advice: pol.advice,
    tip: `gpedit: ${pol.user ? "User Configuration" : "Computer Configuration"}\n${pol.path} > "${pol.title}"`,
    changed,
    state: tgt.kind,
    current: label(pol, pol.cur),
    currentColor: pol.cur.kind === "not_configured" ? "" : "green",
    target: label(pol, tgt),
    note,
    rec: pol.rec ? (same(pol.rec, tgt) ? { text: "recommended", color: "accent" } : { text: `recommended: ${label(pol, pol.rec)}`, color: "" }) : { text: "your choice", color: "" },
    opts: tgt.kind === "enabled" ? pol.opts.map((o, k) => ({ label: o.label, value: vals[k], known: o.choices.some((c) => c[1] === vals[k]), choices: o.choices.map(([l, value]) => ({ label: l, value })) })) : [],  };
}

export const gpolicy: MockTab<View, Action> = {
  view() {
    const loaded = state !== "loading";
    const busy = op.op?.stage === "Running";
    const can = mockAdmin && !busy;
    const changes = policies.filter((x) => x.want).length;
    const q = state === "empty" ? "no such policy" : search.trim().toLowerCase();
    const groups = !loaded
      ? []
      : GROUPS.map((name) => ({
          name,
          rows: policies
            .map((pol, i) => ({ pol, i }))
            .filter(({ pol }) => pol.group === name && (q === "" || [pol.name, pol.title, pol.advice].some((s) => s.toLowerCase().includes(q))))
            .map(({ pol, i }) => row(pol, i)),
        })).filter((g) => g.rows.length > 0);
    return {
      loaded,
      gpo,
      edition: gpo ? "Professional" : "Core",
      loadError: state === "error" ? "Could not read the policy file C:\\Windows\\System32\\GroupPolicy\\Machine\\Registry.pol (Registry.pol is damaged). Showing the registry instead." : null,
      configured: policies.filter((x) => x.cur.kind !== "not_configured").length,
      total: policies.length,
      changes,
      canEdit: !busy,
      canApply: can && changes > 0,
      canDiscard: changes > 0 && !busy,
      canUndo: can && lastBatch !== null,
      canRefresh: !busy && loaded,
      working: busy || !loaded,
      lastAction,
      search: state === "empty" ? "no such policy" : search,
      groups,
      op: op.op,
      opCan: can,
    };
  },
  act(a) {
    switch (a.kind) {
      case "set":
        setTarget(policies[a.index], a.state === "enabled" ? on() : a.state === "disabled" ? off : nc);
        break;
      case "reset":
        policies[a.index].want = null;
        break;
      case "opt": {
        const pol = policies[a.index];
        const tgt = pol.want ?? pol.cur;
        if (tgt.kind !== "enabled") break;
        const vals = tgt.vals.length === pol.opts.length ? [...tgt.vals] : defaults(pol);
        vals[a.opt] = a.value;
        const s = on(...vals);
        pol.want = same(s, pol.cur) ? null : s;
        break;
      }
      case "search":
        search = a.text;
        break;
      case "apply": {
        const ch = policies.map((pol, index) => ({ pol, index })).filter(({ pol }) => pol.want);
        if (!mockAdmin || ch.length === 0) break;
        op.open(
          makeOp(
            "GROUP POLICY",
            "change",
            "changed",
            ch.map(({ pol, index }) => opItem(String(index), pol.name, pol.group, `${label(pol, pol.cur)} -> ${label(pol, pol.want ?? pol.cur)}`, pol.rec === null)),
            {
              noun: ["policy", "policies"],
              preview_text: gpo
                ? "Saved in the local Group Policy (the same place gpedit uses), then applied with gpupdate. You will see these in gpedit.msc."
                : "Windows Core has no Group Policy editor: the policy values are written straight to the registry.",
              reassure: "Every change goes into the history - 'Undo last change' puts it all back.",
              go_label: "Apply",
              running_text: gpo ? "Applying (gpupdate can take a minute)..." : "Applying...",
            },
          ),
          () => {
            lastBatch = ch.map(({ pol, index }) => ({ index, before: pol.cur }));
            ch.forEach(({ pol }) => {
              pol.cur = pol.want ?? pol.cur;
              pol.want = null;
              pol.inReg = undefined;
              pol.notLive = false;
            });
            lastAction = `${ch.length} policies changed.`;
            if (op.op) {
              op.op.can_undo = false;
              if (gpo) op.op.notes = ["Some policies (Widgets, Search) need a sign-out or restart to show."];
            }
          },
        );
        break;
      }
      case "discard":
        policies.forEach((pol) => (pol.want = null));
        lastAction = "";
        break;
      case "undo": {
        const batch = lastBatch;
        if (!mockAdmin || !batch) break;
        op.open(
          makeOp(
            "GROUP POLICY",
            "undo",
            "undone",
            batch.map(({ index }, i) => opItem(`u${i}`, `${policies[index].name}: ${label(policies[index], policies[index].cur)}`, "Undo")),
            { noun: ["policy", "policies"], preview_text: "Puts these policies back the way they were before 2026-11-16 20:15:22.", go_label: "Undo", running_text: "Undoing..." },
          ),
          () => {
            batch.forEach(({ index, before }) => (policies[index].cur = before));
            lastBatch = null;
            lastAction = "Last change undone.";
            if (op.op) op.op.can_undo = false;
          },
        );
        break;
      }
      case "refresh":
      case "gpedit":
        break;
      case "op":
        op.event(a.ev);
        break;
    }
  },
  tick: () => op.tick(),
};
