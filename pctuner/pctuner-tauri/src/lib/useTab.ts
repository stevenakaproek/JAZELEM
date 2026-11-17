import { useCallback, useEffect, useRef, useState } from "react";
import { onTab, tabAction, tabState } from "./api";

export interface TabHandle<V, A> {
  /** The latest view, or null while the first one loads. */
  view: V | null;
  /** The backend could not be reached at all. */
  error: string | null;
  act: (action: A) => Promise<void>;
}

/** Subscribe to one tab backend: its first view, later updates, and a way to send actions. */
export function useTab<V, A = unknown>(tab: string): TabHandle<V, A> {
  const [view, setView] = useState<V | null>(null);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    const off = onTab<V>(tab, (v) => alive.current && setView(v));
    tabState<V>(tab)
      .then((v) => alive.current && setView(v))
      .catch((e) => alive.current && setError(String(e)));
    return () => {
      alive.current = false;
      off();
    };
  }, [tab]);

  const act = useCallback(
    async (action: A) => {
      try {
        const v = await tabAction<V>(tab, action);
        if (alive.current) setView(v);
      } catch (e) {
        if (alive.current) setError(String(e));
      }
    },
    [tab],
  );

  return { view, error, act };
}
