import { useCallback, useEffect, useState } from "react";
import { api } from "./api";

export type ThemePreference = "dark" | "light" | "system";
export type ResolvedTheme = "dark" | "light";

export function isThemePreference(v: unknown): v is ThemePreference {
  return v === "dark" || v === "light" || v === "system";
}

export function resolveTheme(pref: ThemePreference, systemPrefersDark: boolean): ResolvedTheme {
  if (pref === "system") return systemPrefersDark ? "dark" : "light";
  return pref;
}

function systemPrefersDark(): boolean {
  return typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)").matches
    : true;
}

/** Match the native title bar / window chrome to the app theme (no-op outside Tauri). */
function syncNativeWindowTheme(theme: ResolvedTheme) {
  if (!("__TAURI_INTERNALS__" in window)) return;
  import("@tauri-apps/api/window")
    .then(({ getCurrentWindow }) => getCurrentWindow().setTheme(theme))
    .catch((e) => console.error("Failed to set window theme", e));
}

/** Theme preference persisted in the app database; applied as `data-theme` on <html>. */
export function useTheme() {
  const [preference, setPreference] = useState<ThemePreference>("dark");
  const [systemDark, setSystemDark] = useState(systemPrefersDark);

  useEffect(() => {
    let cancelled = false;
    api
      .getSetting("theme")
      .then((v) => {
        if (!cancelled && isThemePreference(v)) setPreference(v);
      })
      .catch((e) => console.error("Failed to load theme setting", e));
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  const resolved = resolveTheme(preference, systemDark);

  useEffect(() => {
    document.documentElement.dataset.theme = resolved;
    syncNativeWindowTheme(resolved);
  }, [resolved]);

  const update = useCallback((next: ThemePreference) => {
    setPreference(next);
    api.setSetting("theme", next).catch((e) => console.error("Failed to save theme setting", e));
  }, []);

  return { preference, resolved, setPreference: update };
}
