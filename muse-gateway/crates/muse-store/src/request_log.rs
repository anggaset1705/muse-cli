//! `requests.db`: `RequestLog` (Store.cs:405-680).
//!
//! One SQLite row per `/v1/*` request, same schema and indexes as the .NET
//! port (which itself mirrors the earlier Python `services/request_log.py`).
//! The schema SQL below is copied verbatim from `RequestLog.Open`
//! (Store.cs:443-460); the live database in the .NET repo
//! (`src/Muse2Api/data/requests.db`) was checked with `sqlite3 .schema`
//! and matches byte for byte (table + the four indexes).
//!
//! Like .NET, one connection is held under a lock (`Mutex<Connection>`).
//! A failed `add` is the caller's problem here: .NET logs the failure to
//! the console and drops the row, never surfacing it to the client — the
//! gateway should do the same (`let _ = log.add(...)` with a log line).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{now_unix, StoreError};

/// One request row (Store.cs:407-409). `user_agent`, `task_status` and
/// `task_ms` exist as columns but are never read back into a record —
/// `RequestLog.Row` (Store.cs:515-524) ignores them — so they are absent
/// here too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    #[serde(default)]
    pub ts: f64,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub key_name: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub status_code: i32,
    #[serde(default)]
    pub latency_ms: i64,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub poll: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub client_ip: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
CREATE TABLE IF NOT EXISTS requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL, method TEXT NOT NULL, path TEXT NOT NULL,
    model TEXT, key_id TEXT, key_name TEXT, account_id TEXT,
    status_code INTEGER NOT NULL, latency_ms INTEGER NOT NULL,
    stream INTEGER NOT NULL DEFAULT 0, poll INTEGER NOT NULL DEFAULT 0,
    error TEXT, client_ip TEXT, user_agent TEXT, task_id TEXT,
    task_status TEXT, task_ms INTEGER
);
CREATE INDEX IF NOT EXISTS idx_requests_ts ON requests (ts);
CREATE INDEX IF NOT EXISTS idx_requests_key ON requests (key_id, ts);
CREATE INDEX IF NOT EXISTS idx_requests_account ON requests (account_id, ts);
CREATE INDEX IF NOT EXISTS idx_requests_task ON requests (task_id);
"#;

/// `RequestLog` (Store.cs:417-678). `file` may be `":memory:"` for tests.
pub struct RequestLog {
    file: PathBuf,
    conn: Mutex<Connection>,
}

