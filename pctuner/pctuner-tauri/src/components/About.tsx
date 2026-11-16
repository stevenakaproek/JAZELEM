// The description that opens under a Services / Startup Cleaner row when it is right-clicked:
// what the item is, in plain words, and what switching it off means.
import { Info } from "lucide-react";
import { useState } from "react";

/** pctuner_core::explain::About */
export interface About {
  what: string;
  off: string;
  /** Windows' own description, when it says more */
  windows: string;
}

/** Which rows are open (right-click opens / closes a row; several can be open). */
export function useOpenRows() {
  const [open, setOpen] = useState<Set<string>>(new Set());
  const toggle = (id: string) =>
    setOpen((o) => {
      const n = new Set(o);
      if (!n.delete(id)) n.add(id);
      return n;
    });
  return { open, toggle };
}

/** A full-width table row under the item. */
export function AboutRow({ about, cols }: { about: About; cols: number }) {
  return (
    <tr className="about-row">
      <td colSpan={cols}>
        <div className="about">
          <Info className="about-icon" aria-hidden />
          <div className="about-body selectable">
            <p className="about-what">{about.what}</p>
            <p className="about-off">
              <span className="about-label">If you switch it off: </span>
              {about.off}
            </p>
            {about.windows && (
              <p className="about-windows">
                <span className="about-label">Windows' description: </span>
                {about.windows}
              </p>
            )}
          </div>
        </div>
      </td>
    </tr>
  );
}

export const ABOUT_HINT = "Right-click a row to see what it is.";
