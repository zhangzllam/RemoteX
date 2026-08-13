import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type AppearancePreference = "system" | "light" | "dark";
export type ResolvedAppearance = "light" | "dark";

const APPEARANCE_STORAGE_KEY = "remotex-appearance";
const DARK_MODE_QUERY = "(prefers-color-scheme: dark)";
const IS_TAURI = typeof (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ === "object";

function loadAppearance(): AppearancePreference {
  const stored = window.localStorage.getItem(APPEARANCE_STORAGE_KEY);
  return stored === "light" || stored === "dark" || stored === "system" ? stored : "system";
}

function resolveAppearance(preference: AppearancePreference): ResolvedAppearance {
  if (preference !== "system") return preference;
  return window.matchMedia(DARK_MODE_QUERY).matches ? "dark" : "light";
}

function applyDocumentAppearance(appearance: ResolvedAppearance) {
  document.documentElement.dataset.theme = appearance;
  document.documentElement.style.colorScheme = appearance;
}

const initialAppearance = loadAppearance();
applyDocumentAppearance(resolveAppearance(initialAppearance));

/** Keeps RemoteX content and the native window frame on the same appearance. */
export function useAppearance() {
  const [preference, setPreference] = useState<AppearancePreference>(initialAppearance);
  const [resolved, setResolved] = useState<ResolvedAppearance>(() => resolveAppearance(initialAppearance));

  useEffect(() => {
    const media = window.matchMedia(DARK_MODE_QUERY);
    const apply = () => {
      const next = preference === "system" ? (media.matches ? "dark" : "light") : preference;
      setResolved(next);
      applyDocumentAppearance(next);
      window.localStorage.setItem(APPEARANCE_STORAGE_KEY, preference);
      if (IS_TAURI) void getCurrentWindow().setTheme(preference === "system" ? null : preference).catch(() => undefined);
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [preference]);

  return { preference, resolved, setPreference };
}
