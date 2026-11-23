import { create } from "zustand";
import {
  api,
  on,
  type ApiStatus,
  type CatalogTable,
  type Config,
  type ConfigError,
  type QueryResult,
  type SourceInfo,
  type TabSummary,
  type WorkspaceTrust,
} from "@/lib/api";

interface Versioned {
  version: number;
  data: QueryResult | null;
}

interface State {
  ready: boolean;
  tabs: Record<string, TabSummary>;
  order: string[];
  active: string;
  config: Config | null;
  configErrors: ConfigError[];
  source: SourceInfo | null;
  sourceError: string | null;
  connections: string[];
  recents: string[];
  apiStatus: ApiStatus | null;
  workspace: string | null;
  workspaceTrust: WorkspaceTrust | null;
  queryFile: string;
  configFile: string;
  results: Record<string, Versioned>;
  stats: Record<string, Versioned>;
  catalog: CatalogTable[] | null;
  catalogError: string | null;
  paletteOpen: boolean;
  detailPinned: boolean;
  /** Column to scroll to and highlight in the main grid, per tab. */
  jump: { tab: string; column: string; nonce: number } | null;
  setPalette: (open: boolean) => void;
  setDetailPinned: (open: boolean) => void;
  jumpTo: (tab: string, column: string) => void;
  refreshCatalog: () => Promise<void>;
}

export const useStore = create<State>((set) => ({
  ready: false,
  tabs: {},
  order: [],
  active: "",
  config: null,
  configErrors: [],
  source: null,
  sourceError: null,
  connections: [],
  recents: [],
  apiStatus: null,
  workspace: null,
  workspaceTrust: null,
  queryFile: "",
  configFile: "",
  results: {},
  stats: {},
  catalog: null,
  catalogError: null,
  paletteOpen: false,
  detailPinned: false,
  jump: null,
  setPalette: (open) => set({ paletteOpen: open }),
  setDetailPinned: (open) => set({ detailPinned: open }),
  jumpTo: (tab, column) => set({ jump: { tab, column, nonce: Date.now() } }),
  refreshCatalog: async () => {
    try {
      const catalog = await api.catalog();
      set({ catalog, catalogError: null });
    } catch (e) {
      set({ catalogError: String(e) });
    }
  },
}));

export const activeTab = () => {
  const s = useStore.getState();
  return s.tabs[s.active];
};

const inflight = new Map<string, number>();

/** Fetch the result or stats for a tab if the local copy is stale. */
async function sync(kind: "results" | "stats", t: TabSummary) {
  const version = kind === "results" ? t.result_version : t.stats_version;
  const have = useStore.getState()[kind][t.id];
  if (have && have.version === version) return;
  const key = `${kind}:${t.id}`;
  if (inflight.get(key) === version) return;
  inflight.set(key, version);
  try {
    const data = kind === "results" ? await api.result(t.id) : await api.stats(t.id);
    const cur = useStore.getState().tabs[t.id];
    const curVersion = cur ? (kind === "results" ? cur.result_version : cur.stats_version) : -1;
    if (curVersion !== version) return; // a newer one is coming
    useStore.setState((s) => ({ [kind]: { ...s[kind], [t.id]: { version, data } } }) as Partial<State>);
  } finally {
    if (inflight.get(key) === version) inflight.delete(key);
  }
}

function syncActive() {
  const t = activeTab();
  if (!t) return;
  void sync("results", t);
  if (t.stats_open) void sync("stats", t);
}

function applyTab(t: TabSummary) {
  const prev = useStore.getState().tabs[t.id];
  useStore.setState((s) => ({ tabs: { ...s.tabs, [t.id]: t } }));
  if (t.id === useStore.getState().active) {
    syncActive();
    // DDL may change the catalog.
    if (prev && prev.status === "running" && t.status !== "running" && !t.wrapped) {
      void useStore.getState().refreshCatalog();
    }
  }
}

export async function initStore() {
  const snap = await api.snapshot();
  useStore.setState({
    ready: true,
    tabs: Object.fromEntries(snap.tabs.map((t) => [t.id, t])),
    order: snap.tabs.map((t) => t.id),
    active: snap.active,
    config: snap.config,
    configErrors: snap.config_errors,
    source: snap.source,
    sourceError: snap.source_error,
    connections: snap.connections,
    recents: snap.recents,
    apiStatus: snap.api,
    workspace: snap.workspace,
    workspaceTrust: snap.workspace_trust,
    queryFile: snap.query_file,
    configFile: snap.config_file,
  });
  void useStore.getState().refreshCatalog();

  await on<TabSummary>("tab", applyTab);
  await on<{ tabs: TabSummary[]; active: string }>("tabs", (p) => {
    const ids = new Set(p.tabs.map((t) => t.id));
    useStore.setState((s) => ({
      tabs: Object.fromEntries(p.tabs.map((t) => [t.id, t])),
      order: p.tabs.map((t) => t.id),
      active: p.active,
      results: Object.fromEntries(Object.entries(s.results).filter(([k]) => ids.has(k))),
      stats: Object.fromEntries(Object.entries(s.stats).filter(([k]) => ids.has(k))),
    }));
    syncActive();
  });
  await on<{ config: Config; connections: string[] }>("config", (p) =>
    useStore.setState({ config: p.config, connections: p.connections }),
  );
  await on<{ api: ApiStatus; config_errors: ConfigError[]; source_error: string | null; workspace_trust: WorkspaceTrust }>("status", (p) =>
    useStore.setState({ apiStatus: p.api, configErrors: p.config_errors, sourceError: p.source_error, workspaceTrust: p.workspace_trust }),
  );
  await on<{ source: SourceInfo; recents: string[] }>("source", (p) => {
    useStore.setState({ source: p.source, recents: p.recents, catalog: null });
    void useStore.getState().refreshCatalog();
  });
  syncActive();
}
