import { useEffect, useRef } from "react";
import { Annotation, Compartment, EditorState, Prec, type Extension } from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  placeholder,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { bracketMatching, indentUnit } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { PostgreSQL, SQLDialect, sql, type SQLNamespace } from "@codemirror/lang-sql";
import { vim } from "@replit/codemirror-vim";
import { api, type CatalogTable, type Config } from "@/lib/api";
import { baseTheme, highlight } from "@/lib/cm";
import { useStore } from "@/store";

const DuckDB = SQLDialect.define({
  ...PostgreSQL.spec,
  keywords:
    (PostgreSQL.spec.keywords ?? "") +
    " summarize describe pivot unpivot qualify asof positional exclude replace columns attach detach install load pragma copy export import macro struct map union list",
  builtin:
    (PostgreSQL.spec.builtin ?? "") +
    " read_csv read_csv_auto read_parquet read_json read_json_auto read_ndjson glob unnest list_value struct_pack to_json json_extract json_extract_string strftime strptime date_trunc epoch_ms regexp_matches regexp_extract string_agg list_agg approx_count_distinct",
});

/** Marks changes that come from the backend (agent, file) so they are not echoed. */
const External = Annotation.define<boolean>();

const states = new Map<string, EditorState>();
const appliedRev = new Map<string, number>();
let view: EditorView | null = null;
let currentTab = "";

const keysC = new Compartment();
const vimC = new Compartment();
const indentC = new Compartment();
const sqlC = new Compartment();

/** Used by the sidebar, palette and history to edit the active editor. */
export const editorBridge = {
  insertAtCursor(text: string) {
    if (!view) return;
    const { from, to } = view.state.selection.main;
    view.dispatch({ changes: { from, to, insert: text }, selection: { anchor: from + text.length }, scrollIntoView: true });
    view.focus();
  },
  replaceAll(text: string) {
    if (!view) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text }, selection: { anchor: text.length } });
    view.focus();
  },
  isEmpty() {
    return !view || view.state.doc.toString().trim() === "";
  },
  focus() {
    view?.focus();
  },
};

async function runFromEditor(v: EditorView, statementOnly: boolean) {
  const tab = currentTab;
  const sel = v.state.selection.main;
  // Make sure the backend has the current text before it runs.
  if (syncTimer) window.clearTimeout(syncTimer);
  syncTimer = null;
  pending = null;
  await api.setSql(tab, v.state.doc.toString());
  if (!statementOnly) {
    const text = sel.empty ? null : v.state.sliceDoc(sel.from, sel.to);
    await api.run(tab, text);
    return;
  }
  const doc = v.state.doc.toString();
  const ranges = await api.splitStatements(doc);
  if (!ranges.length) return;
  const pos = sel.head;
  let pick = ranges.find((r) => pos >= r.start && pos <= r.end + 1);
  pick ??= [...ranges].reverse().find((r) => r.end <= pos) ?? ranges[0];
  await api.run(tab, pick.text);
}

function runKeys(cfg: Config | null): Extension {
  const kb = cfg?.keybindings ?? { run: "Mod-Enter", run_statement: "Shift-Mod-Enter" };
  return Prec.highest(
    keymap.of([
      { key: kb.run, run: (v) => (void runFromEditor(v, false), true) },
      { key: kb.run_statement, run: (v) => (void runFromEditor(v, true), true) },
      {
        key: "Escape",
        run: () => {
          const t = useStore.getState().tabs[currentTab];
          if (t?.status === "running") {
            void api.stop(currentTab);
            return true;
          }
          return false;
        },
      },
    ]),
  );
}

function schemaFor(catalog: CatalogTable[] | null): SQLNamespace {
  const ns: Record<string, SQLNamespace> = {};
  for (const t of catalog ?? []) {
    const cols = t.columns.map((c) => c.name);
    ns[t.name] ??= cols;
    const schema = (ns[t.schema] ??= {}) as Record<string, SQLNamespace>;
    if (typeof schema === "object" && !Array.isArray(schema)) schema[t.name] = cols;
  }
  return ns;
}

function makeState(doc: string): EditorState {
  const s = useStore.getState();
  return EditorState.create({
    doc,
    extensions: [
      keysC.of(runKeys(s.config)),
      vimC.of(s.config?.editor.vim ? vim() : []),
      indentC.of(tabSize(s.config?.editor.tab_size ?? 2)),
      sqlC.of(sql({ dialect: DuckDB, schema: schemaFor(s.catalog), upperCaseKeywords: false })),
      lineNumbers(),
      highlightActiveLine(),
      highlightActiveLineGutter(),
      highlightSelectionMatches(),
      drawSelection(),
      history(),
      bracketMatching(),
      closeBrackets(),
      autocompletion({ icons: false }),
      placeholder("FROM 'data.parquet'   (Cmd+Enter runs, Shift+Cmd+Enter runs the statement under the cursor, Cmd+K opens commands)"),
      keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...completionKeymap, indentWithTab]),
      baseTheme,
      highlight,
      EditorView.updateListener.of((u) => {
        if (!u.docChanged) return;
        if (u.transactions.some((tr) => tr.annotation(External))) return;
        scheduleSync(currentTab, u.state.doc.toString());
      }),
    ],
  });
}

