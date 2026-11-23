# Gander design

Gander is a desktop data viewer for DuckDB. A user types SQL, runs it, and browses the result in a dense grid, much like the DuckDB web UI. Two design choices shape everything else:

1. Configuration is agent first. Every setting lives in plain files that an agent can read and write, and the app reloads them when they change.
2. A local HTTP API lets an agent drive the UI. The agent can read the current query and result, replace the query, and run it. A Python CLI wrapped in a Claude skill is the supported client.

The app is a Tauri v2 shell with a React frontend in `src/` and a Rust backend in `src-tauri/`. The Rust side owns all state: the DuckDB connections, the tabs, the config, and the API. The frontend renders that state and sends user actions back through Tauri commands.

## Scope

- Single user on their own machine. There are no accounts, sharing, or multi-user concerns; the API key only guards against other local processes.
- macOS is the supported platform. Builds so far are Apple Silicon only; Intel and universal builds are untested. Ubuntu (24.04 LTS, WebKitGTK) is the next target and has not been built yet. Windows is out of scope.
- The API is a viewport onto what the user is looking at, rather than a query engine for the agent. The agent sees the active tab and can edit and run that tab's query, and everything it does is visible to the user. The user decides which data source is open; the agent cannot switch sources, open DuckDB files, list other configured sources, read other tabs, or run queries in the background.

## Stack

| Layer | Choice | Notes |
|---|---|---|
| Shell | Tauri v2 | Rust edition 2024 |
| Database | `duckdb` crate with the `bundled`, `json` and `parquet` features | No system DuckDB required |
| HTTP API | `axum` + `tokio` | Runs inside the Tauri process |
| File watching | `notify` + `notify-debouncer-full` | 150 ms debounce |
| Config parsing | `toml`, `serde`, `schemars` | `schemars` emits a JSON Schema for agents |
| Frontend | Vite, React, TypeScript | |
| Frontend state | `zustand` | Mirrors the Rust state pushed over Tauri events |
| Components | shadcn/ui on Tailwind v4 | Restyled for density (see below) |
| Editor | CodeMirror 6 with `@codemirror/lang-sql` | Schema-aware autocomplete |
| Grid | TanStack Virtual | Row and column virtualization; the column model is small enough not to need TanStack Table |
| JSON view | CodeMirror 6 with `@codemirror/lang-json` | Read-only, for pretty-printed cells |
| Fuzzy filter | `fzf` (npm, fzf-for-js) in the UI, `nucleo-matcher` (fzf scoring) for `GET /catalog?q=` | Schema browser filter |
| Native dialogs | `tauri-plugin-dialog` | Open DuckDB file |
| Skill CLI | Python 3.10+, standard library only | No `pip install` step |

## User interface

### Density

The UI is intentionally compact. Some people will find it crowded; that is the accepted cost of seeing more data per screen. The compact sizes are:

- Base font size 12px, grid and editor 12px monospace, secondary text 11px.
- Grid row height 20px, header row 22px, cell padding 4px horizontal and 0 vertical.
- Buttons and inputs 22-24px tall (shadcn `size="sm"` reduced further via a `xs` variant).
- Panel padding 4-6px. No card shadows; panels are separated by 1px borders.
- Toolbar icons 14px (lucide), icon-only buttons with tooltips.
- No empty hero states or decorative illustrations. Empty states are a single line of muted text.

Density is adjustable in config (`ui.density = "compact" | "cozy"`), but `compact` is the default and the design target.

### Design language

The UI uses shadcn tokens (`--background`, `--foreground`, `--muted`, `--border`, `--primary`, `--ring`, and so on) and the neutral base palette. Light and dark themes follow the system setting unless `ui.theme` overrides it. Radii stay small (`--radius: 0.25rem`). Components come from shadcn: `Resizable`, `Tooltip`, `ContextMenu`, `DropdownMenu`, `Popover`, `Dialog`, `Command` (palette), `ScrollArea`, `Badge`, `Button` and `Input`. There are no toasts; feedback appears inline or in the status bar.

