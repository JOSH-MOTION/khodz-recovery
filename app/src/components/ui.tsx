import type { ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: ReactNode }) {
  return (
    <header className="flex items-end justify-between gap-6 border-b border-line px-8 pt-7 pb-5">
      <div>
        <h1 className="text-[22px] font-semibold tracking-tight">{title}</h1>
        {subtitle && <p className="mt-1 text-[13.5px] text-muted">{subtitle}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}

type ButtonProps = {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  variant?: "primary" | "secondary";
  icon?: IconName;
  title?: string;
};

export function Button({ children, onClick, disabled, variant = "secondary", icon, title }: ButtonProps) {
  const base =
    "inline-flex h-9 items-center gap-2 rounded-lg px-3.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent/60 disabled:cursor-not-allowed disabled:opacity-45";
  const styles =
    variant === "primary"
      ? "bg-accent text-[#04121a] hover:bg-accent-strong"
      : "border border-line bg-surface text-fg hover:bg-surface-2";
  return (
    <button type="button" onClick={onClick} disabled={disabled} title={title} className={`${base} ${styles}`}>
      {icon && <Icon name={icon} size={16} />}
      {children}
    </button>
  );
}

export function EmptyState({
  icon,
  title,
  children,
  badge,
}: {
  icon: IconName;
  title: string;
  children: ReactNode;
  badge?: string;
}) {
  return (
    <div className="k-enter mx-auto mt-16 flex max-w-md flex-col items-center text-center">
      <div className="relative mb-5 grid h-16 w-16 place-items-center rounded-2xl border border-line bg-surface">
        <div className="absolute inset-0 rounded-2xl bg-accent-soft" />
        <Icon name={icon} size={28} className="relative text-accent" />
      </div>
      {badge && (
        <span className="mb-3 rounded-full border border-line px-2.5 py-0.5 text-[11px] font-medium tracking-wide text-muted uppercase">
          {badge}
        </span>
      )}
      <h2 className="text-[16px] font-semibold">{title}</h2>
      <div className="mt-2 text-[13.5px] leading-relaxed text-muted">{children}</div>
    </div>
  );
}

export function ErrorBanner({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div role="alert" className="flex items-center gap-3 rounded-lg border border-danger/40 bg-danger/10 px-4 py-3 text-[13px]">
      <Icon name="alert" size={18} className="shrink-0 text-danger" />
      <span className="selectable flex-1">{message}</span>
      {onRetry && <Button onClick={onRetry}>Retry</Button>}
    </div>
  );
}

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <section className={`rounded-xl border border-line bg-surface ${className}`}>{children}</section>;
}

const STATUS_STYLE: Record<string, string> = {
  running: "text-accent bg-accent-soft",
  completed: "text-ok bg-ok/10",
  cancelled: "text-muted bg-surface-2",
  failed: "text-danger bg-danger/10",
  device_lost: "text-warn bg-warn/10",
};

export function StatusPill({ status }: { status: string }) {
  return (
    <span className={`inline-flex rounded-full px-2 py-0.5 text-[11.5px] font-medium ${STATUS_STYLE[status] ?? "text-muted"}`}>
      {status.replace("_", " ")}
    </span>
  );
}