function tabSize(n: number): Extension {
  return [EditorState.tabSize.of(n), indentUnit.of(" ".repeat(n))];
}

let syncTimer: number | null = null;
let pending: { tab: string; text: string } | null = null;
function flushSync() {
  if (syncTimer) window.clearTimeout(syncTimer);
  syncTimer = null;
  if (pending) {
    void api.setSql(pending.tab, pending.text);
    pending = null;
  }
}
function scheduleSync(tab: string, text: string) {
  if (pending && pending.tab !== tab) flushSync();
  pending = { tab, text };
  if (syncTimer) window.clearTimeout(syncTimer);
  syncTimer = window.setTimeout(flushSync, 150);
}

/** Apply an external text change to a tab's editor state, keeping undo history. */
function applyExternal(id: string, text: string) {
  if (id === currentTab && view) {
    if (view.state.doc.toString() !== text) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text }, annotations: External.of(true) });
    }
    return;
  }
  const st = states.get(id);
  if (st && st.doc.toString() !== text) {
    states.set(id, st.update({ changes: { from: 0, to: st.doc.length, insert: text }, annotations: External.of(true) }).state);
  }
}

export function SqlEditor() {
  const host = useRef<HTMLDivElement>(null);
  const active = useStore((s) => s.active);
  const config = useStore((s) => s.config);
  const catalog = useStore((s) => s.catalog);

  useEffect(() => {
    if (!host.current) return;
    const s = useStore.getState();
    currentTab = s.active;
    const t = s.tabs[s.active];
    const st = states.get(s.active) ?? makeState(t?.sql ?? "");
    if (t) appliedRev.set(t.id, t.sql_rev);
    view = new EditorView({ parent: host.current, state: st });
    view.focus();
    const unsub = useStore.subscribe((s2, prev) => {
      if (s2.tabs === prev.tabs) return;
      for (const t2 of Object.values(s2.tabs)) {
        const seen = appliedRev.get(t2.id) ?? -1;
        if (t2.sql_rev <= seen) continue;
        appliedRev.set(t2.id, t2.sql_rev);
        if (t2.sql_origin !== "user") applyExternal(t2.id, t2.sql);
      }
      for (const id of [...states.keys()]) if (!s2.tabs[id]) states.delete(id);
    });
    return () => {
      unsub();
      flushSync();
      if (view) states.set(currentTab, view.state);
      view?.destroy();
      view = null;
    };
  }, []);

  // switch tabs: keep each tab's editor state (and undo history)
  useEffect(() => {
    if (!view || active === currentTab) return;
    flushSync();
    states.set(currentTab, view.state);
    currentTab = active;
    const t = useStore.getState().tabs[active];
    let st = states.get(active);
    if (!st) st = makeState(t?.sql ?? "");
    else if (t && st.doc.toString() !== t.sql) {
      st = st.update({ changes: { from: 0, to: st.doc.length, insert: t.sql }, annotations: External.of(true) }).state;
    }
    if (t) appliedRev.set(t.id, t.sql_rev);
    view.setState(st);
    // re-apply live config to a state created earlier
    view.dispatch({
      effects: [
        keysC.reconfigure(runKeys(config)),
        vimC.reconfigure(config?.editor.vim ? vim() : []),
        indentC.reconfigure(tabSize(config?.editor.tab_size ?? 2)),
        sqlC.reconfigure(sql({ dialect: DuckDB, schema: schemaFor(useStore.getState().catalog), upperCaseKeywords: false })),
      ],
    });
    view.focus();
  }, [active, config]);

  useEffect(() => {
    view?.dispatch({
      effects: [
        keysC.reconfigure(runKeys(config)),
        vimC.reconfigure(config?.editor.vim ? vim() : []),
        indentC.reconfigure(tabSize(config?.editor.tab_size ?? 2)),
      ],
    });
  }, [config]);

  useEffect(() => {
    view?.dispatch({ effects: sqlC.reconfigure(sql({ dialect: DuckDB, schema: schemaFor(catalog), upperCaseKeywords: false })) });
  }, [catalog]);

  return <div ref={host} className="h-full min-h-0 overflow-hidden" />;
}