impl RequestLog {
    pub fn open(file: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let file = file.into();
        if file.as_os_str() != ":memory:" {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| StoreError::io_path(dir, e))?;
            }
        }
        let conn = Connection::open(&file)?;
        conn.execute_batch(SCHEMA)?;
        Ok(RequestLog {
            file,
            conn: Mutex::new(conn),
        })
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `RequestLog.Add` (Store.cs:462-513). Only the columns .NET inserts
    /// are written; `user_agent`/`task_status`/`task_ms` stay NULL.
    pub fn add(&self, r: &RequestRecord) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO requests (ts, method, path, model, key_id, key_name,
                account_id, status_code, latency_ms, stream, poll, error,
                client_ip, task_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                r.ts,
                r.method,
                r.path,
                r.model,
                r.key_id,
                r.key_name,
                r.account_id,
                r.status_code,
                r.latency_ms,
                if r.stream { 1 } else { 0 },
                if r.poll { 1 } else { 0 },
                r.error,
                r.client_ip,
                r.task_id,
            ],
        )?;
        Ok(())
    }

    /// `RequestLog.Query` (Store.cs:526-577). Filter semantics, verbatim:
    /// `status = "failed"` → `status_code >= 400`; `"Nxx"` → the `N00`
    /// range; a plain number → exact code; `path` is a LIKE prefix with
    /// `\`/`%`/`_` escaped; `hide_polls` adds `poll = 0`. Rows come back
    /// newest first (`ORDER BY id DESC`) with `LIMIT`/`OFFSET`.
    #[allow(clippy::too_many_arguments)]
    pub fn query(
        &self,
        key_id: Option<&str>,
        account_id: Option<&str>,
        status: Option<&str>,
        path: Option<&str>,
        since: Option<f64>,
        hide_polls: bool,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<RequestRecord>, i64), StoreError> {
        let mut conds: Vec<String> = vec!["1=1".to_string()];
        // (name, value) pairs bound positionally in the order they appear.
        let mut args: Vec<Value> = Vec::new();
        if let Some(k) = key_id {
            conds.push("key_id = ?".to_string());
            args.push(Value::String(k.to_string()));
        }
        if let Some(a) = account_id {
            conds.push("account_id = ?".to_string());
            args.push(Value::String(a.to_string()));
        }
        if let Some(s) = status {
            if s == "failed" {
                conds.push("status_code >= 400".to_string());
            } else if s.len() == 3
                && s.ends_with("xx")
                && s[..1].chars().all(|c| c.is_ascii_digit())
            {
                let d: i64 = s[..1].parse().unwrap_or(0);
                conds.push("status_code >= ? AND status_code < ?".to_string());
                args.push(Value::Number((d * 100).into()));
                args.push(Value::Number((d * 100 + 100).into()));
            } else if let Ok(code) = s.parse::<i64>() {
                conds.push("status_code = ?".to_string());
                args.push(Value::Number(code.into()));
            }
            // Any other status string matches nothing extra, like .NET
            // (its final `else if` simply does not fire).
        }
        if let Some(p) = path {
            let escaped = p
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
                + "%";
            conds.push("path LIKE ? ESCAPE '\\'".to_string());
            args.push(Value::String(escaped));
        }
        if let Some(since) = since {
            conds.push("ts >= ?".to_string());
            args.push(
                serde_json::Number::from_f64(since)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
            );
        }
        if hide_polls {
            conds.push("poll = 0".to_string());
        }
        let cond = conds.join(" AND ");

        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM requests WHERE {cond}"),
            rusqlite::params_from_iter(args.iter().map(value_to_param)),
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT * FROM requests WHERE {cond} ORDER BY id DESC LIMIT ? OFFSET ?"
        ))?;
        let mut all_args: Vec<rusqlite::types::Value> = args.iter().map(value_to_param).collect();
        all_args.push(rusqlite::types::Value::Integer(limit));
        all_args.push(rusqlite::types::Value::Integer(offset));
        let rows = stmt
            .query_map(rusqlite::params_from_iter(all_args.iter()), row_to_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((rows, total))
    }

    fn rows_since(&self, start: f64, exclude_polls: bool) -> Result<Vec<RequestRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = if exclude_polls {
            conn.prepare("SELECT * FROM requests WHERE ts >= ?1 AND poll = 0")?
        } else {
            conn.prepare("SELECT * FROM requests WHERE ts >= ?1")?
        };
        let rows = stmt
            .query_map(params![start], row_to_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// `RequestLog.Stats` (Store.cs:580-634). Shape, bucket math and
    /// percentile rule are verbatim: `1h` → (3600 s, 60 s buckets), `7d` →
    /// (7 d, 3 h buckets), anything else → (24 h, 30 min buckets); `start`
    /// is the first bucket boundary at/after `now - length` plus one bucket;
    /// p50/p95 index is `max(0, min(n-1, ceil(q*n)-1))`.
    pub fn stats(&self, window: &str) -> Result<Value, StoreError> {
        let (length, bucket): (f64, f64) = match window {
            "1h" => (3600.0, 60.0),
            "7d" => (7.0 * 86400.0, 3.0 * 3600.0),
            _ => (86400.0, 1800.0),
        };
        let now = now_unix().floor();
        let start = ((now - length) / bucket).floor() * bucket + bucket;
        let n_buckets = ((now - start) / bucket) as i64 + 1;
        let rows = self.rows_since(start, true)?;
        let mut lat: Vec<i64> = rows.iter().map(|r| r.latency_ms).collect();
        lat.sort_unstable();
        let percentile = |q: f64| -> Value {
            if lat.is_empty() {
                return Value::Null;
            }
            let n = lat.len();
            let idx = ((q * n as f64).ceil() as i64 - 1).clamp(0, n as i64 - 1) as usize;
            Value::Number(lat[idx].into())
        };
        let mut series: Vec<Value> = Vec::new();
        for i in 0..n_buckets.max(0) {
            series.push(serde_json::json!({
                "t": dotnet_number(start + i as f64 * bucket),
                "count": 0,
                "errors": 0,
            }));
        }
        for r in &rows {
            let idx = ((r.ts - start) / bucket) as i64;
            if idx >= 0 && idx < n_buckets {
                let slot = &mut series[idx as usize];
                slot["count"] = Value::Number((slot["count"].as_i64().unwrap_or(0) + 1).into());
                if r.status_code >= 400 {
                    slot["errors"] =
                        Value::Number((slot["errors"].as_i64().unwrap_or(0) + 1).into());
                }
            }
        }
        let total = rows.len();
        let errors = rows.iter().filter(|r| r.status_code >= 400).count();
        let server_errors = rows.iter().filter(|r| r.status_code >= 500).count();
        let error_rate = if total == 0 {
            0.0
        } else {
            errors as f64 / total as f64
        };
        Ok(serde_json::json!({
            "window": window,
            "since": dotnet_number(start),
            "bucket_seconds": dotnet_number(bucket),
            "total": total,
            "errors": errors,
            "server_errors": server_errors,
            "error_rate": error_rate,
            "latency_ms": { "p50": percentile(0.50), "p95": percentile(0.95) },
            "by_key": by(&rows, |r| r.key_id.clone(), |r| r.key_name.clone()),
            "by_account": by(&rows, |r| r.account_id.clone(), |_| None),
            "by_model": by(&rows, |r| r.model.clone(), |_| None),
            "by_status": by(&rows, |r| Some(r.status_code.to_string()), |_| None),
            "series": series,
        }))
    }

    /// `RequestLog.KeyUsage` (Store.cs:657-671). Note the .NET quirk,
    /// preserved: rows are read for **all time** (`RowsSince(0, …)`),
    /// while `requests_24h`/`errors_24h` count only the last 24 h.
    pub fn key_usage(&self) -> Result<Value, StoreError> {
        let since = now_unix().floor() - 86400.0;
        let rows: Vec<RequestRecord> = self
            .rows_since(0.0, true)?
            .into_iter()
            .filter(|r| r.key_id.is_some())
            .collect();
        let mut groups: Vec<(String, Vec<&RequestRecord>)> = Vec::new();
        for r in &rows {
            let key = r.key_id.clone().unwrap();
            if let Some(g) = groups.iter_mut().find(|(k, _)| *k == key) {
                g.1.push(r);
            } else {
                groups.push((key, vec![r]));
            }
        }
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        let out: Vec<Value> = groups
            .into_iter()
            .map(|(key, rs)| {
                let latest = rs.iter().max_by(|a, b| {
                    a.ts.partial_cmp(&b.ts)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                serde_json::json!({
                    "key_id": key,
                    "key_name": latest.and_then(|r| r.key_name.clone()),
                    "last_request_at": dotnet_number(rs.iter().map(|r| r.ts).fold(f64::MIN, f64::max)),
                    "total": rs.len(),
                    "requests_24h": rs.iter().filter(|r| r.ts >= since).count(),
                    "errors_24h": rs.iter().filter(|r| r.ts >= since && r.status_code >= 400).count(),
                })
            })
            .collect();
        Ok(Value::Array(out))
    }

    /// `RequestLog.Prune` (Store.cs:675-688): delete rows older than
    /// `retention_days`; returns the number deleted; `<= 0` deletes nothing.
    pub fn prune(&self, retention_days: f64) -> Result<i64, StoreError> {
        if retention_days <= 0.0 {
            return Ok(0);
        }
        let cutoff = now_unix().floor() - retention_days * 86400.0;
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute("DELETE FROM requests WHERE ts < ?1", params![cutoff])?;
        Ok(deleted as i64)
    }
}

fn value_to_param(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rusqlite::types::Value::Integer(i)
            } else {
                rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        _ => rusqlite::types::Value::Text(v.to_string()),
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<RequestRecord> {
    let opt_text = |name: &str| -> rusqlite::Result<Option<String>> {
        row.get::<_, Option<String>>(name)
    };
    Ok(RequestRecord {
        ts: row.get("ts")?,
        method: row.get::<_, Option<String>>("method")?.unwrap_or_default(),
        path: row.get::<_, Option<String>>("path")?.unwrap_or_default(),
        model: opt_text("model")?,
        key_id: opt_text("key_id")?,
        key_name: opt_text("key_name")?,
        account_id: opt_text("account_id")?,
        status_code: row.get("status_code")?,
        latency_ms: row.get("latency_ms")?,
        stream: row.get::<_, i64>("stream")? != 0,
        poll: row.get::<_, i64>("poll")? != 0,
        error: opt_text("error")?,
        client_ip: opt_text("client_ip")?,
        task_id: opt_text("task_id")?,
    })
}

/// `RequestLog.By` (Store.cs:636-655): top 50 groups by count; every group
/// object carries all five key fields, `null` where the grouping does not
/// apply — exactly like .NET.
fn by(
    rows: &[RequestRecord],
    key: impl Fn(&RequestRecord) -> Option<String>,
    name: impl Fn(&RequestRecord) -> Option<String>,
) -> Value {
    let mut groups: Vec<(Option<String>, Vec<&RequestRecord>)> = Vec::new();
    for r in rows {
        let k = key(r);
        if let Some(g) = groups.iter_mut().find(|(gk, _)| *gk == k) {
            g.1.push(r);
        } else {
            groups.push((k, vec![r]));
        }
    }
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    groups.truncate(50);
    let out: Vec<Value> = groups
        .into_iter()
        .map(|(k, rs)| {
            let first = rs.first();
            let key_name = first.and_then(|r| name(r));
            let status_code = match &k {
                None => Value::Null,
                Some(s) => match s.parse::<i64>() {
                    Ok(n) => Value::Number(n.into()),
                    Err(_) => Value::String(s.clone()),
                },
            };
            serde_json::json!({
                "key_id": k,
                "key_name": key_name,
                "account_id": k,
                "model": k,
                "status_code": status_code,
                "count": rs.len(),
                "errors": rs.iter().filter(|r| r.status_code >= 400).count(),
            })
        })
        .collect();
    Value::Array(out)
}

fn dotnet_number(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
        Value::Number((v as i64).into())
    } else {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ts: f64, status: i32, key: &str) -> RequestRecord {
        RequestRecord {
            ts,
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            model: Some("muse-chat".into()),
            key_id: Some(key.into()),
            key_name: Some("agent".into()),
            account_id: Some("acc_0123456789".into()),
            status_code: status,
            latency_ms: 120,
            stream: true,
            poll: false,
            error: None,
            client_ip: Some("127.0.0.1".into()),
            task_id: None,
        }
    }

    #[test]
    fn add_query_filters() {
        let log = RequestLog::open(":memory:").unwrap();
        let now = now_unix().floor();
        log.add(&rec(now - 10.0, 200, "key_a")).unwrap();
        log.add(&rec(now - 5.0, 500, "key_a")).unwrap();
        let mut poll = rec(now, 200, "key_b");
        poll.poll = true;
        poll.path = "/v1/videos/xyz".into();
        log.add(&poll).unwrap();

        let (rows, total) = log.query(None, None, None, None, None, false, 10, 0).unwrap();
        assert_eq!(total, 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].key_id.as_deref(), Some("key_b")); // newest first

        let (rows, total) = log.query(None, None, Some("failed"), None, None, false, 10, 0).unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].status_code, 500);

        let (_, total) = log.query(None, None, Some("5xx"), None, None, false, 10, 0).unwrap();
        assert_eq!(total, 1);
        let (_, total) = log.query(None, None, Some("200"), None, None, false, 10, 0).unwrap();
        assert_eq!(total, 2);

        let (_, total) = log.query(None, None, None, Some("/v1/videos"), None, false, 10, 0).unwrap();
        assert_eq!(total, 1);

        let (_, total) = log.query(None, None, None, None, None, true, 10, 0).unwrap();
        assert_eq!(total, 2); // polls hidden

        let (_, total) = log.query(Some("key_a"), None, None, None, None, false, 10, 0).unwrap();
        assert_eq!(total, 2);

        let (rows, _) = log.query(None, None, None, None, None, false, 1, 1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status_code, 500);
    }

    #[test]
    fn stats_and_key_usage() {
        let log = RequestLog::open(":memory:").unwrap();
        let now = now_unix().floor();
        log.add(&rec(now - 100.0, 200, "key_a")).unwrap();
        log.add(&rec(now - 50.0, 500, "key_a")).unwrap();
        let stats = log.stats("24h").unwrap();
        assert_eq!(stats["total"], 2);
        assert_eq!(stats["errors"], 1);
        assert_eq!(stats["server_errors"], 1);
        assert!((stats["error_rate"].as_f64().unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(stats["latency_ms"]["p50"], 120);
        assert_eq!(stats["by_key"][0]["key_id"], "key_a");
        assert_eq!(stats["by_key"][0]["count"], 2);
        assert_eq!(stats["by_status"].as_array().unwrap().len(), 2);
        assert!(!stats["series"].as_array().unwrap().is_empty());

        let usage = log.key_usage().unwrap();
        assert_eq!(usage[0]["key_id"], "key_a");
        assert_eq!(usage[0]["total"], 2);
        assert_eq!(usage[0]["requests_24h"], 2);
        assert_eq!(usage[0]["errors_24h"], 1);
    }

    #[test]
    fn prune_retention() {
        let log = RequestLog::open(":memory:").unwrap();
        let now = now_unix().floor();
        log.add(&rec(now - 20.0 * 86400.0, 200, "key_a")).unwrap();
        log.add(&rec(now - 1.0, 200, "key_a")).unwrap();
        assert_eq!(log.prune(14.0).unwrap(), 1);
        let (_, total) = log.query(None, None, None, None, None, false, 10, 0).unwrap();
        assert_eq!(total, 1);
        assert_eq!(log.prune(0.0).unwrap(), 0);
        assert_eq!(log.prune(-1.0).unwrap(), 0);
    }

    #[test]
    fn file_persists_across_reopen() {
        let dir = std::env::temp_dir()
            .join(format!("muse-store-test-reqlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("requests.db");
        {
            let log = RequestLog::open(&file).unwrap();
            log.add(&rec(now_unix().floor(), 200, "key_a")).unwrap();
        }
        let log = RequestLog::open(&file).unwrap();
        let (_, total) = log.query(None, None, None, None, None, false, 10, 0).unwrap();
        assert_eq!(total, 1);
        // Schema matches the .NET one (checked against the live repo DB).
        let conn = log.conn.lock().unwrap();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='requests'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(sql.contains("task_status TEXT"));
        assert!(sql.contains("stream INTEGER NOT NULL DEFAULT 0"));
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='requests'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 4);
    }
}
