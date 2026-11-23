import { useCallback, useState } from "react";
import { api, type Selection, type SortSpec } from "@/lib/api";
import { fmtInt } from "@/lib/format";
import { useStore } from "@/store";
import { DataGrid } from "./DataGrid";
import { CellDetail } from "@/features/cell-detail/CellDetail";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { useLayout } from "@/lib/layout";

export function ResultArea() {
  return <GridWithDetail />;
}

function GridWithDetail() {
  const tabId = useStore((s) => s.active);
  const tab = useStore((s) => s.tabs[s.active]);
  const result = useStore((s) => s.results[s.active]);
  const cfg = useStore((s) => s.config);
  const pinned = useStore((s) => s.detailPinned);
  const setPinned = useStore((s) => s.setDetailPinned);
  const jump = useStore((s) => s.jump);
  const [focusCell, setFocusCell] = useState<{ row: number; col: number } | null>(null);
  const layout = useLayout("detail");

  const onSort = useCallback((sort: SortSpec[]) => void api.setSort(tabId, sort), [tabId]);
  const onSelection = useCallback((sel: Selection | null) => void api.setSelection(tabId, sel), [tabId]);

  if (!tab) return null;
  const data = result?.data;
  const body = (() => {
    if (tab.status === "error") return <ErrorView tabId={tabId} code={tab.error_code} message={tab.error ?? ""} />;
    if (!data) {
      if (tab.status === "running") return <Muted>Running...</Muted>;
      if (tab.row_count != null) return <Muted>Last run returned {fmtInt(tab.row_count)} rows. Run again to load results (Cmd+Enter).</Muted>;
      return <Muted>Run a query to see results (Cmd+Enter).</Muted>;
    }
    if (data.columns.length === 0) return <Muted>Statement completed{data.statement ? `: ${data.statement.split("\n")[0].slice(0, 80)}` : ""}</Muted>;
    return (
      <DataGrid
        gridKey={`${tabId}:${result!.version}`}
        columns={data.columns}
        rows={data.rows}
        sort={data.wrapped ? tab.sort : undefined}
        onSort={data.wrapped ? onSort : undefined}
        maxCellChars={cfg?.ui.max_cell_chars ?? 200}
        rowFormat={cfg?.ui.copy_row_format ?? "json"}
        onFocusCell={setFocusCell}
        onSelection={onSelection}
        onPin={() => setPinned(true)}
        highlight={jump && jump.tab === tabId ? { column: jump.column, nonce: jump.nonce } : null}
      />
    );
  })();

  const grid = (
    <div className="flex h-full min-h-0 flex-col">
      <div className="relative min-h-0 flex-1">
        {body}
        {tab.status === "running" && data && <div className="pointer-events-none absolute inset-x-0 top-0 h-0.5 animate-pulse bg-[var(--selection-border)]" />}
      </div>
      {data?.truncated && tab.status !== "error" && (
        <div className="flex h-[22px] shrink-0 items-center gap-2 border-t bg-[var(--warning)]/10 px-2 text-[var(--fs-sm)]">
          <span className="font-medium text-[var(--warning)]">Truncated at {fmtInt(data.limit)} rows.</span>
          <span className="text-muted-foreground">Sorting and stats apply to the full query.</span>
          <button className="ml-auto underline-offset-2 hover:underline disabled:opacity-60" disabled={tab.counting} onClick={() => void api.countRows(tabId)}>
            {tab.counting ? "Counting..." : tab.total_count != null ? `${fmtInt(tab.total_count)} rows in total` : "Count all rows"}
          </button>
        </div>
      )}
    </div>
  );

  if (!pinned || !data) return grid;
  const cell = focusCell && data.rows[focusCell.row] ? { col: data.columns[focusCell.col], v: data.rows[focusCell.row][focusCell.col] } : null;
  return (
    <ResizablePanelGroup orientation="horizontal" defaultLayout={layout.initial} onLayoutChanged={layout.save}>
      <ResizablePanel id="grid" minSize={120}>
        {grid}
      </ResizablePanel>
      <ResizableHandle />
      <ResizablePanel id="detail" defaultSize="30" minSize={160}>
        {cell && cell.col ? (
          <CellDetail column={cell.col.name} type={cell.col.type} value={cell.v} onClose={() => setPinned(false)} className="h-full" />
        ) : (
          <div className="flex h-full flex-col">
            <div className="flex h-[var(--bar-h)] items-center justify-between border-b px-1.5">
              <span className="text-muted-foreground">Cell detail</span>
              <button className="text-muted-foreground hover:text-foreground" onClick={() => setPinned(false)}>
                close
              </button>
            </div>
            <Muted>Select a cell.</Muted>
          </div>
        )}
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}

function Muted({ children }: { children: React.ReactNode }) {
  return <div className="p-2 text-muted-foreground">{children}</div>;
}

function ErrorView({ tabId, code, message }: { tabId: string; code: string | null; message: string }) {
  return (
    <div className="h-full overflow-auto p-2">
      <pre className="font-mono text-[var(--fs-sm)] whitespace-pre-wrap text-destructive">{message}</pre>
      {code === "too_many_rows" && (
        <button className="mt-2 rounded-sm border px-2 py-0.5 hover:bg-accent" onClick={() => void api.rerun(tabId, true)}>
          Run anyway
        </button>
      )}
    </div>
  );
}