### Layout

```
+--------------------------------------------------------------------------+
| [db: analytics.duckdb v] [Run Cmd+Enter] [Stop] [agent: connected]  ... |
+-------------+------------------------------------------------------------+
| [ord am    ]| [orders x] [users by day x] [scratch* x] [+]               |
|  main       | SQL editor (CodeMirror)                                    |
|   *ord*ers  |                                                            |
|    *am*ount +------------------------------------------------------------+
|   user_*ord*| Result grid                                                |
|             |  # | id | meta              | amount   | ts                  |
|             |  1 | 17 | {"a":1,"b":[2,... | 12.40 [c]| 2026-11-02 09:14:00 |
|             |  2 | 18 | NULL              |  9.10    | 2026-11-02 09:15:12 |
|-------------|  3 | 19 | {"a":2}           |  4.75    | 2026-11-02 09:16:40 |
|v Col stats  |                                                            |
| amount DBL  |                                                            |
|  min   0.5  |                                                            |
|  max   812  |                                                            |
|  nulls 0.2% | Truncated at 1,000 rows. ...                [Count all rows] |
+-------------+------------------------------------------------------------+
| 1,000+ rows (limit)  0.042 s  sort: amount desc | Cmd+K commands | ... |
+--------------------------------------------------------------------------+
```

All three splits are resizable and the sizes are persisted to the state file.

### Features

Tabs:
- Each tab has its own query text, result, sort, row limit, scroll position, stats panel open state, and cached stats. The connection is shared, so all tabs see the same databases.
- Compact tab strip above the editor, 22px tall. `Cmd+T` new tab, `Cmd+W` close, `Cmd+1`-`Cmd+9` jump, `Cmd+Shift+[` and `Cmd+Shift+]` cycle, double click to rename. A dot marks a running query; `*` marks text changed since the last run.
- Tab titles default to the first table name in the query and can be renamed.
- Tabs, their text, and their titles are restored on restart. Results are not persisted; a restored tab shows its last row count and a "Run" hint.
- Queries in different tabs can run at the same time. Each tab runs at most one query at once.
- The agent only ever sees the active tab (see "HTTP API").

Query editing:
- SQL syntax highlighting and autocomplete for tables, columns, and DuckDB functions.
- `Cmd+Enter` runs the whole editor, or the selection if one exists. `Shift+Cmd+Enter` runs the statement under the cursor.
- Multiple statements are allowed; the grid shows the result of the last one.
- `Esc` or the Stop button interrupts a running query (`Connection::interrupt`).
- Query history is kept in `history.jsonl` in the state directory and is browsable from the command palette. On startup it is compacted to the newest 5000 entries once it passes 6250.

Result grid:
- Results are capped at the row limit (default 1000, see "Row limit and sorting"). The whole capped result is sent to the frontend; rows and columns are still virtualized for rendering.
- Column headers show name and DuckDB type. Click to sort, right click to copy the column name, sort, hide or show columns. Drag a header edge to resize; double click the edge to reset.
- Numbers right aligned, NULL shown as a muted `NULL`.
- Copy selection as TSV, CSV, Markdown, or JSON.

JSON cells:
- A cell counts as JSON when its DuckDB type is `JSON`, `STRUCT`, `LIST`, `MAP`, or `UNION`, or when it is a `VARCHAR` whose trimmed value starts with `{` or `[` and parses as JSON. Detection for `VARCHAR` runs on the frontend on the values already loaded, so it costs no extra query.
- In the grid, JSON cells show the compact single-line form with muted syntax colours, cut at `ui.max_cell_chars`.
- Hovering for 400 ms, or pressing `Space` on a focused cell, opens a popover with the value pretty-printed (2-space indent, keys in original order) in a read-only CodeMirror view with `@codemirror/lang-json`: syntax highlighting, folding, and its own copy button. `Enter` pins it into a resizable cell detail panel on the right that follows the focused cell.
- Values over 1 MB are shown unformatted with a "Format anyway" action, so a huge blob cannot freeze the UI.
- Invalid JSON in a `VARCHAR` falls back to plain text with no error.

