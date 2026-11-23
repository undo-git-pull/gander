import { useEffect, useRef, useState, type ReactNode } from "react";
import type { PanelImperativeHandle } from "react-resizable-panels";
import { ChartColumn, ChevronDown, ChevronRight, Loader2, Square } from "lucide-react";
import { api, type Cell, type Histogram } from "@/lib/api";
import { useStore } from "@/store";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { useLayout } from "@/lib/layout";
import { cn } from "@/lib/utils";

/**
 * Left column: `top` (the schema browser) above the column stats panel.
 * The stats panel starts collapsed to its header and only runs SUMMARIZE
 * when opened. Opening it from the header or a shortcut sizes it to half the
 * column; a drag keeps whatever size the user chose.
 */
export function StatsDock({ top }: { top: ReactNode }) {
  const tabId = useStore((s) => s.active);
  const statsOpen = useStore((s) => s.tabs[s.active]?.stats_open ?? false);
  const ref = useRef<PanelImperativeHandle | null>(null);
  const layout = useLayout("sidebar");
  // Ignore resize events caused by mounting or by syncing to the tab state;
  // only a user drag should open or close the panel.
  const quietUntil = useRef(Date.now() + 600);

  useEffect(() => {
    quietUntil.current = Date.now() + 300;
    const p = ref.current;
    if (!p) return;
    if (statsOpen && p.isCollapsed()) p.resize("50%");
    if (!statsOpen && !p.isCollapsed()) p.collapse();
  }, [statsOpen, tabId]);

  return (
    <ResizablePanelGroup orientation="vertical" defaultLayout={layout.initial} onLayoutChanged={layout.save}>
      <ResizablePanel id="schema" minSize={60}>
        {top}
      </ResizablePanel>
      <ResizableHandle />
      <ResizablePanel
        id="stats"
        panelRef={ref}
        collapsible
        collapsedSize={22}
        minSize={100}
        defaultSize={statsOpen ? "50" : 22}
        onResize={(size) => {
          if (Date.now() < quietUntil.current) return;
          const collapsed = size.inPixels <= 23;
          const t = useStore.getState().tabs[tabId];
          if (t && collapsed === t.stats_open) void api.setStatsOpen(tabId, !collapsed);
        }}
      >
        <div className="flex h-full min-h-0 flex-col">
          <StatsHeader tabId={tabId} />
          {statsOpen && (
            <div className="min-h-0 flex-1 overflow-auto">
              <StatsBody tabId={tabId} />
            </div>
          )}
        </div>
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}

function StatsHeader({ tabId }: { tabId: string }) {
  const tab = useStore((s) => s.tabs[tabId]);
  if (!tab) return null;
  const running = tab.stats_status === "running";
  return (
    <div
      className="flex h-[22px] shrink-0 cursor-pointer items-center gap-1 border-b bg-muted/50 px-1.5 text-[var(--fs-sm)] hover:bg-accent/60"
      onClick={() => void api.setStatsOpen(tabId, !tab.stats_open)}
      title="Per-column stats from DuckDB SUMMARIZE over the full query (Cmd+I)"
    >
      {tab.stats_open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
      <span className="font-medium">Column stats</span>
      <span className="text-muted-foreground">(SUMMARIZE)</span>
      {running && (
        <>
          <Loader2 className="spin size-3 text-muted-foreground" />
          <button
            title="Stop"
            className="inline-flex size-4 items-center justify-center rounded-sm hover:bg-accent"
            onClick={(e) => {
              e.stopPropagation();
              void api.setStatsOpen(tabId, false);
            }}
          >
            <Square className="size-2.5" />
          </button>
        </>
      )}
    </div>
  );
}

const text = (v: Cell) => (v === null ? null : typeof v === "object" ? JSON.stringify(v) : String(v));

function StatsBody({ tabId }: { tabId: string }) {
  const tab = useStore((s) => s.tabs[tabId]);
  const stats = useStore((s) => s.stats[tabId]);
  const jumpTo = useStore((s) => s.jumpTo);
  const [histOpen, setHistOpen] = useState<ReadonlySet<string>>(new Set());
  const toggleHist = (name: string) =>
    setHistOpen((prev) => {
      const next = new Set(prev);
      if (!next.delete(name)) next.add(name);
      return next;
    });
  if (!tab) return null;
  const muted = (m: string) => <div className="p-2 text-muted-foreground">{m}</div>;
  if (!tab.has_result) return muted("Run a query to see column stats.");
  if (tab.stats_status === "running" && !stats?.data) return muted("Running SUMMARIZE...");
  if (tab.stats_error) return <div className="p-2 font-mono text-[var(--fs-sm)] text-muted-foreground">{tab.stats_error}</div>;
  if (!stats?.data) return muted("...");

  const { columns, rows } = stats.data;
  const idx = (n: string) => columns.findIndex((c) => c.name === n);
  const nameI = idx("column_name");
  const typeI = idx("column_type");
  const statCols = columns.map((c, i) => ({ name: c.name, i })).filter((c) => c.i !== nameI && c.i !== typeI);
  // Histograms are cached per SUMMARIZE run, which embeds the statement.
  const runKey = `${tabId}\0${stats.data.executed_sql}`;

  return (
    <div className="font-mono text-[var(--fs-sm)]">
      {rows.map((r, ri) => {
        const name = text(r[nameI]) ?? "";
        const type = text(r[typeI]) ?? "";
        const showHist = histOpen.has(name);
        return (
          <div key={ri} className="border-b">
            <div
              className="group sticky top-0 z-10 flex h-[var(--row-h)] cursor-pointer items-center gap-1.5 bg-muted px-1.5 hover:bg-accent"
              onClick={() => jumpTo(tabId, name)}
              title="Show this column in the grid"
            >
              <span className="truncate font-sans font-medium">{name}</span>
              <span className="min-w-0 flex-1 truncate text-muted-foreground">{type}</span>
              <button
                title={showHist ? "Hide histogram" : "Show histogram"}
                className={cn(
                  "inline-flex size-4 shrink-0 items-center justify-center rounded-sm hover:bg-background",
                  showHist ? "text-foreground" : "text-muted-foreground opacity-60 group-hover:opacity-100",
                )}
                onClick={(e) => {
                  e.stopPropagation();
                  toggleHist(name);
                }}
              >
                <ChartColumn className="size-3" />
              </button>
            </div>
            {showHist && <HistogramView tabId={tabId} runKey={runKey} column={name} type={type} />}
            {statCols.map(({ name: stat, i }) => {
              const v = text(r[i]);
              if (v === null) return null; // e.g. avg for text columns
              const pct = stat === "null_percentage" ? Math.max(0, Math.min(100, Number.parseFloat(v))) : null;
              return (
                <div key={stat} className="flex h-[var(--row-h)] items-center gap-2 px-1.5 pl-3 hover:bg-accent/50">
                  <span className="w-24 shrink-0 text-muted-foreground">{stat}</span>
                  <span className={cn("relative min-w-0 flex-1 truncate text-right")} title={v}>
                    {pct !== null && !Number.isNaN(pct) && (
                      <span className="absolute top-[30%] bottom-[30%] left-0 rounded-sm bg-[var(--warning)]/35" style={{ width: `${pct}%` }} />
                    )}
                    <span className="relative">{v}</span>
                  </span>
                </div>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}

type HistState = { status: "loading" } | { status: "done"; data: Histogram } | { status: "error"; error: string };

/** Results by run key and column, so re-opening a histogram does not re-query. */
const histCache = new Map<string, Promise<Histogram>>();
const HIST_CACHE_MAX = 200;

function loadHistogram(tabId: string, runKey: string, column: string, type: string): Promise<Histogram> {
  const key = `${runKey}\0${column}`;
  let p = histCache.get(key);
  if (!p) {
    p = api.histogram(tabId, column, type);
    p.catch(() => histCache.delete(key));
    histCache.set(key, p);
    if (histCache.size > HIST_CACHE_MAX) histCache.delete(histCache.keys().next().value!);
  }
  return p;
}

const numFmt = new Intl.NumberFormat(undefined, { maximumSignificantDigits: 4 });
const countFmt = new Intl.NumberFormat();

function formatBound(kind: Histogram["kind"], v: Cell): string {
  if (typeof v !== "number") return text(v) ?? "";
  if (kind === "date") return new Date(v * 1000).toISOString().slice(0, 10);
  if (kind === "timestamp") return new Date(v * 1000).toISOString().slice(0, 19).replace("T", " ");
  return numFmt.format(v);
}

function HistogramView({ tabId, runKey, column, type }: { tabId: string; runKey: string; column: string; type: string }) {
  const [state, setState] = useState<HistState>({ status: "loading" });
  useEffect(() => {
    let live = true;
    setState({ status: "loading" });
    loadHistogram(tabId, runKey, column, type).then(
      (data) => live && setState({ status: "done", data }),
      (e) => live && setState({ status: "error", error: String(e) }),
    );
    return () => {
      live = false;
    };
  }, [tabId, runKey, column, type]);

  const box = (children: ReactNode) => <div className="border-b border-dashed px-1.5 py-1.5 pl-3">{children}</div>;
  if (state.status === "loading")
    return box(
      <div className="flex items-center gap-1.5 text-muted-foreground">
        <Loader2 className="spin size-3" /> Computing histogram...
      </div>,
    );
  if (state.status === "error") return box(<div className="text-muted-foreground">{state.error}</div>);

  const { kind, result } = state.data;
  const rows = result.rows;
  if (rows.length === 0) return box(<div className="text-muted-foreground">No non-null values.</div>);

  if (kind === "values") {
    const max = Math.max(...rows.map((r) => Number(r[2]) || 0), 1);
    return box(
      <div className="flex flex-col">
        {rows.map((r, i) => {
          const n = Number(r[2]) || 0;
          const other = r[3] === true;
          const label = other ? `(${r[1]} other values)` : r[1] === null ? "NULL" : String(r[1]);
          return (
            <div key={i} className="flex h-[var(--row-h)] items-center gap-2" title={`${label}: ${countFmt.format(n)}`}>
              <span className={cn("w-24 shrink-0 truncate", (other || r[1] === null) && "text-muted-foreground italic")}>{label}</span>
              <span className="relative h-full min-w-0 flex-1">
                <span
                  className="absolute top-[25%] bottom-[25%] left-0 rounded-sm bg-[var(--selection-border)]/45"
                  style={{ width: `${(n / max) * 100}%` }}
                />
              </span>
              <span className="w-14 shrink-0 text-right text-muted-foreground">{countFmt.format(n)}</span>
            </div>
          );
        })}
      </div>,
    );
  }

  const max = Math.max(...rows.map((r) => Number(r[3]) || 0), 1);
  const lo = formatBound(kind, rows[0][1]);
  const hi = formatBound(kind, rows[rows.length - 1][2]);
  return box(
    <div className="flex flex-col gap-0.5">
      <div className="flex h-20 items-end gap-px">
        {rows.map((r, i) => {
          const n = Number(r[3]) || 0;
          return (
            <div
              key={i}
              className="flex h-full min-w-0 flex-1 items-end hover:bg-accent/60"
              title={`${formatBound(kind, r[1])} - ${formatBound(kind, r[2])}: ${countFmt.format(n)}`}
            >
              <div
                className="w-full rounded-t-[1px] bg-[var(--selection-border)]/60"
                style={{ height: n > 0 ? `max(1px, ${(n / max) * 100}%)` : 0 }}
              />
            </div>
          );
        })}
      </div>
      <div className="flex justify-between gap-2 text-muted-foreground">
        <span className="truncate">{lo}</span>
        <span className="shrink-0" title="Rows in the tallest bin">peak {countFmt.format(max)}</span>
        <span className="truncate text-right">{hi}</span>
      </div>
    </div>,
  );
}
