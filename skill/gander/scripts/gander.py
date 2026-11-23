#!/usr/bin/env python3
"""Command line client for the Gander agent API.

Gander is a DuckDB viewer. This CLI reads and drives the tab the user is
looking at. It uses only the Python standard library.

Exit codes: 0 ok, 1 query error, 2 cannot reach Gander, 3 auth failure,
4 bad usage or other API error.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

CELL_CHARS = 40


class Fail(Exception):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


def state_dir() -> Path:
    if os.environ.get("GANDER_STATE_DIR"):
        return Path(os.environ["GANDER_STATE_DIR"])
    if os.environ.get("XDG_STATE_HOME"):
        return Path(os.environ["XDG_STATE_HOME"]) / "gander"
    return Path.home() / ".local" / "state" / "gander"


def config_dir() -> Path:
    if os.environ.get("GANDER_CONFIG_DIR"):
        return Path(os.environ["GANDER_CONFIG_DIR"])
    if os.environ.get("XDG_CONFIG_HOME"):
        return Path(os.environ["XDG_CONFIG_HOME"]) / "gander"
    return Path.home() / ".config" / "gander"


def server_info() -> dict:
    try:
        info = json.loads((state_dir() / "server.json").read_text())
        return info if isinstance(info, dict) else {}
    except (OSError, ValueError):
        return {}


def api_key() -> str | None:
    """GANDER_API_KEY wins; otherwise the per-launch key Gander writes to server.json."""
    return os.environ.get("GANDER_API_KEY") or server_info().get("api_key") or None


def base_url() -> str:
    server = state_dir() / "server.json"
    try:
        info = json.loads(server.read_text())
        if info.get("url"):
            return str(info["url"]).rstrip("/")
        if info.get("port"):
            return f"http://127.0.0.1:{info['port']}"
    except (OSError, ValueError):
        pass
    url = os.environ.get("GANDER_URL")
    if url:
        return url.rstrip("/")
    raise Fail(2, f"Gander is not running (no {server}). Start the app, or set GANDER_URL.")


def request(method: str, path: str, body: Any = None, params: dict | None = None) -> tuple[int, dict, Any]:
    url = base_url() + path
    if params:
        q = {k: v for k, v in params.items() if v is not None}
        if q:
            url += "?" + urllib.parse.urlencode(q)
    data = None
    headers = {"Accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    key = api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=600) as resp:
            return resp.status, dict(resp.headers), decode(resp.read(), resp.headers.get("Content-Type", ""))
    except urllib.error.HTTPError as e:
        payload = decode(e.read(), e.headers.get("Content-Type", "") if e.headers else "")
        if e.code == 401:
            raise Fail(3, "API key missing or wrong. Set GANDER_API_KEY.") from None
        return e.code, dict(e.headers or {}), payload
    except urllib.error.URLError as e:
        raise Fail(2, f"cannot reach Gander at {url.split('?')[0]}: {e.reason}") from None


def decode(raw: bytes, ctype: str) -> Any:
    text = raw.decode("utf-8", errors="replace")
    if "json" in ctype:
        try:
            return json.loads(text)
        except ValueError:
            return text
    return text


def check(status: int, body: Any) -> Any:
    if 200 <= status < 300:
        return body
    err = body.get("error") if isinstance(body, dict) else None
    if isinstance(err, dict):
        code, msg = err.get("code", ""), err.get("message") or ""
    else:
        code, msg = "", str(body)
    if status == 422 or code in ("query_error", "too_many_rows", "interrupted"):
        raise Fail(1, f"{code}: {msg}" if code else msg)
    raise Fail(4, f"{status} {code}: {msg}".strip())


# ---- formatting ----


def cell(v: Any, cut: bool = True) -> str:
    if v is None:
        s = "NULL"
    elif isinstance(v, str):
        s = v
    else:
        s = json.dumps(v, ensure_ascii=False, separators=(",", ":"))
    s = s.replace("\n", " ").replace("|", "\\|")
    if cut and len(s) > CELL_CHARS:
        s = s[: CELL_CHARS - 1] + "…"
    return s


def markdown(columns: list[dict], rows: list[list[Any]], cut: bool = True) -> str:
    if not columns:
        return "(no columns)"
    names = [cell(c.get("name", ""), cut=False) for c in columns]
    lines = ["| " + " | ".join(names) + " |", "|" + "|".join("---" for _ in names) + "|"]
    for r in rows:
        lines.append("| " + " | ".join(cell(v, cut) for v in r) + " |")
    return "\n".join(lines)


def truncation_line(body: dict) -> str | None:
    returned = body.get("returned_rows")
    total = body.get("result_rows")
    if returned is None or total is None:
        return None
    if body.get("truncated"):
        return f"truncated: showing {returned} of {total}+ rows (limit {body.get('limit')})"
    if returned < total:
        return f"showing {returned} of {total} rows (use --rows/--offset for more)"
    return None


def print_rows(body: dict, fmt: str) -> None:
    if fmt == "json":
        print(json.dumps(body, indent=2, ensure_ascii=False))
        return
    line = truncation_line(body)
    if line:
        print(line)
    if fmt == "csv":
        import csv
        import io

        buf = io.StringIO()
        w = csv.writer(buf, lineterminator="\n")
        w.writerow([c["name"] for c in body.get("columns", [])])
        for r in body.get("rows", []):
            w.writerow(["" if v is None else v if isinstance(v, str) else json.dumps(v) for v in r])
        print(buf.getvalue(), end="")
        return
    print(markdown(body.get("columns", []), body.get("rows", [])))


def run_summary(body: dict) -> str:
    tab = body.get("tab") or {}
    parts = [f"tab {tab.get('id')} \"{tab.get('title')}\"", f"status {body.get('status')}"]
    if body.get("elapsed_ms") is not None:
        parts.append(f"{body['elapsed_ms']:.0f} ms")
    if body.get("sort"):
        parts.append("sort " + ", ".join(f"{s['column']} {s['dir']}" for s in body["sort"]))
    return " | ".join(parts)


# ---- commands ----


def cmd_status(a) -> int:
    st = check(*request("GET", "/state")[::2])
    src = st.get("source") or {}
    tab = st.get("tab") or {}
    print(f"tab: {tab.get('id')} \"{tab.get('title')}\"")
    where = src.get("path") or "in-memory (query files directly)"
    print(f"source: {src.get('label')} [{src.get('kind')}] {where}{' (read only)' if src.get('read_only') else ''}")
    print(f"status: {st.get('status')}{' error: ' + st['error'] if st.get('error') else ''}")
    rc = st.get("row_count")
    if rc is not None:
        if st.get("truncated"):
            total = st.get("total_count")
            extra = f", {total} in total" if total is not None else ""
            print(f"truncated: {rc}+ rows shown (limit {st.get('limit')}{extra})")
        else:
            print(f"rows: {rc}")
    print(f"limit: {st.get('limit')}")
    if st.get("sort"):
        print("sort: " + ", ".join(f"{s['column']} {s['dir']}" for s in st["sort"]))
    print(f"last change: {st.get('last_change')}{' (editor text differs from last run)' if st.get('dirty') else ''}")
    for e in st.get("config_errors") or []:
        loc = f"{e['file']}:{e['line']}" if e.get("line") else e["file"]
        print(f"config error: {loc}: {e['message']}")
    print("query:")
    print(st.get("query") or "(empty)")
    if st.get("executed_sql"):
        print("executed:")
        print(st["executed_sql"])
    return 0


def read_sql(a) -> str:
    if getattr(a, "file", None):
        return sys.stdin.read() if a.file == "-" else Path(a.file).read_text()
    if a.sql is None:
        raise Fail(4, "give SQL as an argument, or -f FILE (- for stdin)")
    return a.sql


def cmd_query(a) -> int:
    if a.action is None:
        body = check(*request("GET", "/query")[::2])
        print(body.get("sql", ""))
        return 0
    sql = read_sql(a)
    payload: dict[str, Any] = {"sql": sql, "run": a.run}
    if a.rows is not None:
        payload["rows"] = a.rows
    body = check(*request("PUT", "/query", payload)[::2])
    if a.run:
        return show_run(body, a.format)
    print(f"query set in tab {body['tab']['id']} \"{body['tab']['title']}\" (not run)")
    return 0


def show_run(body: dict, fmt: str) -> int:
    if body.get("status") == "running":
        print("still running; check again with: gander.py status")
        return 0
    if fmt == "json":
        print(json.dumps(body, indent=2, ensure_ascii=False))
        return 0
    print(run_summary(body))
    if body.get("columns") is not None:
        print_rows(body, fmt)
    return 0


def cmd_run(a) -> int:
    payload: dict[str, Any] = {}
    if a.sql is not None or a.file:
        payload["sql"] = read_sql(a)
    for k in ("limit", "rows", "wait_ms"):
        v = getattr(a, k)
        if v is not None:
            payload[k] = v
    if a.force:
        payload["force"] = True
    return show_run(check(*request("POST", "/run", payload)[::2]), a.format)


def parse_sort(items: list[str]) -> list[dict]:
    out = []
    for it in items:
        col, _, d = it.rpartition(":")
        if not col:
            col, d = it, "asc"
        d = d.lower()
        if d not in ("asc", "desc"):
            col, d = it, "asc"
        out.append({"column": col, "dir": d})
    return out


def cmd_sort(a) -> int:
    if not a.clear and not a.columns:
        raise Fail(4, "give column:dir pairs, or --clear")
    payload: dict[str, Any] = {"sort": [] if a.clear else parse_sort(a.columns)}
    if a.rows is not None:
        payload["rows"] = a.rows
    body = check(*request("PUT", "/sort", payload)[::2])
    if body.get("note"):
        print(body["note"])
        return 0
    return show_run(body, a.format)


def cmd_limit(a) -> int:
    payload: dict[str, Any] = {"limit": a.limit}
    if a.rows is not None:
        payload["rows"] = a.rows
    if a.force:
        payload["force"] = True
    body = check(*request("PUT", "/limit", payload)[::2])
    if body.get("note"):
        print(body["note"])
        return 0
    return show_run(body, a.format)


def cmd_result(a) -> int:
    params = {"rows": a.rows, "offset": a.offset, "columns": a.columns}
    body = check(*request("GET", "/result", params=params)[::2])
    print_rows(body, a.format)
    return 0


def cmd_schema(a) -> int:
    body = check(*request("GET", "/result/schema")[::2])
    if a.format == "json":
        print(json.dumps(body, indent=2))
        return 0
    trunc = f" (truncated at limit {body.get('limit')})" if body.get("truncated") else ""
    print(f"{body.get('result_rows')} rows{trunc}")
    print(markdown([{"name": "column"}, {"name": "type"}], [[c["name"], c["type"]] for c in body.get("columns", [])], cut=False))
    return 0


def cmd_catalog(a) -> int:
    body = check(*request("GET", "/catalog", params={"q": a.filter, "table": a.table})[::2])
    if a.format == "json":
        print(json.dumps(body, indent=2))
        return 0
    tables = body.get("tables", [])
    if not tables:
        src = body.get("source") or {}
        print("no tables" + ("" if src.get("has_catalog") else "; no database open, query files directly, e.g. FROM 'file.parquet'"))
        return 0
    for t in tables:
        qual = f"{t['schema']}.{t['name']}" if t["schema"] != "main" else t["name"]
        if t["database"] not in ("memory",):
            qual = f"{t['database']}.{qual}"
        cols = ", ".join(f"{c['name']} {c['type']}" for c in t["columns"])
        print(f"{t['kind']} {qual}: {cols}")
    return 0


def cmd_selection(a) -> int:
    body = check(*request("GET", "/selection", params={"rows": a.rows})[::2])
    if body.get("selection") is None:
        print("nothing selected")
        return 0
    print_rows(body, a.format)
    return 0


def cmd_stats(a) -> int:
    status, _, body = request("GET", "/stats", params={"wait_ms": a.wait_ms})
    body = check(status, body)
    if body.get("status") == "running":
        print("stats still running; try again shortly")
        return 0
    if a.format == "json":
        print(json.dumps(body, indent=2))
    else:
        print(markdown(body.get("columns", []), body.get("rows", [])))
    return 0


def cmd_stop(a) -> int:
    check(*request("POST", "/stop", {})[::2])
    print("stop requested")
    return 0


def cmd_config(a) -> int:
    if a.action == "schema":
        p = config_dir() / "schema.json"
        try:
            print(p.read_text())
        except OSError:
            raise Fail(4, f"{p} not found; it is written when Gander starts") from None
        return 0
    print(f"global config: {config_dir() / 'config.toml'}")
    print(f"schema: {config_dir() / 'schema.json'}")
    print(f"server info: {state_dir() / 'server.json'}")
    try:
        st = check(*request("GET", "/state")[::2])
        if st.get("workspace_config_file"):
            print(f"workspace config: {st['workspace_config_file']}")
        print(f"query file (active tab): {st.get('query_file')}")
        for e in st.get("config_errors") or []:
            print(f"config error: {e['file']}:{e.get('line') or '?'}: {e['message']}")
    except Fail as e:
        if e.code != 2:
            raise
        print("(Gander not running; workspace paths unknown)")
    return 0


def cmd_watch(a) -> int:
    url = base_url() + "/events"
    headers = {"Accept": "text/event-stream"}
    key = api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers=headers)) as resp:
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").rstrip("\n")
                if line.startswith("data:"):
                    print(line[5:].strip(), flush=True)
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise Fail(3, "API key missing or wrong. Set GANDER_API_KEY.") from None
        raise Fail(4, f"{e.code}") from None
    except urllib.error.URLError as e:
        raise Fail(2, f"cannot reach Gander: {e.reason}") from None
    except KeyboardInterrupt:
        pass
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="gander.py", description="Read and drive the active tab in Gander (DuckDB viewer).")
    sub = p.add_subparsers(dest="cmd", required=True)

    def fmt(sp, choices=("markdown", "csv", "json")):
        sp.add_argument("--format", choices=choices, default="markdown")

    sp = sub.add_parser("status", help="what the user is looking at")
    sp.set_defaults(func=cmd_status)

    sp = sub.add_parser("query", help="print the editor text, or `query set` to replace it")
    sp.add_argument("action", nargs="?", choices=["set"])
    sp.add_argument("sql", nargs="?")
    sp.add_argument("-f", "--file", help="read SQL from a file (- for stdin)")
    sp.add_argument("--run", action="store_true", help="run after setting")
    sp.add_argument("--rows", type=int, help="rows to return when --run is given")
    fmt(sp)
    sp.set_defaults(func=cmd_query)

    sp = sub.add_parser("run", help="run the editor text, or the given SQL (which replaces the editor text)")
    sp.add_argument("sql", nargs="?")
    sp.add_argument("-f", "--file")
    sp.add_argument("--limit", type=int, help="query row limit for the tab (0 = none)")
    sp.add_argument("--rows", type=int, help="rows to include in the output")
    sp.add_argument("--wait-ms", dest="wait_ms", type=int)
    sp.add_argument("--force", action="store_true", help="allow an unlimited query over 1M rows")
    fmt(sp)
    sp.set_defaults(func=cmd_run)

    sp = sub.add_parser("sort", help="sort by columns and re-run, e.g. sort amount:desc ts:asc")
    sp.add_argument("columns", nargs="*")
    sp.add_argument("--clear", action="store_true")
    sp.add_argument("--rows", type=int)
    fmt(sp)
    sp.set_defaults(func=cmd_sort)

    sp = sub.add_parser("limit", help="set the tab's query row limit and re-run")
    sp.add_argument("limit", type=int)
    sp.add_argument("--rows", type=int)
    sp.add_argument("--force", action="store_true")
    fmt(sp)
    sp.set_defaults(func=cmd_limit)

    sp = sub.add_parser("result", help="rows of the current result")
    sp.add_argument("--rows", type=int)
    sp.add_argument("--offset", type=int)
    sp.add_argument("--columns", help="comma separated column names")
    fmt(sp)
    sp.set_defaults(func=cmd_result)

    sp = sub.add_parser("schema", help="columns and types of the current result")
    fmt(sp, ("markdown", "json"))
    sp.set_defaults(func=cmd_schema)

    sp = sub.add_parser("catalog", help="tables and columns of the open database")
    sp.add_argument("--table")
    sp.add_argument("--filter", help='fzf-style filter, e.g. "ord am"')
    fmt(sp, ("markdown", "json"))
    sp.set_defaults(func=cmd_catalog)

    sp = sub.add_parser("selection", help="cells the user has selected in the grid")
    sp.add_argument("--rows", type=int)
    fmt(sp)
    sp.set_defaults(func=cmd_selection)

    sp = sub.add_parser("stats", help="column stats via SUMMARIZE (opens the stats panel)")
    sp.add_argument("--wait-ms", dest="wait_ms", type=int)
    fmt(sp, ("markdown", "json"))
    sp.set_defaults(func=cmd_stats)

    sp = sub.add_parser("stop", help="interrupt the running query")
    sp.set_defaults(func=cmd_stop)

    sp = sub.add_parser("config", help="config file locations, or the JSON Schema")
    sp.add_argument("action", choices=["path", "schema"])
    sp.set_defaults(func=cmd_config)

    sp = sub.add_parser("watch", help="stream events as JSON lines")
    sp.set_defaults(func=cmd_watch)
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except Fail as e:
        print(f"error: {e.message}", file=sys.stderr)
        return e.code


if __name__ == "__main__":
    sys.exit(main())
