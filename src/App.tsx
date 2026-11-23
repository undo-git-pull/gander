import { useEffect } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api } from "@/lib/api";
import { useStore } from "@/store";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { Toolbar, openSource } from "@/features/toolbar/Toolbar";
import { TabStrip } from "@/features/tabs/TabStrip";
import { SqlEditor, editorBridge } from "@/features/editor/SqlEditor";
import { ResultArea } from "@/features/grid/ResultArea";
import { Sidebar, focusSchemaFilter } from "@/features/sidebar/Sidebar";
import { StatusBar } from "@/features/statusbar/StatusBar";
import { TrustBanner } from "@/features/trust/TrustBanner";
import { CommandPalette } from "@/features/palette/CommandPalette";
import { StatsDock } from "@/features/stats-panel/StatsPanel";
import { useDensity } from "@/lib/density";
import { useLayout } from "@/lib/layout";
import { sqlString } from "@/lib/sql";
import { applyWindowTheme, useThemePref } from "@/lib/theme";
import { expandSidebar, sidebarRef, toggleSidebar } from "@/lib/sidebar";

/** Match a CodeMirror-style key spec such as "Mod-k" or "Shift-Mod-Enter". */
function matches(e: KeyboardEvent, spec: string): boolean {
  const parts = spec.split("-");
  const key = parts.pop()!.toLowerCase();
  const mods = new Set(parts.map((p) => p.toLowerCase()));
  const isMac = navigator.platform.toLowerCase().includes("mac");
  const want = {
    meta: mods.has("cmd") || mods.has("meta") || (isMac && mods.has("mod")),
    ctrl: mods.has("ctrl") || (!isMac && mods.has("mod")),
    shift: mods.has("shift"),
    alt: mods.has("alt"),
  };
  return e.key.toLowerCase() === key && e.metaKey === want.meta && e.ctrlKey === want.ctrl && e.shiftKey === want.shift && e.altKey === want.alt;
}

function useTheme() {
  const theme = useThemePref();
  const density = useStore((s) => s.config?.ui.density ?? "compact");
  const { rowH, hdrH, fontSize } = useDensity();
  useEffect(() => {
    const root = document.documentElement;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => root.classList.toggle("dark", theme === "dark" || (theme === "system" && mq.matches));
    applyWindowTheme(theme);
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.density = density;
    root.style.setProperty("--fs", `${fontSize}px`);
    root.style.setProperty("--fs-sm", `${fontSize - 1}px`);
    root.style.setProperty("--row-h", `${rowH}px`);
    root.style.setProperty("--hdr-h", `${hdrH}px`);
  }, [density, fontSize, rowH, hdrH]);
}

function useGlobalKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      const tab = s.tabs[s.active];
      const mod = e.metaKey || e.ctrlKey;
      if (matches(e, s.config?.keybindings.palette ?? "Mod-k")) {
        e.preventDefault();
        s.setPalette(!s.paletteOpen);
        return;
      }
      if (!mod) return;
      const k = e.key.toLowerCase();
      if (k === "t" && !e.shiftKey) {
        e.preventDefault();
        void api.newTab();
      } else if (k === "w" && !e.shiftKey) {
        e.preventDefault();
        if (tab) void api.closeTab(tab.id);
      } else if (k === "b" && !e.shiftKey) {
        e.preventDefault();
        toggleSidebar();
      } else if (k === "p" && !e.shiftKey) {
        e.preventDefault();
        expandSidebar();
        requestAnimationFrame(focusSchemaFilter);
      } else if (k === "i" && !e.shiftKey) {
        e.preventDefault();
        if (tab) void api.setStatsOpen(tab.id, !tab.stats_open);
      } else if (/^[1-9]$/.test(e.key) && !e.shiftKey) {
        e.preventDefault();
        const id = s.order[Number(e.key) - 1];
        if (id) void api.activateTab(id);
      } else if (e.shiftKey && (e.key === "[" || e.key === "]" || e.key === "{" || e.key === "}")) {
        e.preventDefault();
        const i = s.order.indexOf(s.active);
        const step = e.key === "[" || e.key === "{" ? -1 : 1;
        const id = s.order[(i + step + s.order.length) % s.order.length];
        if (id) void api.activateTab(id);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
}

const DATA_EXT = /\.(parquet|csv|tsv|json|jsonl|ndjson|csv\.gz|json\.gz)$/i;
const DB_EXT = /\.(duckdb|ddb|db)$/i;

function useFileDrop() {
  useEffect(() => {
    let un: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent(async (e) => {
        if (e.payload.type !== "drop") return;
        for (const path of e.payload.paths) {
          if (DB_EXT.test(path)) await openSource({ kind: "file", path });
          else if (DATA_EXT.test(path)) await api.newTab(`FROM ${sqlString(path)}`, true);
        }
      })
      .then((f) => (un = f));
    return () => un?.();
  }, []);
}

export default function App() {
  useTheme();
  useGlobalKeys();
  useFileDrop();
  const outer = useLayout("outer");
  const main = useLayout("main");
  return (
    <TooltipProvider delayDuration={400}>
      <div className="flex h-full flex-col">
        <Toolbar />
        <TrustBanner />
        <div className="min-h-0 flex-1">
          <ResizablePanelGroup orientation="horizontal" defaultLayout={outer.initial} onLayoutChanged={outer.save}>
            <ResizablePanel id="sidebar" panelRef={sidebarRef} defaultSize="17" minSize={120} collapsible collapsedSize={0}>
              <StatsDock top={<Sidebar />} />
            </ResizablePanel>
            <ResizableHandle />
            <ResizablePanel id="main" minSize={300}>
              <div className="flex h-full min-h-0 flex-col">
                <TabStrip />
                <div className="min-h-0 flex-1">
                  <ResizablePanelGroup orientation="vertical" defaultLayout={main.initial} onLayoutChanged={main.save}>
                    <ResizablePanel id="editor" defaultSize="32" minSize={44}>
                      <div className="h-full" onClick={() => editorBridge.focus()}>
                        <SqlEditor />
                      </div>
                    </ResizablePanel>
                    <ResizableHandle />
                    <ResizablePanel id="results" minSize={80}>
                      <ResultArea />
                    </ResizablePanel>
                  </ResizablePanelGroup>
                </div>
              </div>
            </ResizablePanel>
          </ResizablePanelGroup>
        </div>
        <StatusBar />
      </div>
      <CommandPalette />
    </TooltipProvider>
  );
}
