import { api } from "../lib/api";
import type { ThemePreference } from "../lib/theme";
import { useAsync } from "../lib/useAsync";
import { Card, ErrorBanner, PageHeader } from "../components/ui";
import { Icon, type IconName } from "../components/Icon";

const THEMES: { value: ThemePreference; label: string; icon: IconName }[] = [
  { value: "dark", label: "Dark", icon: "moon" },
  { value: "light", label: "Light", icon: "sun" },
  { value: "system", label: "System", icon: "monitor" },
];

export function SettingsPage({
  theme,
  onThemeChange,
}: {
  theme: ThemePreference;
  onThemeChange: (t: ThemePreference) => void;
}) {
  const { state, reload } = useAsync(api.appInfo);

  return (
    <>
      <PageHeader title="Settings" />
      <div className="k-enter flex max-w-3xl flex-col gap-5 px-8 py-6">
        <Card className="p-5">
          <h2 className="text-[14px] font-semibold">Appearance</h2>
          <p className="mt-1 text-[13px] text-muted">Choose how Khodz Recovery looks.</p>
          <div role="radiogroup" aria-label="Theme" className="mt-4 grid grid-cols-3 gap-3">
            {THEMES.map((t) => {
              const selected = theme === t.value;
              return (
                <button
                  key={t.value}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  onClick={() => onThemeChange(t.value)}
                  className={`flex items-center gap-2.5 rounded-lg border px-4 py-3 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent/60 ${
                    selected ? "border-accent bg-accent-soft text-fg" : "border-line text-muted hover:bg-surface-2 hover:text-fg"
                  }`}
                >
                  <Icon name={t.icon} size={17} className={selected ? "text-accent" : ""} />
                  {t.label}
                </button>
              );
            })}
          </div>
        </Card>

        <Card className="p-5">
          <h2 className="text-[14px] font-semibold">Data safety</h2>
          <ul className="mt-3 flex flex-col gap-2.5 text-[13px]">
            {[
              "Source drives and images are opened read-only. Khodz Recovery has no code path that writes to them.",
              "Recovery to the same physical disk as the source is blocked by default.",
              "Encrypted volumes (BitLocker, FileVault) must be unlocked by the operating system first; encryption is never bypassed.",
            ].map((line) => (
              <li key={line} className="flex gap-2.5">
                <Icon name="shield" size={16} className="mt-0.5 shrink-0 text-ok" />
                <span className="text-muted">{line}</span>
              </li>
            ))}
          </ul>
        </Card>

        <Card className="p-5">
          <h2 className="text-[14px] font-semibold">About</h2>
          {state.status === "loading" && <p className="mt-3 text-[13px] text-muted">Loading…</p>}
          {state.status === "error" && (
            <div className="mt-3">
              <ErrorBanner message={state.error} onRetry={reload} />
            </div>
          )}
          {state.status === "ready" && (
            <dl className="selectable mt-3 grid grid-cols-[140px_1fr] gap-y-2 text-[13px]">
              <dt className="text-muted">Version</dt>
              <dd>{state.data.version}</dd>
              <dt className="text-muted">Platform</dt>
              <dd>
                {state.data.os} · {state.data.arch}
              </dd>
              <dt className="text-muted">Database</dt>
              <dd className="font-mono text-[12px] break-all">{state.data.db_path}</dd>
              <dt className="text-muted">Schema</dt>
              <dd>v{state.data.schema_version}</dd>
            </dl>
          )}
        </Card>
      </div>
    </>
  );
}
