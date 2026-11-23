import { useEffect, useMemo, useRef, useState } from "react";
import { Fzf, extendedMatch, type FzfResultItem } from "fzf";
import { ChevronDown, ChevronRight, Columns3, Eye, Table2 } from "lucide-react";
import { api, type CatalogTable } from "@/lib/api";
import { useStore } from "@/store";
import { editorBridge } from "@/features/editor/SqlEditor";
import { quoteIdent } from "@/lib/sql";
import { cn } from "@/lib/utils";

type Item =
  | { kind: "table"; key: string; label: string; table: CatalogTable }
  | { kind: "column"; key: string; label: string; table: CatalogTable; column: string; type: string };

export const focusSchemaFilter = () => document.getElementById("schema-filter")?.focus();

function qualified(t: CatalogTable, catalog: CatalogTable[]): string {
  const dbs = new Set(catalog.map((c) => c.database));
  const parts = dbs.size > 1 ? [t.database, t.schema, t.name] : t.schema === "main" ? [t.name] : [t.schema, t.name];
  return parts.map(quoteIdent).join(".");
}

/** Run `FROM <table>`: in the current tab if it is empty, else in a new tab. */
async function openTable(t: CatalogTable, catalog: CatalogTable[]) {
  const sql = `FROM ${qualified(t, catalog)}`;
  const s = useStore.getState();
  if (editorBridge.isEmpty()) {
    editorBridge.replaceAll(sql);
    await api.setSql(s.active, sql);
    await api.run(s.active);
  } else {
    await api.newTab(sql, true);
  }
}

