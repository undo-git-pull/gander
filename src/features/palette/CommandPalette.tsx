import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { api, type HistoryEntry } from "@/lib/api";
import { useStore } from "@/store";
import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandShortcut } from "@/components/ui/command";
import { editorBridge } from "@/features/editor/SqlEditor";
import { focusSchemaFilter } from "@/features/sidebar/Sidebar";
import { openDuckdbFile, openSource } from "@/features/toolbar/Toolbar";
import { copyText } from "@/lib/format";
import { setThemePref, THEME_LABEL } from "@/lib/theme";
import { expandSidebar, toggleSidebar } from "@/lib/sidebar";

function ago(ts: number) {
  const s = Date.now() / 1000 - ts;
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

export function CommandPalette() {
  const open = useStore((s) => s.paletteOpen);
  const setOpen = useStore((s) => s.setPalette);
  const [history, setHistory] = useState<HistoryEntry[]>([]);

  useEffect(() => {
    if (open) void api.history(300).then(setHistory);
  }, [open]);

  const act = (f: () => void | Promise<unknown>) => () => {
    setOpen(false);
    void f();
  };
  const s = useStore.getState();
  const tab = s.tabs[s.active];

  return (
    <CommandDialog open={open} onOpenChange={setOpen} className="top-[15%] max-w-2xl translate-y-0 text-xs">
      <CommandInput placeholder="Type a command or search history..." className="h-8 text-xs" />
      <CommandList className="max-h-[60vh]">
        <CommandEmpty>No results.</CommandEmpty>
        <CommandGroup heading="Actions">
          <CommandItem className="py-1 text-xs" onSelect={act(() => api.newTab())}>
            New tab<CommandShortcut>Cmd+T</CommandShortcut>
          </CommandItem>
          <CommandItem className="py-1 text-xs" onSelect={act(() => tab && api.closeTab(tab.id))}>
            Close tab<CommandShortcut>Cmd+W</CommandShortcut>
          </CommandItem>
          <CommandItem className="py-1 text-xs" onSelect={act(() => tab && api.setStatsOpen(tab.id, !tab.stats_open))}>
            Toggle column stats<CommandShortcut>Cmd+I</CommandShortcut>
          </CommandItem>
          <CommandItem className="py-1 text-xs" onSelect={act(toggleSidebar)}>
            Toggle sidebar<CommandShortcut>Cmd+B</CommandShortcut>
          </CommandItem>
          <CommandItem className="py-1 text-xs" onSelect={act(() => { expandSidebar(); setTimeout(focusSchemaFilter, 50); })}>
            Filter schema<CommandShortcut>Cmd+P</CommandShortcut>
          </CommandItem>
          {(["system", "light", "dark"] as const).map((t) => (
            <CommandItem key={t} className="py-1 text-xs" onSelect={act(() => setThemePref(t))}>
              Theme: {THEME_LABEL[t]}
            </CommandItem>
          ))}
          <CommandItem className="py-1 text-xs" onSelect={act(openDuckdbFile)}>
            Open DuckDB file...
          </CommandItem>
          <CommandItem className="py-1 text-xs" onSelect={act(() => openSource({ kind: "memory" }))}>
            Switch to files only
          </CommandItem>
          {tab?.executed_sql && (
            <CommandItem className="py-1 text-xs" onSelect={act(() => copyText(tab.executed_sql ?? ""))}>
              Copy executed SQL
            </CommandItem>
          )}
          {tab?.truncated && (
            <CommandItem className="py-1 text-xs" onSelect={act(() => api.countRows(tab.id))}>
              Count all rows
            </CommandItem>
          )}
          <CommandItem className="py-1 text-xs" onSelect={act(() => openPath(s.queryFile))}>
            Open query.sql
          </CommandItem>
        </CommandGroup>
        {history.length > 0 && (
          <CommandGroup heading="History (replaces the editor text; Cmd+Z undoes)">
            {history.map((h, i) => (
              <CommandItem key={i} value={`${h.sql} #${i}`} className="py-1 font-mono text-xs" onSelect={act(() => editorBridge.replaceAll(h.sql))}>
                <span className="truncate">{h.sql.replace(/\s+/g, " ").slice(0, 160)}</span>
                <CommandShortcut className="font-sans">
                  {h.origin !== "user" ? `${h.origin} ` : ""}
                  {ago(h.ts)}
                </CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
      </CommandList>
    </CommandDialog>
  );
}
