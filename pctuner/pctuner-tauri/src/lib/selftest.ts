// Self-test (PCTUNER_SELFTEST=<seconds per tab>): visit every tab in the real app, write what was
// rendered and any script error to the app's log, then ask the app to quit. Used to check builds.
import { shellAction } from "./api";

const log = (text: string) => shellAction({ kind: "log", text }).catch(() => {});
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

function describe(label: string) {
  const page = document.querySelector<HTMLElement>(".page");
  const text = (page?.innerText ?? "").replace(/\s+/g, " ").trim();
  const errors = document.querySelectorAll(".state.error").length;
  const loading = document.querySelectorAll(".state .spinner").length;
  void log(`${label}: ${text.length} chars, ${errors} error state(s), ${loading} loading state(s) | ${text.slice(0, 140)}`);
}

export async function runSelftest(secs: number, tabs: string[], go: (id: string) => void) {
  window.addEventListener("error", (e) => void log(`script error: ${e.message} (${e.filename}:${e.lineno})`));
  window.addEventListener("unhandledrejection", (e) => void log(`unhandled rejection: ${String(e.reason)}`));
  for (const id of tabs) {
    go(id);
    await wait(secs * 1000);
    describe(id);
    // parts of a page (Optimization: Performance / Network / Windows tweaks)
    const parts = document.querySelectorAll<HTMLButtonElement>(".page-inner > div > .segmented button");
    for (let i = 1; i < parts.length; i++) {
      document.querySelectorAll<HTMLButtonElement>(".page-inner > div > .segmented button")[i]?.click();
      await wait(secs * 1000);
      describe(`${id} part ${i + 1}`);
    }
  }
  await shellAction({ kind: "selftest_done" });
}
