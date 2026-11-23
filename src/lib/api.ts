// Typed wrappers for Tauri commands and events.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Origin = "user" | "agent" | "file" | "restore";
export type RunStatus = "idle" | "running" | "done" | "error";
export type SortDir = "asc" | "desc";
export interface SortSpec {
  column: string;
  dir: SortDir;
}

export interface TabSummary {
  id: string;
  title: string;
  custom_title: boolean;
  sql: string;
  sql_rev: number;
  sql_origin: Origin;
  dirty: boolean;
  status: RunStatus;
  error: string | null;
  error_code: string | null;
  sort: SortSpec[];
  limit: number;
  limit_override: number | null;
  row_count: number | null;
  truncated: boolean;
  total_count: number | null;
  counting: boolean;
  elapsed_ms: number | null;
  executed_sql: string | null;
  wrapped: boolean;
  has_result: boolean;
  result_version: number;
  stats_open: boolean;
  stats_status: RunStatus;
  stats_error: string | null;
  stats_version: number;
  last_change: Origin;
}

export interface Column {
  name: string;
  type: string;
}

/**
 * numeric/date/timestamp rows: [bin, lo, hi, n] (date and timestamp bounds in epoch seconds).
 * values rows: [rank, label, n, other] where `other` rows group the remaining distinct values.
 */
export interface Histogram {
  kind: "numeric" | "date" | "timestamp" | "values";
  result: QueryResult;
}

export type Cell = null | boolean | number | string | Cell[] | { [k: string]: Cell };

export interface QueryResult {
  columns: Column[];
  rows: Cell[][];
  truncated: boolean;
  executed_sql: string;
  statement: string;
  wrapped: boolean;
  sort: SortSpec[];
  limit: number;
  elapsed_ms: number;
}

export type SourceSpec = { kind: "memory" } | { kind: "configured"; name: string } | { kind: "file"; path: string };

export type SourceInfo = SourceSpec & {
  label: string;
  path: string | null;
  read_only: boolean;
  has_catalog: boolean;
  warnings: string[];
};

export interface ConfigError {
  file: string;
  line: number | null;
  message: string;
}

export type RowFormat = "json" | "tsv" | "csv" | "markdown";

export interface Config {
  default_connection: string | null;
  server: { enabled: boolean; port: number; default_rows: number; max_rows: number };
  ui: {
    theme: "system" | "light" | "dark";
    density: "compact" | "cozy";
    font_size: number;
    max_cell_chars: number;
    copy_row_format: RowFormat;
  };
  editor: { vim: boolean; tab_size: number };
  query: { limit: number; timeout_ms: number };
  keybindings: { run: string; run_statement: string; palette: string };
  connections: { name: string; path: string }[];
}

export interface ApiStatus {
  enabled: boolean;
  running: boolean;
  port: number | null;
  auth_required: boolean;
  error: string | null;
}

export interface WorkspaceTrust {
  hash: string | null;
  trusted: boolean;
  needs_trust: boolean;
  ignored: string[];
}

export interface Snapshot {
  tabs: TabSummary[];
  active: string;
  config: Config;
  config_errors: ConfigError[];
  source: SourceInfo;
  source_error: string | null;
  connections: string[];
  recents: string[];
  api: ApiStatus;
  workspace: string | null;
  workspace_trust: WorkspaceTrust;
  query_file: string;
  config_file: string;
}

export interface CatalogTable {
  database: string;
  schema: string;
  name: string;
  kind: "table" | "view";
  columns: { name: string; type: string }[];
}

export interface StatementRange {
  start: number;
  end: number;
  text: string;
}

export interface Selection {
  row_start: number;
  row_end: number;
  col_start: number;
  col_end: number;
}

export interface HistoryEntry {
  ts: number;
  sql: string;
  origin: string;
}

export const api = {
  snapshot: () => invoke<Snapshot>("get_snapshot"),
  newTab: (sql?: string, run?: boolean) => invoke<string>("new_tab", { sql: sql ?? null, run: run ?? false }),
  closeTab: (id: string) => invoke<void>("close_tab", { id }),
  activateTab: (id: string) => invoke<void>("activate_tab", { id }),
  moveTab: (id: string, to: number) => invoke<void>("move_tab", { id, to }),
  renameTab: (id: string, title: string) => invoke<void>("rename_tab", { id, title }),
  setSql: (id: string, sql: string) => invoke<void>("set_sql", { id, sql }),
  run: (id: string, text?: string | null, force?: boolean) => invoke<number>("run_tab", { id, text: text ?? null, force: force ?? false }),
  rerun: (id: string, force?: boolean) => invoke<number>("rerun_tab", { id, force: force ?? false }),
  stop: (id: string) => invoke<void>("stop_tab", { id }),
  setSort: (id: string, sort: SortSpec[]) => invoke<void>("set_sort", { id, sort }),
  setLimit: (id: string, limit: number | null, force?: boolean) => invoke<void>("set_limit", { id, limit, force: force ?? false }),
  countRows: (id: string) => invoke<void>("count_rows", { id }),
  result: (id: string) => invoke<QueryResult | null>("get_result", { id }),
  stats: (id: string) => invoke<QueryResult | null>("get_stats", { id }),
  setStatsOpen: (id: string, open: boolean) => invoke<void>("set_stats_open", { id, open }),
  histogram: (id: string, column: string, ty: string) => invoke<Histogram>("column_histogram", { id, column, ty }),
  setSelection: (id: string, selection: Selection | null) => invoke<void>("set_selection", { id, selection }),
  catalog: () => invoke<CatalogTable[]>("get_catalog"),
  openSource: (spec: SourceSpec) => invoke<SourceInfo>("open_source", { spec }),
  splitStatements: (sql: string) => invoke<StatementRange[]>("split_statements", { sql }),
  history: (limit?: number) => invoke<HistoryEntry[]>("get_history", { limit: limit ?? null }),
  layout: () => invoke<Record<string, unknown> | null>("get_layout"),
  saveLayout: (layout: Record<string, unknown>) => invoke<void>("save_layout", { layout }),
  workspaceConfig: () => invoke<{ path: string; text: string; hash: string }>("get_workspace_config"),
  trustWorkspaceConfig: (hash: string) => invoke<void>("trust_workspace_config", { hash }),
};

export function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (e) => cb(e.payload));
}