Copy buttons:
- Every cell has a copy button and every row has a copy button. To keep the grid dense they render only on hover or keyboard focus: the cell button is a 12px icon overlaid at the cell's right edge, and the row button replaces the row number in the gutter.
- The cell button copies the raw value (unformatted JSON for JSON cells, `NULL` copies an empty string). `Alt` click copies JSON cells pretty-printed.
- The row button copies the row as JSON by default; `ui.copy_row_format` switches it to `tsv`, `csv`, or `markdown`. A right click on the button offers all four.
- After copying, the icon changes to a check mark for one second. No toast.
- Keyboard: `Cmd+C` copies the selection, or the focused cell when nothing is selected; `Shift+Cmd+C` copies the focused row.

Column stats panel:
- A collapsible panel at the bottom of the left sidebar, below the schema browser, collapsed by default to a single 22px header row: `> Column stats (SUMMARIZE)`. Click the header or press `Cmd+I` to toggle.
- Nothing runs while it is collapsed. Opening it runs `SUMMARIZE SELECT * FROM (<query>)` against the tab's full unwrapped query, without the row limit or sort, so the stats describe all rows even when the grid is truncated.
- The SUMMARIZE output is transposed to fit the narrow sidebar: one group per result column, headed by the column name and type, followed by one row per statistic (`min`, `max`, `approx_unique`, `avg`, `std`, `q25`, `q50`, `q75`, `count`, `null_percentage`). Statistics that are NULL for the column, such as `avg` for text, are left out. Values are right aligned and `null_percentage` gets a thin inline bar. Group headers stay pinned while scrolling through a group.
- Clicking a group header scrolls the result grid to that column and highlights it.
- Results are cached per tab, keyed by the executed query text and the active source. Re-opening the panel without re-running the query shows the cache instantly.
- When the tab's query runs again: if the panel is open, SUMMARIZE re-runs after the main query finishes; if collapsed, the cache is dropped and nothing runs until it is opened.
- SUMMARIZE runs on its own cloned connection so it never blocks the tab's main query. It shows a spinner in the header with a stop button, and it is cancelled when the panel is collapsed, the tab is closed, or the main query re-runs.
- For statements that are not wrapped (see "Row limit and sorting"), the panel shows `No stats for this statement` and runs nothing.
- Open or collapsed state is kept per tab; new tabs start collapsed. The panel height is resizable and persisted.

Data sources:
- The toolbar dropdown picks the source: a configured connection, a recently opened `.duckdb` file, "Open DuckDB file..." (native dialog via `tauri-plugin-dialog`), or "No database (files only)".
- Files-only mode runs on an in-memory DuckDB. The user can still query files directly, for example `FROM 'events.parquet'`, `FROM 'logs.jsonl'`, `FROM read_csv('data/*.csv')`. DuckDB's replacement scans handle `.parquet`, `.csv`, `.tsv`, `.json`, `.jsonl`, and `.ndjson` paths, including globs. Relative paths resolve against the workspace directory (the process working directory is set to it).
- Files-only is the default when no `default_connection` is configured.
- Files can also be queried while a `.duckdb` file is open; the two modes differ only in whether there is a catalog to browse.
- Ad hoc opened files are remembered in the state file as recents. They are never written into `config.toml`.
- Dragging a `.duckdb` file onto the window opens it. Dragging a `.parquet`, `.csv`, `.json`, or `.jsonl` file inserts `FROM '<path>'` into a new tab and runs it.

