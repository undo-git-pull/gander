---
name: gander
description: Read and drive Gander, the user's DuckDB data viewer. Use when the user mentions Gander, asks about the data or table they are looking at, wants the current query changed or re-run, or wants Gander's config edited.
---

# Gander

Gander is a desktop DuckDB viewer. The user types SQL in tabs and browses results in a grid. This skill talks to its localhost API through `scripts/gander.py`, which needs only Python 3.10+.

The API is a viewport onto what the user sees. You can read the active tab and edit or run its query. You cannot see other tabs, switch tabs, change which database is open, or run anything in the background. Everything you do appears in the user's editor and grid.

## First step

Always start with:

```
python3 scripts/gander.py status
```

It prints the active tab id and title, the open source, the editor text, the SQL that last ran, row count, limit, sort and any config errors. Note the tab id. If a later `status` shows a different id, the user has switched tabs and earlier results no longer apply.

## Reading

```
gander.py result                       # first rows as a Markdown table (server default, usually 50)
gander.py result --rows 200 --offset 50
gander.py result --columns id,amount --format csv
gander.py schema                       # column names and DuckDB types
gander.py catalog                      # tables and columns of the open database
gander.py catalog --filter "ord am"    # fzf-style: every term must match
gander.py selection                    # cells the user selected in the grid
gander.py stats                        # SUMMARIZE per column over the full query
```

Results are capped at the tab's row limit (default 1000). When a result hit the limit, the first output line says `truncated: showing N of 1000+ rows (limit 1000)`. Do not treat a truncated result as the whole table. For totals or distributions, run an aggregate query or use `stats`, which covers all rows.

Markdown cells are cut at 40 characters. Use `--format json` for full values.

`--rows` controls how many rows come back to you. `--limit` changes the query's row limit for the user's grid and re-runs. Keep them apart.

## Changing the query

```
gander.py query set "SELECT ..."             # put text in the editor, do not run
gander.py query set -f /tmp/q.sql --run      # replace and run
gander.py run                                # run the editor text as is
gander.py run "FROM 'data/events.parquet'"   # replace the editor text and run
gander.py sort amount:desc ts:asc            # same as clicking headers; re-runs
gander.py sort --clear
gander.py limit 5000                         # 0 means no limit
gander.py stop
```

Prefer `query set` without `--run` when the query could be slow or when the user should review it first. Use `run` for quick exploration. Your edits replace the editor text; the user can undo them with Cmd+Z.

Use `catalog` instead of guessing table names. With no database open, Gander runs an in-memory DuckDB and files can be queried directly: `FROM 'file.parquet'`, `FROM 'logs.jsonl'`, `FROM read_csv('data/*.csv')`. Relative paths resolve against the user's workspace directory.

Sorting and the limit are applied by wrapping the last statement: `SELECT * FROM (<query>) AS a ORDER BY ... LIMIT n`. Do not add your own outer `LIMIT` just to keep output small; use `--rows`.

## Configuration

Gander's config is TOML and reloads when the file changes.

```
gander.py config path      # global config, workspace config, query.sql locations
gander.py config schema    # JSON Schema listing every valid key
```

Edit `config.toml` directly. Unknown keys are errors. After editing, run `status`: config errors appear there with file and line, and the last good config stays active until the file is fixed. `default_connection` must come before any `[[connections]]` table in the file.

Connections added to a workspace config (`<workspace>/.gander/config.toml`) stay off until the user approves the file in Gander's window, and every edit needs approval again. Tell the user to click Review in the banner; you cannot approve it. Server settings belong in the global config; a workspace `[server]` table is ignored.

`query.sql` in the workspace `.gander/` directory always holds the active tab's text. Writing to it replaces the editor text, the same as `query set`.

## Errors

Exit codes: 0 ok, 1 query error (message on stderr), 2 Gander not running, 3 API key missing or wrong (the CLI reads the per-launch key from `server.json`; set `GANDER_API_KEY` if the config sets its own key), 4 other API errors.

A `too_many_rows` error means the limit is 0 and the query returns over one million rows. Set a limit, or pass `--force` if the user wants every row.
