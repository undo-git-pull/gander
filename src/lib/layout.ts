import { useMemo, useRef } from "react";
import type { Layout } from "react-resizable-panels";
import { api } from "@/lib/api";

/** Panel layouts persisted to the app state file. Loaded once at start. */
let saved: Record<string, Layout> = {};
let timer: number | null = null;

export async function loadLayouts() {
  try {
    const l = await api.layout();
    if (l && typeof l === "object") saved = l as Record<string, Layout>;
  } catch {
    /* defaults */
  }
}

export function useLayout(name: string) {
  const initial = useRef(saved[name]);
  return useMemo(
    () => ({
      initial: initial.current,
      save: (layout: Layout) => {
        saved = { ...saved, [name]: layout };
        if (timer) window.clearTimeout(timer);
        timer = window.setTimeout(() => void api.saveLayout(saved), 500);
      },
    }),
    [name],
  );
}
