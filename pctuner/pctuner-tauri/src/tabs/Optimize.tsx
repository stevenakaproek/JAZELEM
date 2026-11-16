// Optimization: Performance, Network and Windows tweaks in one place, one part each.
import { useState } from "react";
import { EmptyState, Page, Segmented, ViewOnlyBanner } from "../components/ui";
import { useShell } from "../lib/shell";
import { Performance } from "./Performance";

type Part = "performance" | "network" | "windows";

const PARTS: { value: Part; label: string }[] = [
  { value: "performance", label: "Performance" },
  { value: "network", label: "Network" },
  { value: "windows", label: "Windows tweaks" },
];

/** "?part=network" in the address opens that part (used for screenshots). */
function initialPart(): Part {
  const p = new URLSearchParams(location.search).get("part");
  return PARTS.some((x) => x.value === p) ? (p as Part) : "performance";
}

export function Optimize() {
  const { admin } = useShell();
  const [part, setPart] = useState<Part>(initialPart);
  return (
    <Page title="Optimization" subtitle="Click the settings you want, then apply them. Every change shows a preview first and can be undone.">
      <ViewOnlyBanner admin={admin} />
      <div>
        <Segmented value={part} options={PARTS} onChange={setPart} />
      </div>
      {part === "performance" && <Performance />}
      {part === "network" && <EmptyState title="Not rebuilt yet" text="Network is still being moved to the new interface." />}
      {part === "windows" && <EmptyState title="Not rebuilt yet" text="Windows tweaks are still being moved to the new interface." />}
    </Page>
  );
}
