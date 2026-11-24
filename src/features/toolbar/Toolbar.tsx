import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Check, ChevronDown, Command, Database, FileText, FolderOpen, HardDrive, Monitor, Moon, PanelLeft, Play, Square, Sun } from "lucide-react";
import { api, type SourceSpec } from "@/lib/api";
import { useStore } from "@/store";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { setThemePref, THEME_LABEL, useThemePref, type ThemePref } from "@/lib/theme";
import { toggleSidebar } from "@/lib/sidebar";

export async function openDuckdbFile() {
  const path = await open({ multiple: false, filters: [{ name: "DuckDB", extensions: ["duckdb", "db", "ddb"] }] });
  if (typeof path === "string") await openSource({ kind: "file", path });
}

export async function openSource(spec: SourceSpec) {
  try {
    await api.openSource(spec);
  } catch (e) {
    useStore.setState({ sourceError: String(e) });
  }
}

function TBtn({ tip, onClick, disabled, children, className }: { tip: string; onClick: () => void; disabled?: boolean; children: React.ReactNode; className?: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          onClick={onClick}
          disabled={disabled}
          className={cn(
            "inline-flex h-[22px] items-center gap-1 rounded-sm px-1.5 hover:bg-accent disabled:pointer-events-none disabled:opacity-40",
            className,
          )}
        >
          {children}
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom" className="text-xs">
        {tip}
      </TooltipContent>
    </Tooltip>
  );
}

