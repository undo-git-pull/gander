import { create } from "zustand";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useStore } from "@/store";

export type ThemePref = "system" | "light" | "dark";

export const THEME_LABEL: Record<ThemePref, string> = { system: "System", light: "Light", dark: "Dark" };

const KEY = "gander.theme";

function read(): ThemePref | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === "system" || v === "light" || v === "dark" ? v : null;
  } catch {
    return null;
  }
}

/**
 * Theme chosen in the UI. It overrides `ui.theme` from the config file once
 * set; until then the config value (default "system") applies.
 */
const useThemeOverride = create<{ pref: ThemePref | null }>(() => ({ pref: read() }));

export function setThemePref(pref: ThemePref) {
  try {
    localStorage.setItem(KEY, pref);
  } catch {
    /* not persisted; still applies for this session */
  }
  useThemeOverride.setState({ pref });
}

/** The theme preference in effect: UI choice, else config, else system. */
export function useThemePref(): ThemePref {
  const override = useThemeOverride((s) => s.pref);
  const configured = useStore((s) => s.config?.ui.theme ?? "system");
  return override ?? configured;
}

/** Match the native title bar to the chosen theme; null follows the OS. */
export function applyWindowTheme(pref: ThemePref) {
  void getCurrentWindow()
    .setTheme(pref === "system" ? null : pref)
    .catch(() => {
      /* window theme is cosmetic */
    });
}
