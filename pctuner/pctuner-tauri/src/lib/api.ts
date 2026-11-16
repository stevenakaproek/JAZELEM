// The one place the page talks to the app. Inside Tauri these are real commands and events;
// in a normal browser they are answered by the mocks in src/mocks, so every tab can be worked on there.
import type { MockTab } from "../mocks/types";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface ShellInfo {
  admin: boolean;
  version: string;
  closePending: boolean;
  notice: string | null;
  /** Self-test: seconds per tab (the page visits every tab, then the app quits). */
  selftest?: number | null;
}

export type ShellAction =
  | { kind: "active"; tabs: string[] }
  | { kind: "reset_gpu" }
  | { kind: "dismiss_notice" }
  | { kind: "keep_open" }
  | { kind: "close_anyway" }
  | { kind: "log"; text: string }
  | { kind: "selftest_done" };

type Unlisten = () => void;

// ---- mock side (browser) ----

let mocks: Promise<{ tabs: Record<string, MockTab>; shell: ShellInfo }> | null = null;
function loadMocks() {
  mocks ??= import("../mocks");
  return mocks;
}
const mockListeners = new Map<string, Set<(v: unknown) => void>>();
function mockEmit(event: string, payload: unknown) {
  mockListeners.get(event)?.forEach((cb) => cb(structuredClone(payload)));
}
function mockListen(event: string, cb: (v: unknown) => void): Unlisten {
  let set = mockListeners.get(event);
  if (!set) mockListeners.set(event, (set = new Set()));
  set.add(cb);
  return () => set.delete(cb);
}
if (!inTauri && typeof window !== "undefined") {
  // live mock data (fan speeds, running operations) moves once a second
  setInterval(async () => {
    const m = await loadMocks();
    for (const [name, tab] of Object.entries(m.tabs)) {
      if (tab.tick?.() && mockListeners.has(`tab:${name}`)) mockEmit(`tab:${name}`, tab.view());
    }
  }, 500);
}

// ---- the API ----

export async function tabState<T>(tab: string): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("tab_state", { tab });
  }
  const m = await loadMocks();
  const t = m.tabs[tab];
  if (!t) throw new Error(`no mock for tab ${tab}`);
  return structuredClone(t.view()) as T;
}

/** Send an action; resolves with the tab's view after it. */
export async function tabAction<T>(tab: string, action: unknown): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("tab_action", { tab, action });
  }
  const m = await loadMocks();
  const t = m.tabs[tab];
  if (!t) throw new Error(`no mock for tab ${tab}`);
  t.act(action as never);
  return structuredClone(t.view()) as T;
}

export function onTab<T>(tab: string, cb: (view: T) => void): Unlisten {
  if (!inTauri) return mockListen(`tab:${tab}`, cb as (v: unknown) => void);
  let off: Unlisten | null = null;
  let cancelled = false;
  import("@tauri-apps/api/event").then(({ listen }) =>
    listen<T>(`tab:${tab}`, (e) => cb(e.payload)).then((u) => {
      if (cancelled) u();
      else off = u;
    }),
  );
  return () => {
    cancelled = true;
    off?.();
  };
}

export async function shellInfo(): Promise<ShellInfo> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<ShellInfo>("shell_info");
  }
  return structuredClone((await loadMocks()).shell);
}

export async function shellAction(action: ShellAction): Promise<void> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke("shell_action", { action });
  }
  const m = await loadMocks();
  if (action.kind === "dismiss_notice") m.shell.notice = null;
  if (action.kind === "keep_open" || action.kind === "close_anyway") m.shell.closePending = false;
  if (action.kind === "reset_gpu") m.tabs.gpu?.act({ kind: "click", click: "Reset" } as never);
  mockEmit("shell", m.shell);
}

export function onShell(cb: (info: ShellInfo) => void): Unlisten {
  if (!inTauri) return mockListen("shell", cb as (v: unknown) => void);
  let off: Unlisten | null = null;
  let cancelled = false;
  import("@tauri-apps/api/event").then(({ listen }) =>
    listen<ShellInfo>("shell", (e) => cb(e.payload)).then((u) => {
      if (cancelled) u();
      else off = u;
    }),
  );
  return () => {
    cancelled = true;
    off?.();
  };
}
