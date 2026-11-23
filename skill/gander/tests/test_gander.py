"""Tests for gander.py against a stub HTTP server.

Run: python3 -m unittest discover -s skill/gander/tests
"""

import contextlib
import io
import json
import os
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import gander  # noqa: E402

RESULT = {
    "tab": {"id": "t1", "title": "orders"},
    "columns": [{"name": "id", "type": "BIGINT"}, {"name": "meta", "type": "JSON"}],
    "rows": [["1", {"a": "x" * 60}], ["2", None]],
    "offset": 0,
    "returned_rows": 2,
    "result_rows": 1000,
    "truncated": True,
    "limit": 1000,
}


class Stub(BaseHTTPRequestHandler):
    calls: list = []
    key: str | None = None

    def log_message(self, *a):
        pass

    def _send(self, status, body, ctype="application/json"):
        data = body.encode() if isinstance(body, str) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _handle(self, method):
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length)) if length else None
        Stub.calls.append((method, self.path, body, dict(self.headers)))
        if Stub.key and self.headers.get("Authorization") != f"Bearer {Stub.key}":
            return self._send(401, {"error": {"code": "unauthorized", "message": "no"}})
        path = self.path.split("?")[0]
        if path == "/state":
            return self._send(200, {
                "tab": {"id": "t1", "title": "orders"},
                "source": {"kind": "memory", "label": "Files only", "path": None, "read_only": False},
                "query": "from orders",
                "executed_sql": "SELECT * FROM (\nfrom orders\n) AS a LIMIT 1001",
                "status": "done", "error": None, "row_count": 1000, "truncated": True,
                "total_count": None, "limit": 1000, "sort": [], "last_change": "user", "dirty": False,
                "config_errors": [{"file": "/c/config.toml", "line": 3, "message": "unknown field `bogus`"}],
            })
        if path == "/result":
            return self._send(200, RESULT)
        if path == "/query" and method == "PUT":
            return self._send(200, {"tab": {"id": "t1", "title": "orders"}, "sql": body["sql"]})
        if path == "/run":
            if body and body.get("sql") == "bad":
                return self._send(422, {"tab": {"id": "t1"}, "status": "error", "error": {"code": "query_error", "message": "Catalog Error: nope"}})
            return self._send(200, {**RESULT, "status": "done", "elapsed_ms": 12.0, "sort": []})
        if path == "/sort":
            return self._send(200, {**RESULT, "status": "done", "sort": body["sort"]})
        return self._send(404, {"error": {"code": "not_found", "message": "unknown endpoint"}})

    def do_GET(self):
        self._handle("GET")

    def do_PUT(self):
        self._handle("PUT")

    def do_POST(self):
        self._handle("POST")


def run_cli(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = gander.main(list(argv))
    return code, out.getvalue(), err.getvalue()


class GanderCliTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Stub)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.state = tempfile.TemporaryDirectory()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.state.cleanup()

    def setUp(self):
        Stub.calls = []
        Stub.key = None
        self.env = dict(os.environ)
        os.environ["GANDER_STATE_DIR"] = self.state.name
        os.environ.pop("GANDER_API_KEY", None)
        os.environ["GANDER_URL"] = f"http://127.0.0.1:{self.server.server_address[1]}"

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.env)

    def test_status(self):
        code, out, _ = run_cli("status")
        self.assertEqual(code, 0)
        self.assertIn('tab: t1 "orders"', out)
        self.assertIn("truncated: 1000+ rows shown (limit 1000)", out)
        self.assertIn("config error: /c/config.toml:3: unknown field `bogus`", out)
        self.assertIn("from orders", out)

    def test_result_markdown_truncation_and_cell_cut(self):
        code, out, _ = run_cli("result", "--rows", "2")
        self.assertEqual(code, 0)
        lines = out.splitlines()
        self.assertEqual(lines[0], "truncated: showing 2 of 1000+ rows (limit 1000)")
        self.assertEqual(lines[1], "| id | meta |")
        self.assertIn("…", lines[3])
        self.assertLessEqual(len(lines[3].split(" | ")[1].rstrip(" |")), 40)
        self.assertIn("| 2 | NULL |", out)
        self.assertIn("rows=2", Stub.calls[0][1])

    def test_result_json_is_raw(self):
        code, out, _ = run_cli("result", "--format", "json")
        self.assertEqual(json.loads(out)["rows"][0][1]["a"], "x" * 60)

    def test_query_set_sends_put(self):
        code, out, _ = run_cli("query", "set", "select 42")
        self.assertEqual(code, 0)
        method, path, body, _ = Stub.calls[0]
        self.assertEqual((method, path), ("PUT", "/query"))
        self.assertEqual(body, {"sql": "select 42", "run": False})
        self.assertIn("not run", out)

    def test_run_with_limit_and_rows(self):
        code, out, _ = run_cli("run", "from t", "--limit", "10", "--rows", "5")
        self.assertEqual(code, 0)
        self.assertEqual(Stub.calls[0][2], {"sql": "from t", "limit": 10, "rows": 5})
        self.assertIn("status done", out)

    def test_query_error_exit_1(self):
        code, _, err = run_cli("run", "bad")
        self.assertEqual(code, 1)
        self.assertIn("Catalog Error", err)

    def test_sort_parsing(self):
        self.assertEqual(
            gander.parse_sort(["amount:desc", "ts", "a:b:asc"]),
            [{"column": "amount", "dir": "desc"}, {"column": "ts", "dir": "asc"}, {"column": "a:b", "dir": "asc"}],
        )
        code, _, _ = run_cli("sort", "amount:desc")
        self.assertEqual(code, 0)
        self.assertEqual(Stub.calls[0][2], {"sort": [{"column": "amount", "dir": "desc"}]})

    def test_auth(self):
        Stub.key = "k1"
        code, _, err = run_cli("status")
        self.assertEqual(code, 3)
        os.environ["GANDER_API_KEY"] = "k1"
        code, _, _ = run_cli("status")
        self.assertEqual(code, 0)
        self.assertEqual(Stub.calls[-1][3].get("Authorization"), "Bearer k1")

    def test_key_from_server_json(self):
        Stub.key = "launchkey"
        Path(self.state.name, "server.json").write_text(json.dumps({"port": self.server.server_address[1], "api_key": "launchkey"}))
        try:
            code, _, _ = run_cli("status")
            self.assertEqual(code, 0)
            self.assertEqual(Stub.calls[-1][3].get("Authorization"), "Bearer launchkey")
            # GANDER_API_KEY overrides the file.
            os.environ["GANDER_API_KEY"] = "other"
            code, _, _ = run_cli("status")
            self.assertEqual(code, 3)
        finally:
            Path(self.state.name, "server.json").unlink()

    def test_server_json_preferred(self):
        Path(self.state.name, "server.json").write_text(json.dumps({"port": self.server.server_address[1]}))
        try:
            os.environ["GANDER_URL"] = "http://127.0.0.1:9"
            code, _, _ = run_cli("status")
            self.assertEqual(code, 0)
        finally:
            Path(self.state.name, "server.json").unlink()

    def test_unreachable_exit_2(self):
        os.environ["GANDER_URL"] = "http://127.0.0.1:9"
        code, _, err = run_cli("status")
        self.assertEqual(code, 2)
        self.assertIn("cannot reach", err)

    def test_not_running_exit_2(self):
        os.environ.pop("GANDER_URL")
        code, _, err = run_cli("status")
        self.assertEqual(code, 2)
        self.assertIn("not running", err)


if __name__ == "__main__":
    unittest.main()
