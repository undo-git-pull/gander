import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, Check, Copy } from "lucide-react";
import type { Cell, Column, RowFormat, Selection, SortSpec } from "@/lib/api";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { asJson, copyText, formatRows, isNumericType, jsonTokens, prettyOrRaw, rawCopy } from "@/lib/format";
import { CellDetail, isJsonCell } from "@/features/cell-detail/CellDetail";
import { useDensity } from "@/lib/density";
import { cn } from "@/lib/utils";

export interface GridProps {
  /** Identity of the data set; widths and selection reset when it changes. */
  gridKey: string;
  columns: Column[];
  rows: Cell[][];
  sort?: SortSpec[];
  onSort?: (sort: SortSpec[]) => void;
  maxCellChars: number;
  rowFormat: RowFormat;
  onFocusCell?: (cell: { row: number; col: number } | null) => void;
  onSelection?: (sel: Selection | null) => void;
  onRowActivate?: (row: number) => void;
  onPin?: () => void;
  renderCell?: (col: Column, value: Cell, row: Cell[]) => ReactNode | undefined;
  highlight?: { column: string; nonce: number } | null;
  className?: string;
}

type Pos = { r: number; c: number };
type CtxTarget = { kind: "cell"; r: number; c: number } | { kind: "row"; r: number } | { kind: "header"; c: number } | null;

let measureCanvas: HTMLCanvasElement | null = null;
function charWidth(fontSize: number): number {
  measureCanvas ??= document.createElement("canvas");
  const ctx = measureCanvas.getContext("2d");
  if (!ctx) return fontSize * 0.6;
  ctx.font = `${fontSize}px Menlo, Monaco, Consolas, monospace`;
  return ctx.measureText("0000000000").width / 10;
}

const FORMATS: RowFormat[] = ["json", "tsv", "csv", "markdown"];
const FORMAT_LABEL: Record<RowFormat, string> = { json: "JSON", tsv: "TSV", csv: "CSV", markdown: "Markdown" };