export function Sidebar() {
  const catalog = useStore((s) => s.catalog);
  const catalogError = useStore((s) => s.catalogError);
  const source = useStore((s) => s.source);
  const [q, setQ] = useState("");
  const [cursor, setCursor] = useState(0);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const listRef = useRef<HTMLDivElement>(null);

  const items = useMemo<Item[]>(() => {
    const out: Item[] = [];
    for (const t of catalog ?? []) {
      const tk = `${t.database}.${t.schema}.${t.name}`;
      out.push({ kind: "table", key: tk, label: `${t.schema}.${t.name}`, table: t });
      for (const c of t.columns) out.push({ kind: "column", key: `${tk}.${c.name}`, label: `${t.name}.${c.name}`, table: t, column: c.name, type: c.type });
    }
    return out;
  }, [catalog]);

  const fzf = useMemo(() => new Fzf(items, { selector: (i: Item) => i.label, match: extendedMatch, limit: 300 }), [items]);
  const results: FzfResultItem<Item>[] = useMemo(() => (q.trim() ? fzf.find(q.trim()) : []), [fzf, q]);

  useEffect(() => setCursor(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-idx="${cursor}"]`)?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  const activate = (item: Item) => {
    if (item.kind === "table") void openTable(item.table, catalog ?? []);
    else editorBridge.insertAtCursor(quoteIdent(item.column));
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const r = results[cursor];
      if (r) activate(r.item);
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (q) setQ("");
      else editorBridge.focus();
    }
  };

  const hasTables = (catalog?.length ?? 0) > 0;
  const showFilter = !!source?.has_catalog || hasTables;

  return (
    <div className="flex h-full min-h-0 flex-col">
      {showFilter && (
        <div className="shrink-0 border-b p-1">
          <input
            id="schema-filter"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={onKey}
            placeholder="Filter (Cmd+P)  e.g. ord am"
            spellCheck={false}
            autoComplete="off"
            className="h-[22px] w-full rounded-sm border bg-background px-1.5 font-mono text-[var(--fs-sm)] outline-none placeholder:text-muted-foreground focus:border-ring"
          />
        </div>
      )}
      <div ref={listRef} className="min-h-0 flex-1 overflow-auto py-0.5 font-mono text-[var(--fs-sm)]">
        {catalogError && <div className="px-2 py-1 text-destructive">{catalogError}</div>}
        {q.trim() ? (
          results.length === 0 ? (
            <div className="px-2 py-1 text-muted-foreground">No matches</div>
          ) : (
            results.map((r, i) => (
              <div
                key={r.item.key}
                data-idx={i}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setCursor(i);
                }}
                onDoubleClick={() => activate(r.item)}
                className={cn("flex h-[var(--row-h)] items-center gap-1 px-1.5 whitespace-nowrap", i === cursor && "bg-accent")}
                title={r.item.kind === "table" ? "Enter: run FROM table" : "Enter: insert column name"}
              >
                {r.item.kind === "table" ? <TableIcon kind={r.item.table.kind} /> : <Columns3 className="size-3 shrink-0 text-muted-foreground" />}
                <Highlighted text={r.item.label} positions={r.positions} />
                {r.item.kind === "column" && <span className="ml-auto pl-2 text-muted-foreground">{r.item.type}</span>}
              </div>
            ))
          )
        ) : hasTables ? (
          <Tree catalog={catalog!} open={open} setOpen={setOpen} />
        ) : (
          <FilesHint />
        )}
        {!q.trim() && hasTables && !source?.has_catalog && <FilesHint compact />}
      </div>
    </div>
  );
}

function TableIcon({ kind }: { kind: string }) {
  return kind === "view" ? <Eye className="size-3 shrink-0 text-muted-foreground" /> : <Table2 className="size-3 shrink-0 text-muted-foreground" />;
}

function Highlighted({ text, positions }: { text: string; positions: Set<number> }) {
  return (
    <span className="truncate">
      {[...text].map((ch, i) =>
        positions.has(i) ? (
          <b key={i} className="font-semibold text-[var(--syn-number)]">
            {ch}
          </b>
        ) : (
          <span key={i}>{ch}</span>
        ),
      )}
    </span>
  );
}

function Tree({
  catalog,
  open,
  setOpen,
}: {
  catalog: CatalogTable[];
  open: Record<string, boolean>;
  setOpen: (f: (o: Record<string, boolean>) => Record<string, boolean>) => void;
}) {
  const groups = useMemo(() => {
    const m = new Map<string, CatalogTable[]>();
    for (const t of catalog) {
      const k = `${t.database}.${t.schema}`;
      if (!m.has(k)) m.set(k, []);
      m.get(k)!.push(t);
    }
    return [...m.entries()];
  }, [catalog]);
  const toggle = (k: string, def: boolean) => setOpen((o) => ({ ...o, [k]: !(o[k] ?? def) }));
  return (
    <div>
      {groups.map(([g, tables]) => {
        const gOpen = open[g] ?? true;
        return (
          <div key={g}>
            <Row depth={0} onClick={() => toggle(g, true)}>
              {gOpen ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
              <span className="truncate text-muted-foreground">{g}</span>
            </Row>
            {gOpen &&
              tables.map((t) => {
                const k = `${g}.${t.name}`;
                const tOpen = open[k] ?? false;
                return (
                  <div key={k}>
                    <Row depth={1} onClick={() => toggle(k, false)} onDoubleClick={() => void openTable(t, catalog)} title="Double click: run FROM table">
                      {tOpen ? <ChevronDown className="size-3 shrink-0" /> : <ChevronRight className="size-3 shrink-0" />}
                      <TableIcon kind={t.kind} />
                      <span className="truncate">{t.name}</span>
                      <span className="ml-auto pl-1 text-muted-foreground">{t.columns.length}</span>
                    </Row>
                    {tOpen &&
                      t.columns.map((c) => (
                        <Row key={c.name} depth={2} onDoubleClick={() => editorBridge.insertAtCursor(quoteIdent(c.name))} title="Double click: insert column name">
                          <span className="truncate">{c.name}</span>
                          <span className="ml-auto truncate pl-2 text-muted-foreground">{c.type}</span>
                        </Row>
                      ))}
                  </div>
                );
              })}
          </div>
        );
      })}
    </div>
  );
}

function Row({
  depth,
  children,
  onClick,
  onDoubleClick,
  title,
}: {
  depth: number;
  children: React.ReactNode;
  onClick?: () => void;
  onDoubleClick?: () => void;
  title?: string;
}) {
  return (
    <div
      title={title}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      className="flex h-[var(--row-h)] items-center gap-1 pr-1.5 whitespace-nowrap hover:bg-accent"
      style={{ paddingLeft: 4 + depth * 12 }}
    >
      {children}
    </div>
  );
}

function FilesHint({ compact }: { compact?: boolean }) {
  const ws = useStore((s) => s.workspace);
  return (
    <div className={cn("space-y-1 px-2 py-1.5 text-muted-foreground", compact && "mt-2 border-t pt-2")}>
      {!compact && <div>No database open. Query files directly:</div>}
      <div className="font-mono text-foreground/80">FROM 'events.parquet'</div>
      <div className="font-mono text-foreground/80">FROM 'logs.jsonl'</div>
      <div className="font-mono text-foreground/80">FROM read_csv('data/*.csv')</div>
      {ws && !compact && <div className="break-all">Relative paths resolve against {ws}</div>}
      {!compact && <div>Drop a file on the window to open it.</div>}
    </div>
  );
}
