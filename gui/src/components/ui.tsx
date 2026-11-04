import clsx from "clsx";
import { Loader2, X } from "lucide-react";
import { type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode, type Ref, useEffect, useRef } from "react";

type Tone = "cyan" | "violet" | "pink" | "lime" | "amber" | "red" | "muted";

const toneText: Record<Tone, string> = {
  cyan: "text-neon-cyan",
  violet: "text-neon-violet",
  pink: "text-neon-pink",
  lime: "text-neon-lime",
  amber: "text-neon-amber",
  red: "text-neon-red",
  muted: "text-ink-400",
};
const toneBg: Record<Tone, string> = {
  cyan: "bg-neon-cyan/12 border-neon-cyan/30",
  violet: "bg-neon-violet/12 border-neon-violet/30",
  pink: "bg-neon-pink/12 border-neon-pink/30",
  lime: "bg-neon-lime/12 border-neon-lime/30",
  amber: "bg-neon-amber/12 border-neon-amber/30",
  red: "bg-neon-red/12 border-neon-red/30",
  muted: "bg-ink-800 border-ink-700",
};

export function Badge({ tone = "muted", children, className, title }: { tone?: Tone; children: ReactNode; className?: string; title?: string }) {
  return (
    <span
      title={title}
      className={clsx(
        "inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-xs font-medium whitespace-nowrap",
        toneBg[tone],
        toneText[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

type BtnVariant = "primary" | "secondary" | "ghost" | "danger";
const btnVariant: Record<BtnVariant, string> = {
  primary: "bg-neon-cyan text-ink-950 hover:bg-cyan-300 font-semibold glow-cyan",
  secondary: "bg-ink-800 text-ink-100 border border-ink-700 hover:border-ink-600 hover:bg-ink-700",
  ghost: "text-ink-300 hover:text-ink-100 hover:bg-ink-800",
  danger: "bg-neon-red/15 text-neon-red border border-neon-red/30 hover:bg-neon-red/25",
};

export function Button({
  variant = "secondary",
  size = "md",
  loading,
  icon,
  className,
  children,
  ref,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: BtnVariant;
  size?: "sm" | "md" | "lg";
  loading?: boolean;
  icon?: ReactNode;
  ref?: Ref<HTMLButtonElement>;
}) {
  return (
    <button
      type="button"
      ref={ref}
      {...rest}
      disabled={rest.disabled || loading}
      className={clsx(
        "inline-flex items-center justify-center gap-2 rounded-lg transition-colors disabled:opacity-40 disabled:pointer-events-none cursor-pointer",
        size === "sm" && "h-8 px-3 text-sm",
        size === "md" && "h-10 px-4 text-sm",
        size === "lg" && "h-12 px-5 text-base",
        btnVariant[variant],
        className,
      )}
    >
      {loading ? <Loader2 className="size-4 animate-spin" /> : icon}
      {children}
    </button>
  );
}

export function IconButton({ label, children, className, ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      type="button"
      {...rest}
      aria-label={label}
      title={label}
      className={clsx(
        "inline-flex size-9 items-center justify-center rounded-lg text-ink-300 hover:bg-ink-800 hover:text-ink-100 disabled:opacity-30 cursor-pointer",
        className,
      )}
    >
      {children}
    </button>
  );
}

export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={(e) => {
        e.stopPropagation();
        onChange(!checked);
      }}
      className={clsx(
        "relative inline-flex h-7 w-12 shrink-0 items-center rounded-full border transition-colors cursor-pointer disabled:opacity-40",
        checked ? "bg-neon-cyan/25 border-neon-cyan/50" : "bg-ink-800 border-ink-700",
      )}
    >
      <span className={clsx("inline-block size-5 rounded-full transition-transform", checked ? "translate-x-6 bg-neon-cyan" : "translate-x-1 bg-ink-400")} />
    </button>
  );
}

export function Input(props: InputHTMLAttributes<HTMLInputElement> & { icon?: ReactNode }) {
  const { icon, className, ...rest } = props;
  return (
    <div className={clsx("flex h-10 items-center gap-2 rounded-lg border border-ink-700 bg-ink-900 px-3 focus-within:border-neon-cyan/60", className)}>
      {icon && <span className="text-ink-400">{icon}</span>}
      <input {...rest} className="h-full w-full bg-transparent text-sm text-ink-100 placeholder:text-ink-400 outline-none select-text" />
    </div>
  );
}

export function Card({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={clsx("glass rounded-2xl", className)}>{children}</div>;
}

export function Stat({ label, value, tone = "cyan", icon }: { label: string; value: ReactNode; tone?: Tone; icon?: ReactNode }) {
  return (
    <Card className="flex items-center gap-4 px-5 py-4">
      <div className={clsx("flex size-11 items-center justify-center rounded-xl border", toneBg[tone], toneText[tone])}>{icon}</div>
      <div>
        <div className="text-2xl font-semibold tabular-nums">{value}</div>
        <div className="text-xs uppercase tracking-wider text-ink-400">{label}</div>
      </div>
    </Card>
  );
}

export function Empty({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 py-16 text-center">
      <div className="flex size-14 items-center justify-center rounded-2xl border border-ink-700 bg-ink-850 text-ink-400">{icon}</div>
      <div className="text-lg font-medium">{title}</div>
      {children && <div className="max-w-md text-sm text-ink-400">{children}</div>}
    </div>
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center justify-center gap-3 py-16 text-ink-400">
      <Loader2 className="size-5 animate-spin text-neon-cyan" /> {label ?? "Loading…"}
    </div>
  );
}

export function ErrorBox({ error }: { error: unknown }) {
  return (
    <div className="rounded-xl border border-neon-red/30 bg-neon-red/10 px-4 py-3 font-mono text-sm text-neon-red whitespace-pre-wrap">{String(error)}</div>
  );
}

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-6 flex flex-wrap items-end justify-between gap-4">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        {subtitle && <p className="mt-1 text-sm text-ink-400">{subtitle}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

/** Side drawer; it is a navigation layer, so gamepad focus stays inside it. */
export function Drawer({
  open,
  onClose,
  title,
  children,
  width = "max-w-2xl",
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  width?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (open) setTimeout(() => ref.current?.querySelector<HTMLElement>("button, input")?.focus(), 30);
  }, [open]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-40 flex justify-end" data-nav-layer>
      <div className="absolute inset-0 bg-ink-950/60 backdrop-blur-sm" onClick={onClose} />
      <div ref={ref} className={clsx("relative flex h-full w-full flex-col border-l border-ink-700 bg-ink-900 shadow-2xl", width)}>
        <div className="flex items-center justify-between gap-4 border-b border-ink-700 px-6 py-4">
          <div className="min-w-0 flex-1">{title}</div>
          <IconButton label="Close (B / Esc)" onClick={onClose}>
            <X className="size-5" />
          </IconButton>
        </div>
        <div className="flex-1 overflow-y-auto px-6 py-5">{children}</div>
      </div>
    </div>
  );
}

export function Confirm({
  open,
  title,
  body,
  confirmLabel,
  danger,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  body: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (open) setTimeout(() => ref.current?.focus(), 30);
  }, [open]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-6" data-nav-layer>
      <div className="absolute inset-0 bg-ink-950/70 backdrop-blur-sm" onClick={onCancel} />
      <Card className="relative w-full max-w-md p-6">
        <h2 className="text-lg font-semibold">{title}</h2>
        <div className="mt-2 text-sm text-ink-300">{body}</div>
        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button ref={ref} variant={danger ? "danger" : "primary"} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </div>
      </Card>
    </div>
  );
}
