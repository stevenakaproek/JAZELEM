// A pretend operation window for the browser mocks: preview -> running (one item per tick) -> done.
import type { OpEvent, OpItem, OpWindow } from "../lib/types";

export function opItem(id: string, name: string, group: string, detail = "", optional = false): OpItem {
  return { id, name, group, detail, optional, result: null };
}

export function makeOp(kicker: string, verb: string, doneWord: string, items: OpItem[], more: Partial<OpWindow> = {}): OpWindow {
  return {
    kicker,
    verb,
    done_word: doneWord,
    noun: ["item", "items"],
    preview_text: "",
    running_text: "Working...",
    warning: null,
    reassure: "",
    go_label: "Apply",
    stage: "Preview",
    items,
    show_steps: true,
    notes: [],
    footer: null,
    extras: [],
    can_undo: false,
    undo_label: "Undo",
    empty_title: "Nothing to do",
    empty_text: "",
    empty_ok: true,
    fail_phrase: "could not be changed",
    progress: null,
    ...more,
  };
}

/** Holds one optional operation window and plays it through when "Go" is pressed. */
export class MockOp {
  op: OpWindow | null = null;
  private onDone: (() => void) | null = null;

  open(op: OpWindow, onDone?: () => void) {
    this.op = op;
    this.onDone = onDone ?? null;
  }

  /** Returns true when the event was "Undo" or an extra button (the tab decides what those do). */
  event(ev: OpEvent): "undo" | "extra" | null {
    if (!this.op) return null;
    if (ev === "Go") this.op.stage = "Running";
    else if (ev === "Close") this.op = null;
    else if (ev === "Undo") return "undo";
    else return "extra";
    return null;
  }

  /** Advance a running operation by one item. Returns true when something changed. */
  tick(): boolean {
    const op = this.op;
    if (!op || op.stage !== "Running") return false;
    const next = op.items.find((i) => i.result === null);
    if (next) {
      // the third item of a long run fails, to show what a failure looks like
      next.result = op.items.length > 3 && op.items.indexOf(next) === 2 ? { Err: "Access is denied." } : { Ok: "" };
      return true;
    }
    op.stage = "Done";
    op.can_undo = true;
    this.onDone?.();
    this.onDone = null;
    return true;
  }
}
