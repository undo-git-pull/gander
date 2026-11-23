//! Starts the API on port 0 against an in-memory DuckDB and exercises every
//! endpoint, auth, host/origin checks and active-tab scoping.

use gander_lib::{api, core};
use serde_json::{json, Value};
use std::sync::Arc;

struct Ctx {
    base: String,
    port: u16,
    key: &'static str,
    http: reqwest::Client,
}

impl Ctx {
    fn url(&self, p: &str) -> String {
        format!("{}{}", self.base, p)
    }
    async fn get(&self, p: &str) -> (u16, Value) {
        let r = self.http.get(self.url(p)).bearer_auth(self.key).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn get_text(&self, p: &str) -> (u16, String) {
        let r = self.http.get(self.url(p)).bearer_auth(self.key).send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }
    async fn send(&self, method: reqwest::Method, p: &str, body: Value) -> (u16, Value) {
        let r = self.http.request(method, self.url(p)).bearer_auth(self.key).json(&body).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_end_to_end() {
    let cfg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    // Env is process-global; this is the only test in this binary.
    unsafe {
        std::env::set_var("GANDER_CONFIG_DIR", cfg.path());
        std::env::set_var("GANDER_STATE_DIR", state.path());
    }
    std::fs::write(cfg.path().join("config.toml"), "[server]\napi_key = \"sekret\"\ndefault_rows = 7\nmax_rows = 100\n").unwrap();

    let core = core::Core::new(core::CoreOptions {
        workspace: None,
        ui: Box::new(core::NoUi),
        rt: tokio::runtime::Handle::current(),
        ephemeral: true,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = api::router(core.clone(), port);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let c = Ctx { base: format!("http://127.0.0.1:{port}"), port, key: "sekret", http: reqwest::Client::new() };

    // health needs no key
    let r = c.http.get(c.url("/health")).send().await.unwrap();
    assert_eq!(r.status(), 200);

    // auth
    let r = c.http.get(c.url("/state")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = c.http.get(c.url("/state")).bearer_auth("wrong").send().await.unwrap();
    assert_eq!(r.status(), 401);

    // host and origin checks
    let r = c.http.get(c.url("/health")).header("Host", "evil.example:80").send().await.unwrap();
    assert_eq!(r.status(), 403);
    let r = c.http.get(c.url("/health")).header("Host", format!("localhost:{}", c.port)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = c.http.get(c.url("/health")).header("Origin", "http://evil.example").send().await.unwrap();
    assert_eq!(r.status(), 403);

    // state of the initial tab
    let (s, st) = c.get("/state").await;
    assert_eq!(s, 200);
    let first_tab = st["tab"]["id"].as_str().unwrap().to_string();
    assert_eq!(st["source"]["kind"], "memory");
    assert_eq!(st["limit"], 1000);

    // set query without running
    let (s, _) = c.send(reqwest::Method::PUT, "/query", json!({ "sql": "select range as id, range % 7 as m from range(5000)" })).await;
    assert_eq!(s, 200);
    let (_, q) = c.get("/query").await;
    assert_eq!(q["sql"], "select range as id, range % 7 as m from range(5000)");
    assert_eq!(core.get_sql(&first_tab).unwrap(), "select range as id, range % 7 as m from range(5000)");
    let (s, _) = c.get("/result").await;
    assert_eq!(s, 404);

    // run: default rows from config, truncated at limit 1000
    let (s, r) = c.send(reqwest::Method::POST, "/run", json!({})).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["status"], "done");
    assert_eq!(r["returned_rows"], 7);
    assert_eq!(r["result_rows"], 1000);
    assert_eq!(r["truncated"], true);
    assert!(r["executed_sql"].as_str().unwrap().ends_with("LIMIT 1001"));

    // result paging, column filter, clamping, formats
    let (_, r) = c.get("/result?offset=5&rows=3&columns=id").await;
    assert_eq!(r["rows"], json!([["5"], ["6"], ["7"]]));
    assert_eq!(r["columns"][0]["name"], "id");
    let (_, r) = c.get("/result?rows=100000").await;
    assert_eq!(r["returned_rows"], 100);
    let (s, text) = c.get_text("/result?rows=2&format=csv").await;
    assert_eq!(s, 200);
    assert_eq!(text, "id,m\n0,0\n1,1\n");
    let (_, text) = c.get_text("/result?rows=1&format=markdown").await;
    assert_eq!(text, "| id | m |\n|---|---|\n| 0 | 0 |\n");
    let (_, r) = c.get("/result/schema").await;
    assert_eq!(r["columns"][0]["type"], "BIGINT");
    assert_eq!(r["result_rows"], 1000);

    // sort re-runs the last statement
    let (s, r) = c.send(reqwest::Method::PUT, "/sort", json!({ "sort": [{ "column": "id", "dir": "desc" }], "rows": 1 })).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["rows"][0][0], "4999");
    assert!(r["executed_sql"].as_str().unwrap().contains("ORDER BY a.\"id\" DESC"));

    // limit
    let (_, r) = c.send(reqwest::Method::PUT, "/limit", json!({ "limit": 10, "rows": 0 })).await;
    assert_eq!(r["result_rows"], 10);
    assert_eq!(r["returned_rows"], 0);
    assert_eq!(r["truncated"], true);

    // run with sql and limit in one call
    let (_, r) = c.send(reqwest::Method::POST, "/run", json!({ "sql": "select 1 as one", "limit": 50 })).await;
    assert_eq!(r["rows"], json!([[1]]));
    assert_eq!(r["truncated"], false);
    assert_eq!(r["limit"], 50);

    // query errors
    let (s, r) = c.send(reqwest::Method::POST, "/run", json!({ "sql": "select * from nope" })).await;
    assert_eq!(s, 422);
    assert_eq!(r["error"]["code"], "query_error");

    // stats (opens the panel and runs SUMMARIZE)
    c.send(reqwest::Method::POST, "/run", json!({ "sql": "select range as id, 'x' || range as s from range(20)" })).await;
    let (s, r) = c.get("/stats").await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["rows"].as_array().unwrap().len(), 2);
    assert_eq!(r["columns"][0]["name"], "column_name");
    assert!(core.tab_summary(&first_tab).unwrap().stats_open);

    // selection
    let (_, r) = c.get("/selection").await;
    assert_eq!(r["selection"], Value::Null);
    core.set_selection(&first_tab, Some(core::Selection { row_start: 2, row_end: 4, col_start: 1, col_end: 1 })).unwrap();
    let (_, r) = c.get("/selection").await;
    assert_eq!(r["rows"], json!([["x2"], ["x3"], ["x4"]]));

    // catalog
    c.send(reqwest::Method::POST, "/run", json!({ "sql": "create table orders(id int, amount double); create table users(id int, name text); select 1" })).await;
    let (_, r) = c.get("/catalog").await;
    assert_eq!(r["tables"].as_array().unwrap().len(), 2);
    let (_, r) = c.get("/catalog?q=ord%20am").await;
    assert_eq!(r["tables"].as_array().unwrap().len(), 1);
    assert_eq!(r["tables"][0]["columns"][0]["name"], "amount");

    // tab scoping: the user opens a new tab; the API only sees that one
    let before = core.get_sql(&first_tab).unwrap();
    let second = core.new_tab(Some("select 2".into()), true);
    let (_, st) = c.get("/state").await;
    assert_eq!(st["tab"]["id"], second.as_str());
    let (s, _) = c.get("/result").await;
    assert_eq!(s, 404, "the new tab has no result; the old tab's result must not leak");
    c.send(reqwest::Method::PUT, "/query", json!({ "sql": "select 3 as three", "run": true })).await;
    assert_eq!(core.get_sql(&first_tab).unwrap(), before, "inactive tab must not change");
    assert_eq!(core.get_sql(&second).unwrap(), "select 3 as three");
    let (_, r) = c.get("/result").await;
    assert_eq!(r["rows"], json!([[3]]));
    core.activate_tab(&first_tab).unwrap();
    let (_, r) = c.get("/result").await;
    assert_eq!(r["rows"], json!([[1]]));

    // SSE delivers events
    let mut sse = c.http.get(c.url("/events")).bearer_auth(c.key).send().await.unwrap();
    assert_eq!(sse.status(), 200);
    let core2: Arc<core::Core> = core.clone();
    core2.activate_tab(&second).unwrap();
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), sse.chunk()).await.unwrap().unwrap().unwrap();
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("tab_changed"), "{text}");

    // stop is accepted
    let (s, _) = c.send(reqwest::Method::POST, "/stop", json!({})).await;
    assert_eq!(s, 200);
    let (s, _) = c.get("/nope").await;
    assert_eq!(s, 404);

    // Without a configured key the API still requires one: a random key for this launch.
    drop(sse);
    std::fs::write(cfg.path().join("config.toml"), "[server]\ndefault_rows = 7\nmax_rows = 100\n").unwrap();
    core.reload_config();
    assert!(core.api_key_is_generated());
    let generated = core.api_key();
    assert_eq!(generated.len(), 64);
    let r = c.http.get(c.url("/state")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = c.http.get(c.url("/state")).bearer_auth("sekret").send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = c.http.get(c.url("/state")).bearer_auth(&generated).send().await.unwrap();
    assert_eq!(r.status(), 200);
}
