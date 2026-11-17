// PC Tuner's own small component set. Styling lives in styles/app.css; values come from styles/tokens.css.
import { AlertTriangle, Check as CheckIcon, CircleAlert, CircleCheck, Info, Lock, Search as SearchIcon } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { Tone } from "../lib/types";
import { toneBadge, toneClass } from "../lib/types";

export function cx(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

// ---------- page ----------

export function Page({ title, subtitle, actions, children }: { title: string; subtitle?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="page">
      <div className="page-inner">
        <header className="page-header">
          <div>
            <h1>{title}</h1>
            {subtitle && <p>{subtitle}</p>}
          </div>
          {actions && <div className="actions">{actions}</div>}
        </header>
        {children}
      </div>
    </div>
  );
}

// ---------- grouped inset list ----------

export function Group({ title, count, footer, children, padded, style }: { title?: ReactNode; count?: ReactNode; footer?: ReactNode; children: ReactNode; padded?: boolean; style?: CSSProperties }) {
  return (
    <section className="group-wrap" style={style}>
      {title && (
        <div className="group-title">
          <span>{title}</span>
          {count !== undefined && <span className="count">{count}</span>}
        </div>
      )}
      <div className={cx("group", padded && "padded")}>{children}</div>
      {footer && <div className="group-footer">{footer}</div>}
    </section>
  );
}

export interface RowProps {
  title: ReactNode;
  subtitle?: ReactNode;
  icon?: ReactNode;
  /** Shows the round selection mark; "none" keeps its space empty (aligned with selectable rows); undefined = no mark. */
  checked?: boolean | "none";
  value?: ReactNode;
  accessory?: ReactNode;
  selected?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  title2?: string;
}

export function Row({ title, subtitle, icon, checked, value, accessory, selected, disabled, onClick, onContextMenu, title2 }: RowProps) {
  // a clickable row is a div with the button role (not a <button>), so an accessory can hold its own
  // controls (a Switch, a Button); clicks on those do not also click the row
  const click = disabled ? undefined : onClick;
  return (
    <div
      className={cx("row", onClick && !disabled && "clickable", (selected || checked === true) && "selected", disabled && "disabled")}
      onClick={click}
      onContextMenu={onContextMenu}
      title={title2}
      {...(onClick
        ? {
            role: "button",
            tabIndex: disabled ? -1 : 0,
            "aria-pressed": checked === true,
            "aria-disabled": disabled || undefined,
            onKeyDown: (e: React.KeyboardEvent) => {
              if (click && e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
                e.preventDefault();
                click();
              }
            },
          }
        : {})}
    >
      {checked === "none" ? <span className="check-space" /> : checked !== undefined && <Check on={checked} />}
      {icon && <span className="row-icon">{icon}</span>}
      <span className="row-main">
        <div className="row-title">{title}</div>
        {subtitle && <div className="row-sub">{subtitle}</div>}
      </span>
      {value !== undefined && <span className="row-value">{value}</span>}
      {accessory && (
        <span
          className="row-accessory"
          onClick={(e) => {
            // its own buttons and switches act alone (a plain badge still clicks the row)
            if ((e.target as HTMLElement).closest("button, a, input, select, [role=switch]")) e.stopPropagation();
          }}
        >
          {accessory}
        </span>
      )}
    </div>
  );
}

export function Check({ on }: { on: boolean }) {
  return (
    <span className={cx("check", on && "on")} aria-hidden>
      <CheckIcon strokeWidth={3} />
    </span>
  );
}

// ---------- controls ----------

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "default" | "primary" | "danger" | "danger-filled" | "plain";
  size?: "md" | "lg";
  block?: boolean;
  icon?: ReactNode;
}

export function Button({ variant = "default", size = "md", block, icon, className, children, ...rest }: ButtonProps) {
  return (
    <button
      type="button"
      className={cx("btn", variant === "primary" && "primary", variant === "danger" && "danger", variant === "danger-filled" && "danger filled", variant === "plain" && "plain", size === "lg" && "lg", block && "block", className)}
      {...rest}
    >
      {icon}
      {children}
    </button>
  );
}

export function Switch({ on, onChange, disabled, label }: { on: boolean; onChange: (on: boolean) => void; disabled?: boolean; label?: string }) {
  return <button type="button" role="switch" aria-checked={on} aria-label={label} disabled={disabled} className={cx("switch", on && "on")} onClick={() => onChange(!on)} />;
}

