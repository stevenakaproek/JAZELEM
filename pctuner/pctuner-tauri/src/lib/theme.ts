// Light / Dark theme: the colours come from tokens.css by data-theme on <html>; the choice is remembered
// in the webview's storage and the window (title bar, Mica) is told to match.
import { useState } from "react";
import { inTauri } from "./api";

export type Theme = "light" | "dark";
const KEY = "pctuner.theme";

export function savedTheme(): Theme {
  try {
    return localStorage.getItem(KEY) === "light" ? "light" : "dark";
  } catch {
    return "dark";
  }
}

export function applyTheme(t: Theme) {
  document.documentElement.dataset.theme = t;
  if (inTauri) {
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().setTheme(t)).catch(() => {});
  }
}

export function useTheme(): [Theme, (t: Theme) => void] {
  const [theme, setTheme] = useState<Theme>(savedTheme);
  const set = (t: Theme) => {
    try {
      localStorage.setItem(KEY, t);
    } catch {
      /* not remembered, still applied */
    }
    applyTheme(t);
    setTheme(t);
  };
  return [theme, set];
}
