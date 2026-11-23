import { useStore } from "@/store";

const SIZES = {
  compact: { rowH: 20, hdrH: 22 },
  cozy: { rowH: 26, hdrH: 28 },
};

export function useDensity() {
  const density = useStore((s) => s.config?.ui.density ?? "compact");
  const fontSize = useStore((s) => s.config?.ui.font_size ?? 12);
  const base = SIZES[density];
  // Rows grow with larger fonts so text never clips.
  const extra = Math.max(0, fontSize - 12) * 2;
  return { rowH: base.rowH + extra, hdrH: base.hdrH + extra, fontSize };
}
