import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { onShell, shellInfo, type ShellInfo } from "./api";

const fallback: ShellInfo = { admin: false, version: "", closePending: false, notice: null };
const ShellContext = createContext<ShellInfo>(fallback);

/** App-wide facts: administrator or view-only, the close guard, background notices. */
export function ShellProvider({ children }: { children: ReactNode }) {
  const [info, setInfo] = useState<ShellInfo>(fallback);
  useEffect(() => {
    let alive = true;
    shellInfo().then((i) => alive && setInfo(i));
    const off = onShell((i) => alive && setInfo(i));
    return () => {
      alive = false;
      off();
    };
  }, []);
  return <ShellContext.Provider value={info}>{children}</ShellContext.Provider>;
}

export function useShell(): ShellInfo {
  return useContext(ShellContext);
}