Schema browser (when a DuckDB file is open):
- Tree of databases, schemas, tables, views, and columns with types.
- A filter box sits at the top of the tree with fzf-style fuzzy matching, using the `fzf` npm package (a port of fzf's matching algorithm). It matches against `schema.table` and `table.column`, so `ord am` finds `orders.amount`. Space-separated terms must all match, matched characters are highlighted, and results are ranked by fzf's score with exact and prefix matches first.
- `Cmd+P` focuses the filter from anywhere. Arrow keys move through matches, `Enter` on a table inserts `FROM <table>` into the editor and runs it, `Enter` on a column inserts the column name at the cursor, `Esc` clears the filter.
- Double click a table to run `FROM <table>`: in the current tab when its editor is empty, otherwise in a new tab, so the user's query is never overwritten. The row limit applies, so no `LIMIT` is needed in the text.
- In files-only mode the sidebar shows a short hint with example `FROM 'file.parquet'` queries instead of the tree, plus any tables or views the user has created in the in-memory database.

Status bar:
- A `Cmd+K commands` hint that opens the command palette (the toolbar button carries the same label).
- Row count (with `+` and "limit" when truncated), elapsed time, active sort, config status (ok or the parse error), API status, and the source of the last change to the query (`user` or `agent`).

### Row limit and sorting

Every query result is capped at `query.limit` rows, default 1000. The limit and the sort are applied by wrapping the user's query; the text in the editor is never modified.

With no sort active, Gander runs:

```sql
SELECT * FROM (<query>) AS a LIMIT 1001
```

Clicking a column header sets the sort and re-runs the wrapped query:

```sql
SELECT * FROM (<query>) AS a ORDER BY a."amount" DESC LIMIT 1001
```

Details:

- The wrapper asks for `limit + 1` rows. If the extra row comes back it is dropped and the result is marked truncated.
- Truncation is always visible, with no confirmation dialogs or toasts:
  - The status bar row count reads `1,000+ rows (limit)` in the shadcn warning color instead of the muted text color.
  - A one-line bar pinned below the last grid row reads `Truncated at 1,000 rows. Sorting and stats apply to the full query.` with the "Count all rows" action next to it.
  - The tab title gets a small `+` suffix while its result is truncated.
- Sorting a truncated result re-runs the full query and shows whichever rows come first under the new order. This happens without a prompt; the truncation markers above are what tell the user the grid is a slice.
- Clicking a header cycles ascending, descending, off. Shift click adds a secondary sort column (`ORDER BY a."x" ASC, a."y" DESC`). The sort indicator and position appear in the header.
- Column names are double-quoted with embedded quotes escaped, so names with spaces or mixed case work.
- Trailing semicolons and whitespace are stripped from the query before wrapping. Any `LIMIT` or `ORDER BY` the user wrote stays inside the subquery and still applies; the outer limit only caps it further.
- Only the last statement is wrapped, and only when it is a query (`SELECT`, `WITH`, `FROM`, `VALUES`, `TABLE`, `SUMMARIZE`, `DESCRIBE`, `PIVOT`, `UNPIVOT`, `SHOW`, or a parenthesised query). Gander uses DuckDB's parser (`json_serialize_sql`, which only accepts SELECT statements) to decide this. Other statements (`CREATE`, `INSERT`, `SET`, `ATTACH`, `PRAGMA`) run unwrapped and show their returned rows, if any, without sorting.
- Sort and limit belong to the tab. The sort is kept while the user edits and re-runs the tab's query, and cleared when a column it refers to no longer exists in the new result.
- A "Count all rows" action in the status bar runs `SELECT count(*) FROM (<query>)` on demand when the result is truncated.
- The limit can be changed per session from a small input in the toolbar, or via the API. `query.limit = 0` disables the limit. An unlimited query first counts its rows, and if there are more than one million it stops with a `too_many_rows` error. The UI then offers to run it anyway, and the API and CLI accept a `force` flag.

The wrapped SQL that actually ran is shown in a tooltip on the status bar row count and returned by the API as `executed_sql`.

## Configuration

### Locations

| Path | Purpose | Written by |
|---|---|---|
| `~/.config/gander/config.toml` | Global settings | User or agent |
| `<workspace>/.gander/config.toml` | Workspace overrides, merged over global (see "Workspace trust") | User or agent |
| `<workspace>/.gander/query.sql` | Query text of the active tab | App and agent |
| `~/.local/state/gander/state.json` | Window layout, recents, last workspace | App only |
| `~/.local/state/gander/history.jsonl` | Query history | App only |
| `~/.local/state/gander/workspaces/<hash>/tabs.json` | All tabs of a workspace: ids, titles, text, sort, limit | App only |
| `~/.local/state/gander/server.json` | Running API port, pid, and the per-launch API key when no key is configured | App only |
| `~/.local/state/gander/trusted_workspaces.json` | SHA-256 of each workspace config the user approved | App only |
| `~/.config/gander/schema.json` | JSON Schema for `config.toml` | App, on start |

XDG style paths are used on macOS as well as Ubuntu, so an agent can find them without platform logic. `GANDER_CONFIG_DIR` and `GANDER_STATE_DIR` override them.

The workspace is the directory Gander was opened with (`gander <dir>` or the last one used). When no workspace is set, `query.sql` lives in `~/.config/gander/`.

On startup Gander removes a workspace's `tabs.json` folder when the workspace path has been missing for 30 days. Folders written before the path was recorded are removed after 90 days untouched.

### Example

```toml
default_connection = "analytics"   # omit to start in files-only mode; must come before any [[connections]] table

[server]
enabled = true
port = 7420                 # 0 picks a free port; the real one goes in server.json
api_key_env = "GANDER_API_KEY"   # optional; key read from this env var
# api_key = "..."           # optional; literal key, takes precedence over api_key_env
                            # with neither, a random key is made each launch and written to server.json
default_rows = 50           # rows returned per API response when the request omits `rows`
max_rows = 10000            # upper bound a request can ask for

[ui]
theme = "system"            # system | light | dark
density = "compact"         # compact | cozy
font_size = 12
max_cell_chars = 200
copy_row_format = "json"  # json | tsv | csv | markdown

[editor]
vim = false
tab_size = 2

[query]
limit = 1000                # max rows returned, applied by wrapping the query; 0 disables
timeout_ms = 0              # 0 means no timeout

[[connections]]
name = "analytics"
path = "~/data/analytics.duckdb"   # or ":memory:"
read_only = true
extensions = ["httpfs", "spatial"]
init_sql = ["SET threads = 4"]
attach = [{ alias = "raw", path = "~/data/raw.duckdb", read_only = true }]

[keybindings]
run = "Mod-Enter"
run_statement = "Shift-Mod-Enter"
palette = "Mod-k"
```

### Workspace trust

A workspace config comes with the folder, which may be a cloned repository or a download. Opening a connection runs its `init_sql`, extensions and attaches, and DuckDB SQL can read and write any file the user can, so a workspace config is not allowed to do that on its own.

- `[server]` in a workspace config is always ignored. Server settings, including the API key, come only from the global config.
- `[[connections]]` in a workspace config apply only after the user approves that file in the app. A banner offers a review dialog that shows the file; approving it stores the SHA-256 of the contents that were shown. If the file changed in between, the approval is refused and the dialog shows the new contents.
- Any later edit to the file makes it untrusted again until it is reviewed. Connections it defined are dropped and an open one falls back to files-only.
- Until then, a `default_connection` in the workspace file that names one of its own connections is ignored. One that names a global connection still applies.
- Approval is only possible from the app window. The HTTP API has no way to approve, so an agent cannot approve its own edits. Global config connections need no approval.

### Reload behaviour

The Rust side watches every config file and `query.sql`. On a change it parses, validates against the schema, and diffs against the current config.

| Change | Effect |
|---|---|
| `ui.*`, `editor.*`, `keybindings.*` | Applied live |
| `query.*` | Applied to the next query; a changed `query.limit` re-runs the current query if it was truncated |
| `connections` added or removed | Applied live; the source list refreshes. Removing the open connection falls back to files-only |
| Any field of the open connection | The source reopens with the new settings |
| `server.api_key*`, `server.default_rows`, `server.max_rows` | Applied to the next request |
| `server.port`, `server.enabled` | Server restarts on the new port; `server.json` updated |
| `query.sql` changed on disk | Active tab's editor content replaced (see conflicts below) |

If a file fails to parse or validate, the last good config stays active, the status bar shows the error with file and line, and `GET /state` reports it. The app never rewrites `config.toml` itself, so comments and formatting written by a person or agent survive.

`query.sql` always mirrors the active tab. When the user switches tabs, the app rewrites `query.sql` with the new tab's text. An external write to the file goes to whichever tab is active at that moment. Other tabs have no file an agent can read; their text lives only in the app-owned `tabs.json`.

Conflicts on `query.sql`: if the active tab has unsaved user edits when the file changes, the incoming version wins and the previous text is pushed onto that tab's history, so nothing is lost. The app writes the active tab's text to `query.sql` with a 500 ms debounce and ignores the watcher event caused by its own write (tracked by content hash).

## HTTP API

### Binding and security

- Binds to `127.0.0.1` and `[::1]` only. There is no setting to bind elsewhere.
- Rejects requests whose `Host` header is not `localhost`, `127.0.0.1`, or `[::1]` with the configured port. This blocks DNS rebinding from a browser page.
- Rejects requests that carry an `Origin` header (browsers send one; the CLI does not). No CORS headers are ever sent.
- Every request except `GET /health` must send `Authorization: Bearer <key>`. Comparison is constant time. The key is `server.api_key` or `server.api_key_env` from the global config; with neither, Gander makes a random 256 bit key on each launch.
- `server.json` is written with mode `0600` and contains the port and pid. It also holds the per-launch key, so local clients run by the same user can find it. A key from the config is never written there.

### Tab scoping

Every endpoint acts on the active tab, the one the user is looking at. There is no tab parameter, no endpoint that lists tabs, and no way to switch tabs or read another tab's query or result. `GET /state` returns the active tab's id and title so the agent can tell when the user has switched; a `tab_changed` event signals the same.

A run started through the API is bound to the tab that was active when it started. If the user switches tabs while it runs, it finishes in its original tab, and `POST /run` still returns its summary. Later calls act on the new active tab.

### Rows returned

Two separate limits apply, and the API keeps their names apart:

- `limit` is the query row limit from "Row limit and sorting". It decides how many rows the grid holds, and changing it re-runs the query.
- `rows` is how many of those rows one API response returns. It never re-runs anything and never changes what the user sees.

Every endpoint that returns result rows (`POST /run`, `GET /result`, `GET /selection`) accepts `rows`. When omitted it falls back to `server.default_rows` (default 50). Values above `server.max_rows` (default 10000) are clamped, and `rows = 0` returns the summary with no rows. Responses report `returned_rows`, `offset`, `result_rows`, and `truncated` so the agent can tell how much it has seen and whether more exists.

### Endpoints

All bodies are JSON. Errors return `{ "error": { "code": "...", "message": "..." } }` with a 4xx or 5xx status.

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | `{ "ok": true, "version": "..." }` |
| GET | `/state` | Active tab id and title, the active source (name and path, or files-only), query text, executed SQL, limit, sort, run status, row count, truncated flag, config errors, last change source |
| GET | `/query` | Current editor text |
| PUT | `/query` | Replace editor text. Body `{ "sql": "...", "run": false }` |
| POST | `/run` | Run the current editor text, or `{ "sql": "..." }` to set and run in one call. Optional `limit` sets the tab's query row limit. Optional `rows` sets how many rows to include in the response. Waits up to `wait_ms` (default 30000) and returns the result summary with the first `rows` rows |
| PUT | `/sort` | Set the sort and re-run, same as clicking headers. Body `{ "sort": [{ "column": "amount", "dir": "desc" }] }`; an empty list clears it |
| PUT | `/limit` | Set the session row limit and re-run. Body `{ "limit": 5000 }` |
| POST | `/stop` | Interrupt the running query |
| GET | `/result` | Rows of the current (already limited and sorted) result. Query params `offset`, `rows` (default `server.default_rows`), `format` = `json` \| `csv` \| `markdown`, `columns` |
| GET | `/result/schema` | Column names, DuckDB types, row count |
| GET | `/selection` | Cells or rows the user has selected in the grid. Accepts `rows` and `format` |
| GET | `/stats` | SUMMARIZE output for the active tab's query. Opens the stats panel in the UI if collapsed (so the run stays visible), waits for the result, and returns it. Served from the tab's cache when available |
| GET | `/catalog` | Databases, schemas, tables, views, columns. Optional `q` applies the same fzf-style matching as the sidebar filter |
| GET | `/events` | Server sent events: `tab_changed`, `source_changed`, `query_changed`, `run_started`, `run_finished`, `stats_finished`, `config_reloaded`, `config_error` |

A `PUT /query` or `POST /run` from the API updates the editor visibly, marks the status bar with `last: agent`, and adds a history entry tagged `agent`. The user can undo it with `Cmd+Z` like any other edit.

`GET /result` returns what the user currently sees: the active tab's last result, regardless of who triggered it. There is no background execution: the agent cannot run SQL anywhere except the active tab's editor, and every run replaces what the user sees in that tab.

The data source is chosen only in the UI. `GET /state` and `GET /catalog` describe whatever source the user has open, and there are no endpoints to list or switch sources. If the user changes source, the agent sees it through `/state` and a `source_changed` event.

Read only enforcement belongs to the connection (`read_only = true`). The API does not filter SQL.

### Implementation

- The `axum` server runs on Tauri's async runtime and talks to app state through an `Arc<Core>` holding the DuckDB database, the tab list with per-tab query, result, sort, and limit, the active tab id, and a `tokio::sync::broadcast` channel for events. API handlers only ever look up the active tab.
- Each tab gets its own DuckDB connection (`Connection::try_clone`) so tabs can run concurrently and be interrupted independently.
- UI changes reach the webview through Tauri events (`app.emit("query:changed", ...)`). Frontend changes reach Rust through Tauri commands, so the API and the UI share one source of truth in Rust.
- The query wrapper (limit and sort) lives in one Rust function shared by the UI and the API, with unit tests for quoting, semicolon stripping, multi-statement input, and non-SELECT statements.
- Results are stored as Arrow record batches in memory. Because of the row limit they stay small, so the grid receives the full result in one transfer and the API slices from the same batches.
- DuckDB calls run on `spawn_blocking` so the API stays responsive while a query runs.

## Agent skill

The skill lives in `skill/gander/` and is installed by copying or symlinking it into `~/.claude/skills/gander/`.

```
skill/gander/
  SKILL.md
  scripts/gander.py
  tests/test_gander.py
```

### CLI

`gander.py` uses only the Python standard library (`urllib`, `json`, `argparse`). It finds the server by reading `server.json`, falling back to `GANDER_URL`. It sends the key from `GANDER_API_KEY` when set, otherwise the per-launch key from `server.json`.

```
gander.py status                       # active source, query, row count, errors
gander.py query                        # print current SQL
gander.py query set "SELECT ..."       # replace editor text
gander.py query set -f file.sql --run  # replace from file and run
gander.py run ["SELECT ..."] [--limit 5000] [--rows 20]  # run, print summary and first rows
gander.py sort amount:desc [ts:asc]    # set sort and re-run; `sort --clear` removes it
gander.py limit 5000                   # set session row limit and re-run
gander.py result [--rows 50] [--offset 0] [--format markdown|csv|json] [--columns a,b]
gander.py schema                       # columns and types of the current result
gander.py catalog [--table orders] [--filter "ord am"]  # tables and columns, fuzzy filtered
gander.py selection                    # what the user has selected
gander.py stats                        # column stats (opens the stats panel)
gander.py stop
gander.py config path | config schema  # where config lives, JSON Schema
gander.py watch                        # stream events as JSON lines
```

`--rows` maps to the API's `rows`; without it the server's `default_rows` applies. `--limit` always means the query row limit. Output defaults to Markdown tables with cells cut at 40 characters, which keeps agent context small. When the underlying result hit the row limit, `run`, `result`, and `status` print a first line such as `truncated: showing 50 of 1000+ rows (limit 1000)` so the agent does not mistake the slice for the whole table. `--format json` gives full values. Exit codes: 0 ok, 1 query error, 2 cannot reach Gander, 3 auth failure, 4 other API errors.

### SKILL.md

`SKILL.md` tells the agent:

- When to use it: the user mentions Gander, the data they are looking at, or asks to change the current query.
- First step is always `gander.py status` to see what the user is looking at.
- Gander has tabs, but the agent only sees the active one. If `status` shows a different tab id than before, the user has switched and earlier results no longer apply. Never assume the agent can reach other tabs or run anything in the background.
- Read before writing: check `result` and `schema` before proposing a new query.
- Prefer `query set` without `--run` when the query is expensive or the user should review it; use `run` for quick exploratory queries.
- Use `catalog` instead of guessing table names.
- Configuration changes go through editing `config.toml` directly; `config schema` lists valid keys, and `status` reports parse errors after the edit.

## Repository layout

```
gander/
  design.md, README.md, app-icon.svg
  package.json, vite.config.ts, components.json, tsconfig.json
  src/                      # React frontend
    App.tsx, main.tsx, store.ts
    components/ui/          # shadcn components (compact variants)
    features/
      tabs/                 # tab strip
      toolbar/              # source picker, run, stop, limit
      editor/               # CodeMirror SQL editor
      grid/                 # virtualized result grid and truncation bar
      sidebar/              # schema tree and fzf filter
      cell-detail/          # JSON popover and pinned panel
      stats-panel/
      statusbar/
      palette/              # command palette
    lib/                    # Tauri bindings, theme, density, formatting
  src-tauri/
    src/
      main.rs, lib.rs       # entry point, menu, startup
      core.rs               # app state: tabs, runs, sources, events
      commands.rs           # Tauri commands for the frontend
      api.rs                # axum router, auth, host checks, SSE
      config.rs             # load, merge, validate, JSON Schema
      watcher.rs            # config and query.sql watching
      paths.rs              # config and state locations
      fuzzy.rs              # nucleo matching for /catalog
      housekeeping.rs       # history and stale tab cleanup
      db/mod.rs             # connections, execution, results, catalog
      db/wrap.rs            # limit and sort wrapper
    tests/api.rs            # API end-to-end test
  skill/gander/
    SKILL.md
    scripts/gander.py
    tests/test_gander.py
```

The macOS app menu leaves out "Close Window" so `Cmd+W` closes a tab instead of the window.

`bundle.targets` in `tauri.conf.json` is `["app", "dmg"]`. Ubuntu support would add `deb` and `appimage`, and needs checks of WebKitGTK rendering of the compact theme and fonts, `inotify` watcher behaviour, and `Ctrl` keybindings in place of `Cmd`.

## Testing

- Rust unit tests cover config merge and validation, the query wrapper, query execution, fuzzy matching, host header checks, and housekeeping.
- A Rust integration test (`src-tauri/tests/api.rs`) starts the API on port 0 against an in-memory DuckDB and exercises every endpoint. It also creates two tabs, switches between them, and confirms that API calls never read or modify the inactive tab.
- Python tests run `gander.py` against a stub server built with `http.server`.
- Config reload behaviour and the UI are checked by hand; the README lists the steps.
- Tests run on macOS only.