export function Segmented<T extends string | number>({ value, options, onChange, disabled }: { value: T; options: { value: T; label: ReactNode; disabled?: boolean }[]; onChange: (v: T) => void; disabled?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const [thumb, setThumb] = useState<{ x: number; w: number } | null>(null);
  const index = options.findIndex((o) => o.value === value);
  useLayoutEffect(() => {
    const el = ref.current?.querySelectorAll("button")[index];
    if (el) setThumb({ x: el.offsetLeft - 2, w: el.offsetWidth });
    else setThumb(null);
  }, [index, options.length]);
  return (
    <div className="segmented" role="tablist" ref={ref}>
      {thumb && <span className="thumb" style={{ transform: `translateX(${thumb.x}px)`, width: thumb.w }} />}
      {options.map((o) => (
        <button key={String(o.value)} type="button" role="tab" aria-selected={o.value === value} disabled={disabled || o.disabled} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Slider({ value, min, max, step = 1, onChange, onCommit, disabled, label }: { value: number; min: number; max: number; step?: number; onChange: (v: number) => void; onCommit?: (v: number) => void; disabled?: boolean; label?: string }) {
  const pct = max > min ? ((value - min) / (max - min)) * 100 : 0;
  return (
    <input
      className="slider"
      type="range"
      aria-label={label}
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      style={{ "--pct": `${pct}%` } as CSSProperties}
      onChange={(e) => onChange(Number(e.target.value))}
      onPointerUp={(e) => onCommit?.(Number((e.target as HTMLInputElement).value))}
      onKeyUp={(e) => onCommit?.(Number((e.target as HTMLInputElement).value))}
    />
  );
}

/** A number field that sends its value when the user leaves it or presses Enter. */
export function NumberField({ value, min, max, onCommit, disabled, suffix, label }: { value: number; min: number; max: number; onCommit: (v: number) => void; disabled?: boolean; suffix?: string; label?: string }) {
  const [text, setText] = useState(String(value));
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText(String(value));
  }, [value]);
  const commit = () => {
    editing.current = false;
    const n = Number(text);
    if (text.trim() !== "" && Number.isFinite(n)) {
      const v = Math.min(max, Math.max(min, n));
      setText(String(v));
      if (v !== value) onCommit(v);
    } else setText(String(value));
  };
  return (
    <span className="hstack" style={{ gap: 4 }}>
      <input
        className="input number"
        aria-label={label}
        inputMode="decimal"
        value={text}
        disabled={disabled}
        onFocus={(e) => {
          editing.current = true;
          e.target.select();
        }}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
      {suffix && <span className="secondary">{suffix}</span>}
    </span>
  );
}

export function SearchField({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
  return (
    <label className="search">
      <SearchIcon />
      <input className="input" type="search" value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} spellCheck={false} />
    </label>
  );
}

// ---------- status ----------

export type BadgeColor = "green" | "orange" | "yellow" | "red" | "accent" | "";

export function Badge({ color = "", children, icon, title }: { color?: BadgeColor; children: ReactNode; icon?: ReactNode; title?: string }) {
  return (
    <span className={cx("badge", color)} title={title}>
      {icon}
      {children}
    </span>
  );
}

export function ToneBadge({ tone, children }: { tone: Tone; children: ReactNode }) {
  return <Badge color={toneBadge(tone)}>{children}</Badge>;
}

export function ToneText({ tone, children, className }: { tone: Tone; children: ReactNode; className?: string }) {
  return <span className={cx(toneClass(tone), className)}>{children}</span>;
}

export function Dot({ color = "" }: { color?: BadgeColor }) {
  return <span className={cx("dot", color)} />;
}

export function Banner({ kind = "info", children, action }: { kind?: "info" | "warn" | "error" | "ok" | "locked"; children: ReactNode; action?: ReactNode }) {
  const Icon = kind === "warn" ? AlertTriangle : kind === "error" ? CircleAlert : kind === "ok" ? CircleCheck : kind === "locked" ? Lock : Info;
  return (
    <div className={cx("banner", kind === "locked" ? "warn" : kind)} role={kind === "error" ? "alert" : "status"}>
      <Icon />
      <div className="banner-body selectable">{children}</div>
      {action}
    </div>
  );
}

/** Shown on every tab when PC Tuner does not run as administrator. */
export function ViewOnlyBanner({ admin, text = "View only: start PC Tuner as administrator to apply changes." }: { admin: boolean; text?: string }) {
  if (admin) return null;
  return <Banner kind="locked">{text}</Banner>;
}

export function Spinner({ large }: { large?: boolean }) {
  return <span className={cx("spinner", large && "lg")} role="progressbar" aria-label="Working" />;
}

export function Progress({ value }: { value: number | null }) {
  return (
    <div className={cx("progress", value === null && "indeterminate")}>
      <div style={value === null ? undefined : { width: `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%` }} />
    </div>
  );
}

export function LoadingState({ title = "Loading...", text }: { title?: string; text?: string }) {
  return (
    <div className="state">
      <Spinner large />
      <div className="state-title" style={{ marginTop: 8 }}>
        {title}
      </div>
      {text && <div className="state-text">{text}</div>}
    </div>
  );
}

export function EmptyState({ icon, title, text, action }: { icon?: ReactNode; title: string; text?: ReactNode; action?: ReactNode }) {
  return (
    <div className="state">
      {icon && <div className="state-icon">{icon}</div>}
      <div className="state-title">{title}</div>
      {text && <div className="state-text">{text}</div>}
      {action && <div style={{ marginTop: 8 }}>{action}</div>}
    </div>
  );
}

export function ErrorState({ title = "Something went wrong", text, action }: { title?: string; text: ReactNode; action?: ReactNode }) {
  return (
    <div className="state error" role="alert">
      <div className="state-icon">
        <CircleAlert />
      </div>
      <div className="state-title">{title}</div>
      <div className="state-text selectable">{text}</div>
      {action && <div style={{ marginTop: 8 }}>{action}</div>}
    </div>
  );
}

// ---------- numbers ----------

export function Stat({ value, unit, label, tone }: { value: ReactNode; unit?: string; label: string; tone?: Tone }) {
  return (
    <div className="stat">
      <div className={cx("stat-value", tone && toneClass(tone))}>
        {value}
        {unit && <small>{unit}</small>}
      </div>
      <div className="stat-label">{label}</div>
    </div>
  );
}

export function Hero({ value, unit, caption, tone }: { value: ReactNode; unit?: ReactNode; caption?: ReactNode; tone?: Tone }) {
  return (
    <div className="hero">
      <div className={cx("hero-value", tone && toneClass(tone))}>{value}</div>
      <div>
        {unit && <div className="hero-unit">{unit}</div>}
        {caption && <div className="hero-caption">{caption}</div>}
      </div>
    </div>
  );
}

/** A small live graph: a line with a soft fill, scaled to its own range. */
export function Sparkline({ values, color = "var(--accent)", min, max }: { values: number[]; color?: string; min?: number; max?: number }) {
  const w = 200;
  const h = 34;
  if (values.length < 2) return <svg className="sparkline" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" />;
  const lo = min ?? Math.min(...values);
  const hi = max ?? Math.max(...values);
  const span = hi - lo || 1;
  const pts = values.map((v, i) => [(i / (values.length - 1)) * w, h - 3 - ((v - lo) / span) * (h - 6)] as const);
  const line = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
  return (
    <svg className="sparkline" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden>
      <path d={`${line} L${w},${h} L0,${h} Z`} fill={color} opacity={0.14} />
      <path d={line} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

// ---------- overlays ----------

export function Sheet({ kicker, title, subtitle, children, footer, wide, onClose }: { kicker?: string; title: ReactNode; subtitle?: ReactNode; children?: ReactNode; footer?: ReactNode; wide?: boolean; onClose?: () => void }) {
  useEffect(() => {
    if (!onClose) return;
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose?.()}>
      <div className={cx("sheet", wide && "wide")} role="dialog" aria-modal>
        <div className="sheet-head">
          {kicker && <div className="kicker">{kicker}</div>}
          <h2>{title}</h2>
          {subtitle && <p>{subtitle}</p>}
        </div>
        {children && <div className="sheet-body">{children}</div>}
        {footer && <div className="sheet-foot">{footer}</div>}
      </div>
    </div>
  );
}

export interface MenuItem {
  label: string;
  onClick: () => void;
  disabled?: boolean;
}

/** A context menu at a screen position (closes on any click or Escape). */
export function ContextMenu({ x, y, items, onClose }: { x: number; y: number; items: (MenuItem | "separator")[]; onClose: () => void }) {
  useEffect(() => {
    const close = () => onClose();
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    // listen only once the click that opened the menu is over, or that same click would close it at once
    const start = setTimeout(() => {
      window.addEventListener("click", close);
      window.addEventListener("contextmenu", close);
    }, 0);
    window.addEventListener("keydown", key);
    return () => {
      clearTimeout(start);
      window.removeEventListener("click", close);
      window.removeEventListener("contextmenu", close);
      window.removeEventListener("keydown", key);
    };
  }, [onClose]);
  const left = Math.min(x, window.innerWidth - 200);
  const top = Math.min(y, window.innerHeight - items.length * 28 - 16);
  return (
    <div className="menu" style={{ left, top }} role="menu">
      {items.map((it, i) =>
        it === "separator" ? (
          <hr key={i} />
        ) : (
          <button key={i} type="button" role="menuitem" disabled={it.disabled}
            onClick={() => {
              it.onClick();
              onClose();
            }}
          >
            {it.label}
          </button>
        ),
      )}
    </div>
  );
}