export function DataGrid(p: GridProps) {
  const { rowH, hdrH, fontSize } = useDensity();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [widthOverride, setWidthOverride] = useState<Record<string, number>>({});
  const [focus, setFocus] = useState<Pos | null>(null);
  const [anchor, setAnchor] = useState<Pos | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [popover, setPopover] = useState<{ r: number; c: number; rect: DOMRect } | null>(null);
  const [ctx, setCtx] = useState<CtxTarget>(null);
  const [flashCol, setFlashCol] = useState<string | null>(null);
  const hoverTimer = useRef<number | null>(null);

  const colKey = p.columns.map((c) => c.name).join("\u0000");
  useEffect(() => {
    setFocus(null);
    setAnchor(null);
    setPopover(null);
  }, [p.gridKey]);
  useEffect(() => {
    setHidden(new Set());
    setWidthOverride({});
  }, [colKey]);

  // visible column indexes
  const vis = useMemo(() => p.columns.map((_, i) => i).filter((i) => !hidden.has(p.columns[i].name)), [p.columns, hidden]);

  const cw = useMemo(() => charWidth(fontSize), [fontSize]);
  const widths = useMemo(() => {
    const sample = p.rows.slice(0, 60);
    return vis.map((ci) => {
      const col = p.columns[ci];
      const o = widthOverride[col.name];
      if (o) return o;
      // header shows name and type
      let chars = col.name.length + Math.min(col.type.length, 14) + 3;
      for (const r of sample) {
        const v = r[ci];
        const len = v === null ? 4 : typeof v === "object" ? Math.min(JSON.stringify(v).length, 60) : String(v).length;
        if (len > chars) chars = len;
      }
      return Math.round(Math.min(Math.max(chars * cw + 18, 44), 340));
    });
  }, [vis, p.columns, p.rows, widthOverride, cw]);

  // Row number (click selects the row) plus a slot for the copy button.
  const COPY_W = 16;
  const gutterW = Math.max(28, String(p.rows.length).length * cw + 14) + COPY_W;

  const rowV = useVirtualizer({
    count: p.rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowH,
    overscan: 12,
  });
  const colV = useVirtualizer({
    horizontal: true,
    count: vis.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => widths[i] ?? 100,
    overscan: 4,
  });
  useLayoutEffect(() => {
    colV.measure();
  }, [widths, colV]);
  useLayoutEffect(() => {
    rowV.measure();
  }, [rowH, rowV]);

  // scroll to and flash a column (from the stats panel)
  useEffect(() => {
    if (!p.highlight) return;
    const idx = vis.findIndex((ci) => p.columns[ci].name === p.highlight!.column);
    if (idx < 0) return;
    colV.scrollToIndex(idx, { align: "center" });
    setFlashCol(p.highlight.column);
    const t = setTimeout(() => setFlashCol(null), 1600);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.highlight?.nonce]);

  const sel = useMemo(() => {
    if (!focus || !anchor) return null;
    return {
      r0: Math.min(focus.r, anchor.r),
      r1: Math.max(focus.r, anchor.r),
      c0: Math.min(focus.c, anchor.c),
      c1: Math.max(focus.c, anchor.c),
    };
  }, [focus, anchor]);

  const { onSelection, onFocusCell } = p;
  useEffect(() => {
    const t = setTimeout(() => {
      onSelection?.(
        sel && vis.length
          ? {
              row_start: sel.r0,
              row_end: sel.r1,
              col_start: Math.min(vis[sel.c0], vis[sel.c1]),
              col_end: Math.max(vis[sel.c0], vis[sel.c1]),
            }
          : null,
      );
    }, 150);
    return () => clearTimeout(t);
  }, [sel, vis, onSelection]);
  useEffect(() => {
    onFocusCell?.(focus && vis[focus.c] !== undefined ? { row: focus.r, col: vis[focus.c] } : null);
  }, [focus, vis, onFocusCell]);

  const flash = (key: string) => {
    setCopied(key);
    setTimeout(() => setCopied((k) => (k === key ? null : k)), 1000);
  };

  const copyCell = async (r: number, c: number, pretty: boolean) => {
    const ci = vis[c];
    const v = p.rows[r][ci];
    await copyText(pretty ? prettyOrRaw(v, p.columns[ci].type) : rawCopy(v));
    flash(`${r}:${c}`);
  };
  const copyRow = async (r: number, fmt: RowFormat) => {
    const cols = vis.map((ci) => p.columns[ci]);
    await copyText(formatRows(cols, [vis.map((ci) => p.rows[r][ci])], fmt));
    flash(`row:${r}`);
  };
  const selectionText = (fmt: RowFormat): string | null => {
    if (!sel) return null;
    if (sel.r0 === sel.r1 && sel.c0 === sel.c1) return rawCopy(p.rows[sel.r0][vis[sel.c0]]);
    const ci = vis.slice(sel.c0, sel.c1 + 1);
    const cols = ci.map((i) => p.columns[i]);
    const rows = p.rows.slice(sel.r0, sel.r1 + 1).map((r) => ci.map((i) => r[i]));
    return formatRows(cols, rows, fmt);
  };

  const scrollIntoView = (pos: Pos) => {
    rowV.scrollToIndex(pos.r);
    colV.scrollToIndex(pos.c);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!p.rows.length || !vis.length) return;
    const mod = e.metaKey || e.ctrlKey;
    const cur = focus ?? { r: 0, c: 0 };
    let next: Pos | null = null;
    const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? 200) / rowH) - 1);
    switch (e.key) {
      case "ArrowDown":
        next = { r: mod ? p.rows.length - 1 : Math.min(cur.r + 1, p.rows.length - 1), c: cur.c };
        break;
      case "ArrowUp":
        next = { r: mod ? 0 : Math.max(cur.r - 1, 0), c: cur.c };
        break;
      case "ArrowRight":
        next = { r: cur.r, c: mod ? vis.length - 1 : Math.min(cur.c + 1, vis.length - 1) };
        break;
      case "ArrowLeft":
        next = { r: cur.r, c: mod ? 0 : Math.max(cur.c - 1, 0) };
        break;
      case "PageDown":
        next = { r: Math.min(cur.r + page, p.rows.length - 1), c: cur.c };
        break;
      case "PageUp":
        next = { r: Math.max(cur.r - page, 0), c: cur.c };
        break;
      case "Home":
        next = { r: mod ? 0 : cur.r, c: 0 };
        break;
      case "End":
        next = { r: mod ? p.rows.length - 1 : cur.r, c: vis.length - 1 };
        break;
      case " ": {
        e.preventDefault();
        if (!focus) return;
        const el = scrollRef.current?.querySelector(`[data-cell="${focus.r}:${focus.c}"]`);
        if (el) setPopover((pp) => (pp ? null : { r: focus.r, c: focus.c, rect: el.getBoundingClientRect() }));
        return;
      }
      case "Enter":
        if (focus) {
          e.preventDefault();
          setPopover(null);
          p.onRowActivate?.(focus.r);
          p.onPin?.();
        }
        return;
      case "Escape":
        if (popover) {
          e.stopPropagation();
          setPopover(null);
        }
        return;
      case "a":
        if (mod) {
          e.preventDefault();
          setAnchor({ r: 0, c: 0 });
          setFocus({ r: p.rows.length - 1, c: vis.length - 1 });
        }
        return;
      case "c":
      case "C":
        if (mod && e.shiftKey && focus) {
          e.preventDefault();
          void copyRow(focus.r, p.rowFormat);
        }
        return;
      default:
        return;
    }
    if (next) {
      e.preventDefault();
      setFocus(next);
      if (!e.shiftKey) setAnchor(next);
      else if (!anchor) setAnchor(cur);
      setPopover(null);
      scrollIntoView(next);
    }
  };

  const onCopy = (e: React.ClipboardEvent) => {
    const text = selectionText("tsv");
    if (text == null) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", text);
    if (sel && sel.r0 === sel.r1 && sel.c0 === sel.c1) flash(`${sel.r0}:${sel.c0}`);
  };

  const onCellDown = (e: React.MouseEvent, r: number, c: number) => {
    if (e.button !== 0) return;
    scrollRef.current?.focus({ preventScroll: true });
    const pos = { r, c };
    setFocus(pos);
    if (!e.shiftKey || !anchor) setAnchor(pos);
  };

  const onRowDown = (e: React.MouseEvent, r: number) => {
    if (e.button !== 0) return;
    scrollRef.current?.focus({ preventScroll: true });
    if (e.shiftKey && anchor) {
      setFocus({ r, c: vis.length - 1 });
    } else {
      setAnchor({ r, c: 0 });
      setFocus({ r, c: vis.length - 1 });
    }
    p.onRowActivate?.(r);
  };

  const cancelHover = () => {
    if (hoverTimer.current) window.clearTimeout(hoverTimer.current);
    hoverTimer.current = null;
  };
  const onCellEnter = (e: React.MouseEvent, r: number, c: number) => {
    cancelHover();
    const ci = vis[c];
    if (!isJsonCell(p.rows[r][ci], p.columns[ci].type)) return;
    const el = e.currentTarget as HTMLElement;
    hoverTimer.current = window.setTimeout(() => setPopover({ r, c, rect: el.getBoundingClientRect() }), 400);
  };
  const onCellLeave = () => cancelHover();

  const headerClick = (e: React.MouseEvent, ci: number) => {
    if (!p.onSort) return;
    const name = p.columns[ci].name;
    const cur = p.sort ?? [];
    const existing = cur.find((s) => s.column === name);
    const cycle = (s?: SortSpec): SortSpec | null => (!s ? { column: name, dir: "asc" } : s.dir === "asc" ? { column: name, dir: "desc" } : null);
    let next: SortSpec[];
    if (e.shiftKey) {
      const n = cycle(existing);
      next = existing ? (n ? cur.map((s) => (s.column === name ? n : s)) : cur.filter((s) => s.column !== name)) : [...cur, n!];
    } else {
      const n = cycle(existing);
      next = n ? [n] : [];
    }
    p.onSort(next);
  };

  const startResize = (e: React.PointerEvent, ci: number, startW: number) => {
    e.preventDefault();
    e.stopPropagation();
    const x0 = e.clientX;
    const name = p.columns[ci].name;
    const move = (ev: PointerEvent) => setWidthOverride((w) => ({ ...w, [name]: Math.max(36, startW + ev.clientX - x0) }));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const totalW = gutterW + colV.getTotalSize();
  const totalH = hdrH + rowV.getTotalSize();
  const vRows = rowV.getVirtualItems();
  const vCols = colV.getVirtualItems();

  const renderValue = (col: Column, v: Cell, row: Cell[]): ReactNode => {
    const custom = p.renderCell?.(col, v, row);
    if (custom !== undefined) return custom;
    if (v === null) return <span className="g-null">NULL</span>;
    if (typeof v === "boolean") return <span className="j-lit">{String(v)}</span>;
    const j = asJson(v, col.type);
    if (j !== undefined) {
      const text = (typeof v === "string" ? v : JSON.stringify(v)).slice(0, p.maxCellChars);
      return jsonTokens(text).map((t, i) => (
        <span key={i} className={t.c}>
          {t.t}
        </span>
      ));
    }
    const s = String(v);
    const cut = s.length > p.maxCellChars ? s.slice(0, p.maxCellChars) + "…" : s;
    return cut.replace(/\r?\n/g, "↵");
  };

  const popCell = popover && p.rows[popover.r] ? { col: p.columns[vis[popover.c]], v: p.rows[popover.r][vis[popover.c]] } : null;

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={scrollRef}
          tabIndex={0}
          onKeyDown={onKeyDown}
          onCopy={onCopy}
          className={cn("g-scroll relative h-full w-full overflow-auto outline-none", p.className)}
        >
          <div style={{ width: totalW, height: totalH, position: "relative", minWidth: "100%" }}>
            {/* header */}
            <div className="sticky top-0 z-20 flex bg-muted" style={{ height: hdrH, width: totalW }}>
              <div
                className="g-hdr sticky left-0 z-30 shrink-0 border-r border-b bg-muted"
                style={{ width: gutterW }}
              />
              {vCols.map((vc) => {
                const ci = vis[vc.index];
                const col = p.columns[ci];
                const s = p.sort?.findIndex((x) => x.column === col.name) ?? -1;
                const spec = s >= 0 ? p.sort![s] : null;
                const num = isNumericType(col.type);
                return (
                  <div
                    key={vc.key}
                    onClick={(e) => headerClick(e, ci)}
                    onContextMenu={() => setCtx({ kind: "header", c: vc.index })}
                    title={`${col.name} ${col.type}${p.onSort ? "\nClick to sort, shift click to add a sort column" : ""}`}
                    className={cn(
                      "g-hdr absolute top-0 flex items-center gap-1 overflow-hidden border-r border-b px-[var(--cell-px)] font-sans",
                      p.onSort && "hover:bg-accent",
                      num && "flex-row-reverse",
                      flashCol === col.name && "bg-[var(--highlight)]",
                    )}
                    style={{ left: gutterW + vc.start, width: vc.size }}
                  >
                    <span className="max-w-full shrink-0 truncate font-medium">{col.name}</span>
                    <span className="min-w-0 truncate text-[var(--fs-sm)] text-muted-foreground">{col.type}</span>
                    {spec && (
                      <span className="flex shrink-0 items-center text-foreground">
                        {spec.dir === "asc" ? <ArrowUp className="size-3" /> : <ArrowDown className="size-3" />}
                        {(p.sort?.length ?? 0) > 1 && <span className="text-[10px]">{s + 1}</span>}
                      </span>
                    )}
                    <div
                      className="absolute top-0 right-0 h-full w-1.5 cursor-col-resize hover:bg-ring/40"
                      onPointerDown={(e) => startResize(e, ci, vc.size)}
                      onClick={(e) => e.stopPropagation()}
                      onDoubleClick={(e) => {
                        e.stopPropagation();
                        setWidthOverride((w) => {
                          const n = { ...w };
                          delete n[col.name];
                          return n;
                        });
                      }}
                    />
                  </div>
                );
              })}
            </div>

            {/* rows */}
            {vRows.map((vr) => {
              const r = vr.index;
              const row = p.rows[r];
              const rowSel = sel && r >= sel.r0 && r <= sel.r1;
              return (
                <div
                  key={vr.key}
                  className="group/row absolute left-0 flex"
                  style={{ top: hdrH + vr.start, height: rowH, width: totalW }}
                >
                  <div
                    className={cn(
                      "g-cell sticky left-0 z-10 flex shrink-0 items-center bg-muted/80 pr-0! text-muted-foreground backdrop-blur-sm",
                      rowSel && "text-foreground",
                    )}
                    style={{ width: gutterW }}
                    title="Select row (shift-click to extend)"
                    onMouseDown={(e) => onRowDown(e, r)}
                    onContextMenu={() => setCtx({ kind: "row", r })}
                  >
                    <span className="min-w-0 flex-1 truncate text-right group-hover/row:text-foreground">{r + 1}</span>
                    <button
                      title={`Copy row as ${FORMAT_LABEL[p.rowFormat]} (right click for more)`}
                      className={cn(
                        "flex h-full shrink-0 items-center justify-center rounded-sm hover:bg-accent hover:text-foreground",
                        copied === `row:${r}` ? "visible" : "invisible group-hover/row:visible",
                      )}
                      style={{ width: COPY_W }}
                      onMouseDown={(e) => e.stopPropagation()}
                      onClick={() => void copyRow(r, p.rowFormat)}
                    >
                      {copied === `row:${r}` ? <Check className="size-3" /> : <Copy className="size-3" />}
                    </button>
                  </div>
                  {vCols.map((vc) => {
                    const c = vc.index;
                    const ci = vis[c];
                    const col = p.columns[ci];
                    const v = row[ci];
                    const inSel = sel && rowSel && c >= sel.c0 && c <= sel.c1;
                    const isFocus = focus && focus.r === r && focus.c === c;
                    const num = isNumericType(col.type) && typeof v !== "object";
                    const key = `${r}:${c}`;
                    return (
                      <div
                        key={vc.key}
                        data-cell={key}
                        className={cn(
                          "g-cell group/cell absolute top-0",
                          num && "text-right",
                          inSel && "bg-[var(--selection)]",
                          isFocus && "outline outline-1 -outline-offset-1 outline-[var(--selection-border)]",
                          flashCol === col.name && !inSel && "bg-[var(--highlight)]",
                        )}
                        style={{ left: gutterW + vc.start, width: vc.size }}
                        onMouseDown={(e) => onCellDown(e, r, c)}
                        onMouseEnter={(e) => onCellEnter(e, r, c)}
                        onMouseLeave={onCellLeave}
                        onContextMenu={() => {
                          if (!inSel) {
                            setFocus({ r, c });
                            setAnchor({ r, c });
                          }
                          setCtx({ kind: "cell", r, c });
                        }}
                        onDoubleClick={() => {
                          const el = scrollRef.current?.querySelector(`[data-cell="${key}"]`);
                          if (el) setPopover({ r, c, rect: el.getBoundingClientRect() });
                        }}
                      >
                        {renderValue(col, v, row)}
                        <button
                          title="Copy value (alt click: pretty JSON)"
                          className={cn(
                            "absolute top-1/2 right-0.5 hidden size-4 -translate-y-1/2 items-center justify-center rounded-sm border bg-background text-muted-foreground shadow-xs hover:text-foreground",
                            "group-hover/cell:flex",
                            (isFocus || copied === key) && "flex",
                          )}
                          onMouseDown={(e) => e.stopPropagation()}
                          onClick={(e) => void copyCell(r, c, e.altKey)}
                        >
                          {copied === key ? <Check className="size-3" /> : <Copy className="size-3" />}
                        </button>
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="min-w-44 text-xs">
        {ctx?.kind === "row" &&
          FORMATS.map((f) => (
            <ContextMenuItem key={f} className="py-1 text-xs" onSelect={() => void copyRow(ctx.r, f)}>
              Copy row as {FORMAT_LABEL[f]}
            </ContextMenuItem>
          ))}
        {ctx?.kind === "cell" && (
          <>
            <ContextMenuItem className="py-1 text-xs" onSelect={() => void copyCell(ctx.r, ctx.c, false)}>
              Copy value
            </ContextMenuItem>
            <ContextMenuItem className="py-1 text-xs" onSelect={() => void copyCell(ctx.r, ctx.c, true)}>
              Copy value pretty-printed
            </ContextMenuItem>
            <ContextMenuSeparator />
            {FORMATS.map((f) => (
              <ContextMenuItem key={f} className="py-1 text-xs" onSelect={() => void copyText(selectionText(f) ?? "")}>
                Copy selection as {FORMAT_LABEL[f]}
              </ContextMenuItem>
            ))}
            <ContextMenuSeparator />
            {FORMATS.map((f) => (
              <ContextMenuItem key={`r${f}`} className="py-1 text-xs" onSelect={() => void copyRow(ctx.r, f)}>
                Copy row as {FORMAT_LABEL[f]}
              </ContextMenuItem>
            ))}
          </>
        )}
        {ctx?.kind === "header" && (
          <>
            <ContextMenuItem className="py-1 text-xs" onSelect={() => void copyText(p.columns[vis[ctx.c]].name)}>
              Copy column name
            </ContextMenuItem>
            {p.onSort && (
              <>
                <ContextMenuItem className="py-1 text-xs" onSelect={() => p.onSort!([{ column: p.columns[vis[ctx.c]].name, dir: "asc" }])}>
                  Sort ascending
                </ContextMenuItem>
                <ContextMenuItem className="py-1 text-xs" onSelect={() => p.onSort!([{ column: p.columns[vis[ctx.c]].name, dir: "desc" }])}>
                  Sort descending
                </ContextMenuItem>
                {(p.sort?.length ?? 0) > 0 && (
                  <ContextMenuItem className="py-1 text-xs" onSelect={() => p.onSort!([])}>
                    Clear sort
                  </ContextMenuItem>
                )}
              </>
            )}
            <ContextMenuSeparator />
            <ContextMenuItem
              className="py-1 text-xs"
              disabled={vis.length <= 1}
              onSelect={() => setHidden((h) => new Set(h).add(p.columns[vis[ctx.c]].name))}
            >
              Hide column
            </ContextMenuItem>
            {hidden.size > 0 && (
              <ContextMenuItem className="py-1 text-xs" onSelect={() => setHidden(new Set())}>
                Show all columns ({hidden.size} hidden)
              </ContextMenuItem>
            )}
          </>
        )}
      </ContextMenuContent>
      {popover && popCell && (
        <CellPopover
          rect={popover.rect}
          onClose={() => {
            setPopover(null);
            scrollRef.current?.focus({ preventScroll: true });
          }}
          onPin={
            p.onPin
              ? () => {
                  setFocus({ r: popover.r, c: popover.c });
                  setAnchor({ r: popover.r, c: popover.c });
                  setPopover(null);
                  p.onPin!();
                }
              : undefined
          }
        >
          {(close, pin) => <CellDetail column={popCell.col.name} type={popCell.col.type} value={popCell.v} onClose={close} onPin={pin} className="h-full" />}
        </CellPopover>
      )}
    </ContextMenu>
  );
}

function CellPopover({
  rect,
  onClose,
  onPin,
  children,
}: {
  rect: DOMRect;
  onClose: () => void;
  onPin?: () => void;
  children: (close: () => void, pin?: () => void) => ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const w = 460;
  const h = 320;
  const left = Math.max(4, Math.min(rect.left, window.innerWidth - w - 4));
  const below = rect.bottom + h + 4 < window.innerHeight;
  const top = below ? rect.bottom + 2 : Math.max(4, rect.top - h - 2);
  const close = useCallback(() => onClose(), [onClose]);
  useEffect(() => {
    const down = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("mousedown", down, true);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("mousedown", down, true);
      window.removeEventListener("keydown", key);
    };
  }, [close]);
  return createPortal(
    <div
      ref={ref}
      className="fixed z-50 overflow-hidden rounded-md border bg-popover text-popover-foreground shadow-lg"
      style={{ left, top, width: w, height: h }}
    >
      {children(close, onPin)}
    </div>,
    document.body,
  );
}