export function Toolbar() {
  const source = useStore((s) => s.source);
  const sourceError = useStore((s) => s.sourceError);
  const connections = useStore((s) => s.connections);
  const recents = useStore((s) => s.recents);
  const tab = useStore((s) => s.tabs[s.active]);
  const apiStatus = useStore((s) => s.apiStatus);
  const setPalette = useStore((s) => s.setPalette);
  const running = tab?.status === "running";

  const [limitText, setLimitText] = useState("");
  useEffect(() => setLimitText(tab ? String(tab.limit) : ""), [tab?.id, tab?.limit]);
  const commitLimit = () => {
    if (!tab) return;
    const t = limitText.trim();
    const n = t === "" ? null : Number.parseInt(t, 10);
    if (n !== null && (Number.isNaN(n) || n < 0)) return setLimitText(String(tab.limit));
    if (n === tab.limit && tab.limit_override !== null) return;
    void api.setLimit(tab.id, n);
  };

  const SrcIcon = source?.kind === "memory" ? FileText : source?.kind === "file" ? HardDrive : Database;

  return (
    <div className="flex h-[var(--bar-h)] shrink-0 items-center gap-0.5 border-b px-1">
      <TBtn tip="Toggle sidebar (Cmd+B)" onClick={toggleSidebar}>
        <PanelLeft className="size-3.5" />
      </TBtn>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            className={cn("inline-flex h-[22px] max-w-72 items-center gap-1 rounded-sm border px-1.5 hover:bg-accent", sourceError && "border-destructive")}
            title={sourceError ?? source?.path ?? "Files only (in-memory DuckDB)"}
          >
            <SrcIcon className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate">{source?.label ?? "..."}</span>
            {source?.read_only && <span className="text-[var(--fs-sm)] text-muted-foreground">ro</span>}
            <ChevronDown className="size-3 shrink-0 text-muted-foreground" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="min-w-60 text-xs">
          {connections.length > 0 && <DropdownMenuLabel className="py-0.5 text-[11px] text-muted-foreground">Connections</DropdownMenuLabel>}
          {connections.map((name) => (
            <DropdownMenuItem key={name} className="py-1 text-xs" onSelect={() => void openSource({ kind: "configured", name })}>
              <Database className="size-3.5" />
              {name}
            </DropdownMenuItem>
          ))}
          {recents.length > 0 && <DropdownMenuLabel className="py-0.5 text-[11px] text-muted-foreground">Recent files</DropdownMenuLabel>}
          {recents.map((path) => (
            <DropdownMenuItem key={path} className="py-1 text-xs" title={path} onSelect={() => void openSource({ kind: "file", path })}>
              <HardDrive className="size-3.5" />
              <span className="truncate">{path.split("/").pop()}</span>
              <span className="ml-auto max-w-40 truncate text-muted-foreground">{path}</span>
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem className="py-1 text-xs" onSelect={() => void openDuckdbFile()}>
            <FolderOpen className="size-3.5" />
            Open DuckDB file...
          </DropdownMenuItem>
          <DropdownMenuItem className="py-1 text-xs" onSelect={() => void openSource({ kind: "memory" })}>
            <FileText className="size-3.5" />
            No database (files only)
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <div className="mx-1 h-4 w-px bg-border" />
      <TBtn tip="Run (Cmd+Enter). Runs the selection if there is one." onClick={() => tab && void api.run(tab.id)} disabled={!tab}>
        <Play className="size-3.5" />
        Run
      </TBtn>
      <TBtn tip="Stop (Esc)" onClick={() => tab && void api.stop(tab.id)} disabled={!running}>
        <Square className="size-3" />
      </TBtn>
      <div className="mx-1 h-4 w-px bg-border" />
      <Tooltip>
        <TooltipTrigger asChild>
          <label className="flex items-center gap-1 text-muted-foreground">
            limit
            <input
              value={limitText}
              onChange={(e) => setLimitText(e.target.value.replace(/[^0-9]/g, ""))}
              onBlur={commitLimit}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                if (e.key === "Escape") {
                  setLimitText(String(tab?.limit ?? ""));
                  (e.target as HTMLInputElement).blur();
                }
              }}
              className={cn(
                "h-[20px] w-14 rounded-sm border bg-background px-1 text-right font-mono text-foreground outline-none focus:border-ring",
                tab?.limit_override != null && "border-[var(--selection-border)]",
              )}
            />
          </label>
        </TooltipTrigger>
        <TooltipContent side="bottom" className="text-xs">
          Row limit for this tab. 0 means no limit. Clear to use the config default.
        </TooltipContent>
      </Tooltip>

      <div className="ml-auto flex items-center gap-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <span
              className={cn(
                "inline-flex h-[18px] items-center gap-1 rounded-sm border px-1 font-mono text-[var(--fs-sm)]",
                apiStatus?.running ? "text-foreground" : "text-muted-foreground",
                apiStatus?.error && "border-destructive text-destructive",
              )}
            >
              <span className={cn("size-1.5 rounded-full", apiStatus?.running ? "bg-emerald-500" : "bg-muted-foreground/50")} />
              {apiStatus?.running ? `api :${apiStatus.port}` : "api off"}
            </span>
          </TooltipTrigger>
          <TooltipContent side="bottom" className="max-w-80 text-xs">
            {apiStatus?.error ?? (apiStatus?.running ? `Agent API on 127.0.0.1:${apiStatus.port}${apiStatus.auth_required ? ", API key required" : ""}` : "Agent API disabled in config")}
          </TooltipContent>
        </Tooltip>
        <ThemeMenu />
        <TBtn tip="Command palette: actions and query history" onClick={() => setPalette(true)} className="text-muted-foreground">
          <Command className="size-3.5" />
          <span className="font-mono text-[var(--fs-sm)]">Cmd+K</span>
        </TBtn>
      </div>
    </div>
  );
}

const THEME_ICON: Record<ThemePref, typeof Sun> = { system: Monitor, light: Sun, dark: Moon };

function ThemeMenu() {
  const pref = useThemePref();
  const Icon = THEME_ICON[pref];
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <button className="inline-flex h-[22px] items-center rounded-sm px-1 text-muted-foreground hover:bg-accent hover:text-foreground">
              <Icon className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom" className="text-xs">
          Theme: {THEME_LABEL[pref]}
        </TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="min-w-32">
        {(["system", "light", "dark"] as const).map((t) => {
          const I = THEME_ICON[t];
          return (
            <DropdownMenuItem key={t} className="py-1 text-xs" onSelect={() => setThemePref(t)}>
              <I className="size-3.5" />
              {THEME_LABEL[t]}
              {pref === t && <Check className="ml-auto size-3.5" />}
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
