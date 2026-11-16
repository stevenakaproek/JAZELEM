/** A tab's stand-in for the browser: the same view the Rust backend would send, and its actions. */
export interface MockTab<V = unknown, A = unknown> {
  view(): V;
  act(action: A): void;
  /** Called twice a second; return true when the view changed (live data, running work). */
  tick?(): boolean;
}
