import { api } from "@/lib/api";
import { fmtInt, fmtMs } from "@/lib/format";
import { useStore } from "@/store";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export function StatusBar() {
  const tab = useStore((s) => s.tabs[s.active]);
  const errors = useStore((s) => s.configErrors);
  const sourceError = useStore((s) => s.sourceError);
  const sourceWarnings = useStore((s) => s.source?.warnings ?? []);
  const apiStatus = useStore((s) => s.apiStatus);
  const configFile = useStore((s) => s.configFile);
  const serverIgnored = useStore((s) => s.workspaceTrust?.ignored.includes("server") ?? false);

  const rows = tab?.row_count;
  return (
    <div className="flex h-[22px] shrink-0 items-center gap-3 border-t bg-muted/50 px-2 font-mono text-[var(--fs-sm)] whitespace-nowrap text-muted-foreground">
      {tab && rows != null && (
        <Tooltip>
          <TooltipTrigger asChild>
            <span className={cn(tab.truncated && "font-medium text-[var(--warning)]")}>
              {fmtInt(rows)}
              {tab.truncated ? "+ rows (limit)" : rows === 1 ? " row" : " rows"}
            </span>
          </TooltipTrigger>
          <TooltipContent side="top" className="max-w-[640px] font-mono text-xs whitespace-pre-wrap">
            {tab.executed_sql ?? "not run in this session"}
          </TooltipContent>
        </Tooltip>
      )}
      {tab?.truncated && tab.has_result && (
        <button className="text-foreground underline-offset-2 hover:underline" onClick={() => void api.countRows(tab.id)} disabled={tab.counting}>
          {tab.counting ? "counting..." : tab.total_count != null ? `${fmtInt(tab.total_count)} total` : "Count all rows"}
        </button>
      )}
      {tab?.elapsed_ms != null && <span>{fmtMs(tab.elapsed_ms)}</span>}
      {tab?.status === "running" && <span className="text-foreground">running...</span>}
      {tab && tab.sort.length > 0 && <span>sort: {tab.sort.map((s) => `${s.column} ${s.dir}`).join(", ")}</span>}
      <span className="ml-auto" />
      <button className="hover:text-foreground" onClick={() => useStore.getState().setPalette(true)} title="Open the command palette">
        Cmd+K commands
      </button>
      {sourceError && <span className="truncate text-destructive" title={sourceError}>source: {sourceError}</span>}
      {sourceWarnings.length > 0 && (
        <span className="text-[var(--warning)]" title={sourceWarnings.join("\n")}>
          {sourceWarnings.length} warning{sourceWarnings.length > 1 ? "s" : ""}
        </span>
      )}
      <Tooltip>
        <TooltipTrigger asChild>
          <span className={cn("max-w-[40vw] truncate", errors.length > 0 && "text-destructive")}>
            {errors.length > 0 ? `cfg: ${errors[0].file.split("/").pop()}${errors[0].line ? `:${errors[0].line}` : ""} ${errors[0].message}` : "cfg ok"}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top" className="max-w-[640px] font-mono text-xs whitespace-pre-wrap">
          {errors.length > 0
            ? errors.map((e) => `${e.file}${e.line ? `:${e.line}` : ""}: ${e.message}`).join("\n") + "\nThe last good config stays active."
            : configFile}
          {serverIgnored && "\nThe workspace config's [server] table is ignored; server settings are read only from the global config."}
        </TooltipContent>
      </Tooltip>
      <span>{apiStatus?.running ? `api :${apiStatus.port}` : "api off"}</span>
      {tab && <span className={cn(tab.last_change === "agent" && "text-foreground")}>last: {tab.last_change}</span>}
    </div>
  );
}
