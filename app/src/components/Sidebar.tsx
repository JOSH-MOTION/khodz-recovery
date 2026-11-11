import { Icon, type IconName } from "./Icon";

export type Route = "drives" | "images" | "results" | "recovery" | "history" | "settings";

export const NAV: { route: Route; label: string; icon: IconName; group: "main" | "footer" }[] = [
  { route: "drives", label: "Drives", icon: "drive", group: "main" },
  { route: "images", label: "Disk Images", icon: "image", group: "main" },
  { route: "results", label: "Results", icon: "results", group: "main" },
  { route: "recovery", label: "Recovery", icon: "recover", group: "main" },
  { route: "history", label: "History", icon: "history", group: "main" },
  { route: "settings", label: "Settings", icon: "settings", group: "footer" },
];

function NavButton({ item, active, onSelect }: { item: (typeof NAV)[number]; active: boolean; onSelect: (r: Route) => void }) {
  return (
    <button
      type="button"
      onClick={() => onSelect(item.route)}
      aria-current={active ? "page" : undefined}
      className={`group relative flex w-full items-center gap-3 rounded-lg px-3 py-2 text-[13.5px] font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent/60 ${
        active ? "bg-accent-soft text-fg" : "text-muted hover:bg-surface-2 hover:text-fg"
      }`}
    >
      {active && <span className="absolute top-1/2 left-0 h-5 w-[3px] -translate-y-1/2 rounded-full bg-accent" />}
      <Icon name={item.icon} className={active ? "text-accent" : "text-muted group-hover:text-fg"} />
      {item.label}
    </button>
  );
}

export function Sidebar({ route, onNavigate }: { route: Route; onNavigate: (r: Route) => void }) {
  return (
    <aside className="flex w-[232px] shrink-0 flex-col border-r border-line bg-sidebar">
      <div className="flex items-center gap-2.5 px-5 pt-5 pb-6">
        <svg width="28" height="28" viewBox="0 0 1024 1024" aria-hidden="true">
          <rect x="64" y="64" width="896" height="896" rx="200" fill="#0f182b" stroke="#2a3a5a" strokeWidth="16" />
          <circle cx="512" cy="512" r="250" fill="none" stroke="#2a3a5a" strokeWidth="36" />
          <circle cx="512" cy="512" r="60" fill="#22d3ee" />
          <path d="M512 262 A250 250 0 1 1 285 405" fill="none" stroke="#22d3ee" strokeWidth="56" strokeLinecap="round" />
        </svg>
        <div className="leading-tight">
          <div className="text-[15px] font-semibold tracking-tight">Khodz</div>
          <div className="text-[11px] font-medium tracking-[0.14em] text-muted uppercase">Recovery</div>
        </div>
      </div>

      <nav aria-label="Main" className="flex flex-1 flex-col gap-0.5 px-3">
        {NAV.filter((n) => n.group === "main").map((item) => (
          <NavButton key={item.route} item={item} active={route === item.route} onSelect={onNavigate} />
        ))}
      </nav>

      <div className="flex flex-col gap-3 px-3 pb-4">
        {NAV.filter((n) => n.group === "footer").map((item) => (
          <NavButton key={item.route} item={item} active={route === item.route} onSelect={onNavigate} />
        ))}
        <div
          className="flex items-start gap-2.5 rounded-lg border border-line bg-surface px-3 py-2.5"
          title="Khodz Recovery opens source drives read-only and never writes to them."
        >
          <Icon name="shield" size={16} className="mt-0.5 shrink-0 text-ok" />
          <div className="text-[12px] leading-snug">
            <div className="font-medium">Read-only access</div>
            <div className="text-muted">Source drives are never written</div>
          </div>
        </div>
      </div>
    </aside>
  );
}
