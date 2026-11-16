// The preview / progress / result sheet every change goes through (pctuner_core::opwin::OpWindow).
import { Check, ChevronRight, X } from "lucide-react";
import { useState } from "react";
import type { OpEvent, OpItem, OpWindow } from "../lib/types";
import { toneClass } from "../lib/types";
import { Badge, Banner, Button, Progress, Sheet, Spinner } from "./ui";

function plural(n: number, noun: [string, string]): string {
  return n === 1 ? noun[0] : noun[1];
}

function failed(it: OpItem): boolean {
  return it.result !== null && "Err" in it.result;
}

/** Items per group, in first-seen order: [label, total, failed]. */
function groupCounts(items: OpItem[]): [string, number, number][] {
  const out: [string, number, number][] = [];
  for (const it of items) {
    let g = out.find((x) => x[0] === it.group);
    if (!g) out.push((g = [it.group, 0, 0]));
    g[1] += 1;
    if (failed(it)) g[2] += 1;
  }
  return out;
}

function PreviewList({ op }: { op: OpWindow }) {
  const groups = groupCounts(op.items);
  const n = op.items.length;
  const single = groups.length === 1 && n <= 8;
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const line = (it: OpItem) => (
    <div key={it.id} className="op-line">
      <span className={it.optional ? "dot" : "dot accent"} />
      <span>{it.name}</span>
      {it.detail && <span className="secondary">{it.detail}</span>}
      {it.optional && <span className="tertiary">(optional)</span>}
    </div>
  );
  if (single) return <div className="op-list">{op.items.map(line)}</div>;
  return (
    <div className="op-list">
      {groups.map(([label, total]) => {
        const isOpen = open[label] ?? (groups.length === 1 || n <= 30);
        return (
          <div key={label}>
            <button type="button" className="op-group" aria-expanded={isOpen} onClick={() => setOpen({ ...open, [label]: !isOpen })}>
              <ChevronRight style={{ transform: isOpen ? "rotate(90deg)" : undefined }} />
              <strong>{label}</strong>
              <span className="secondary">{total}</span>
            </button>
            {isOpen && <div className="op-group-body">{op.items.filter((i) => i.group === label).map(line)}</div>}
          </div>
        );
      })}
    </div>
  );
}

export function OpSheet({ op, can, admin, onEvent }: { op: OpWindow; can: boolean; admin: boolean; onEvent: (ev: OpEvent) => void }) {
  const n = op.items.length;

  if (op.stage === "Preview") {
    return (
      <Sheet
        kicker={op.kicker}
        title={`Ready to ${op.verb} ${n} ${plural(n, op.noun)}`}
        subtitle={op.preview_text || undefined}
        onClose={() => onEvent("Close")}
        footer={
          <>
            <Button variant="primary" size="lg" disabled={!can} onClick={() => onEvent("Go")}>
              {op.go_label}
            </Button>
            <Button size="lg" onClick={() => onEvent("Close")}>
              Cancel
            </Button>
            {!admin && <span className="tone-error">View only - start PC Tuner as administrator.</span>}
          </>
        }
      >
        {op.warning && <Banner kind="warn">{op.warning}</Banner>}
        <PreviewList op={op} />
        {op.reassure && <p className="secondary">{op.reassure}</p>}
      </Sheet>
    );
  }

  if (op.stage === "Running") {
    const done = op.items.filter((i) => i.result !== null).length;
    const current = op.items.findIndex((i) => i.result === null);
    return (
      <Sheet
        kicker={op.kicker}
        title={
          <span className="hstack" style={{ gap: 10 }}>
            <Spinner large />
            {op.running_text}
          </span>
        }
      >
        {op.show_steps ? (
          <>
            <Progress value={op.progress ?? done / Math.max(1, n)} />
            <div className="op-list">
              {op.items.map((it, i) => (
                <div key={it.id} className="op-line">
                  {it.result === null ? i === current ? <Spinner /> : <span className="dot" /> : failed(it) ? <X className="op-mark tone-error" /> : <Check className="op-mark tone-ok" />}
                  <span className={it.result === null && i !== current ? "tertiary" : undefined} style={i === current ? { fontWeight: 600 } : undefined}>
                    {it.name}
                  </span>
                  {it.detail && <span className="tertiary">{it.detail}</span>}
                </div>
              ))}
            </div>
          </>
        ) : (
          <>
            <p className="secondary">
              {n} {plural(n, op.noun)}
            </p>
            <Progress value={null} />
          </>
        )}
      </Sheet>
    );
  }

  if (n === 0) {
    return (
      <Sheet
        kicker={op.kicker}
        title={
          <span className={op.empty_ok ? "hstack tone-ok" : "hstack"} style={{ gap: 10 }}>
            {op.empty_ok && <Check />}
            {op.empty_title}
          </span>
        }
        subtitle={op.empty_text}
        onClose={() => onEvent("Close")}
        footer={
          <Button variant="primary" size="lg" onClick={() => onEvent("Close")}>
            Close
          </Button>
        }
      />
    );
  }

  const okTotal = op.items.filter((i) => i.result !== null && "Ok" in i.result).length;
  const failedTotal = n - okTotal;
  const rows = [...op.items].sort((a, b) => Number(failed(b)) - Number(failed(a)));
  const groups = groupCounts(op.items);
  return (
    <Sheet
      kicker={op.kicker}
      title={
        <span className="op-result">
          <span className="hero-value tone-ok">{okTotal}</span>
          <span>
            <div>
              {plural(okTotal, op.noun)} {op.done_word}
            </div>
            {failedTotal > 0 && (
              <div className="tone-error" style={{ fontSize: "var(--text-base)", fontWeight: 500 }}>
                {failedTotal} {op.fail_phrase}
              </div>
            )}
          </span>
        </span>
      }
      onClose={() => onEvent("Close")}
      footer={
        <>
          <Button variant="primary" size="lg" onClick={() => onEvent("Close")}>
            Close
          </Button>
          {op.extras.map((label, i) => (
            <Button key={i} size="lg" disabled={!can} onClick={() => onEvent({ Extra: i })}>
              {label}
            </Button>
          ))}
          {op.can_undo && (
            <Button size="lg" disabled={!can} onClick={() => onEvent("Undo")}>
              {op.undo_label}
            </Button>
          )}
        </>
      }
    >
      <div className="op-list">
        {rows.map((it) => {
          const bad = failed(it);
          const text = it.result === null ? "" : "Err" in it.result ? it.result.Err : it.result.Ok;
          return (
            <div key={it.id}>
              <div className="op-line">
                {bad ? <X className="op-mark tone-error" /> : <Check className="op-mark tone-ok" />}
                <span style={bad ? { fontWeight: 600 } : undefined}>{it.name}</span>
                {it.detail && <span className="secondary">{it.detail}</span>}
                <span className="tertiary" style={{ fontSize: "var(--text-xs)" }}>
                  {it.group}
                </span>
              </div>
              {text && <div className={bad ? "op-note tone-error selectable" : "op-note tone-warn selectable"}>{text}</div>}
            </div>
          );
        })}
      </div>
      {groups.length > 1 && (
        <div className="hstack" style={{ flexWrap: "wrap" }}>
          {groups.map(([label, total, bad]) => (
            <Badge key={label} color={bad === 0 ? "green" : "orange"}>
              {label} {total - bad}/{total}
            </Badge>
          ))}
        </div>
      )}
      {op.footer && <p className={toneClass(op.footer[1])}>{op.footer[0]}</p>}
      {op.notes.map((line, i) => (
        <p key={i} className="secondary selectable">
          {line}
        </p>
      ))}
    </Sheet>
  );
}
