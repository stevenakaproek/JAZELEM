// Types shared by several tabs (they mirror pctuner-core's serialised types).

export type Tone = "Ok" | "Accent" | "Text" | "Dim" | "Faint" | "Off" | "Warn" | "Junk" | "Error";

export function toneClass(t: Tone): string {
  return `tone-${t.toLowerCase()}`;
}

/** The badge colour that goes with a tone. */
export function toneBadge(t: Tone): "green" | "accent" | "yellow" | "orange" | "red" | "" {
  switch (t) {
    case "Ok":
      return "green";
    case "Accent":
      return "accent";
    case "Warn":
      return "yellow";
    case "Junk":
      return "orange";
    case "Error":
      return "red";
    default:
      return "";
  }
}

export type OpStage = "Preview" | "Running" | "Done";

export interface OpItem {
  id: string;
  name: string;
  group: string;
  detail: string;
  optional: boolean;
  result: null | { Ok: string } | { Err: string };
}

/** pctuner_core::opwin::OpWindow: the preview / progress / result pop-up of every change. */
export interface OpWindow {
  kicker: string;
  verb: string;
  done_word: string;
  noun: [string, string];
  preview_text: string;
  running_text: string;
  warning: string | null;
  reassure: string;
  go_label: string;
  stage: OpStage;
  items: OpItem[];
  show_steps: boolean;
  notes: string[];
  footer: [string, Tone] | null;
  extras: string[];
  can_undo: boolean;
  undo_label: string;
  empty_title: string;
  empty_text: string;
  empty_ok: boolean;
  fail_phrase: string;
  progress: number | null;
}

/** pctuner_core::opwin::OpEvent */
export type OpEvent = "Go" | "Close" | "Undo" | { Extra: number };
