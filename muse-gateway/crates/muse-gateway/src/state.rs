//! Shared application state, mirroring .NET `AppState`
//! (`Muse2Api/AppState.cs`): settings, driver, account pool, key/task/
//! request/media/skill stores, and the agent session manager.
//!
//! Agent sessions live in `muse-agent`'s `AgentSessionManager` (in
//! memory). A session checked out for a running turn or direct tool call
//! is tracked in `checked_out` (view snapshot with `running: true` +
//! changes snapshot) so `GET /v1/agent/sessions/{id}` and `list` keep
//! answering while the turn runs; `deleted_while_running` records ids
//! deleted mid-turn so the turn task does not resurrect them on put-back.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use muse_core::Settings;
use muse_store::{
    AccountPool, ChatSessionStore, KeyStore, MediaStore, RequestLog, SkillStore, TaskStore,
};

/// Snapshot of a session checked out of the manager for a running turn.
#[derive(Debug, Clone)]
pub struct CheckedOut {
    /// `SessionView` with `running: true`.
    pub view: serde_json::Value,
    /// `ChangesView` as of checkout (stale during the turn, refreshed on
    /// put-back by the turn task via the live session).
    pub changes: serde_json::Value,
}

pub struct AppState {
    pub settings: Settings,
    /// Browser driver (`mock` / `browser` / `http`) — `muse-browser`.
    pub driver: Arc<dyn muse_browser::MuseDriver>,
    /// Agent-loop driver — adaptor over `driver` (see `adapters.rs`).
    pub agent_driver: Arc<dyn muse_agent::driver::MuseDriver>,
    pub pool: Arc<Mutex<AccountPool>>,
    pub keys: Arc<Mutex<KeyStore>>,
    pub tasks: Arc<Mutex<TaskStore>>,
    pub requests: Arc<RequestLog>,
    pub media: Arc<MediaStore>,
    pub skills: Arc<SkillStore>,
    pub chat_sessions: Arc<ChatSessionStore>,
    pub sessions: tokio::sync::Mutex<muse_agent::session::AgentSessionManager>,
    pub checked_out: Mutex<HashMap<String, CheckedOut>>,
    pub deleted_while_running: Mutex<HashSet<String>>,
    /// Screen control (`muse-screen`); the agent loop's screen driver
    /// adaptor wraps this. On non-Windows it reports unavailable.
    /// (Kept for ownership/documentation; the adaptor holds its own clone.)
    #[allow(dead_code)]
    pub screen: Arc<muse_screen::DesktopComputer>,
    /// Agent-loop screen driver — adaptor over `screen` (see `adapters.rs`),
    /// installed on every agent session's `ToolExecutor`.
    pub screen_driver: Arc<dyn muse_agent::screen::ScreenDriver>,
    /// Shared HTTP client (image-ref downloads).
    pub http_client: reqwest::Client,
    /// Cancellation flags for running image/video background tasks
    /// (`.NET TaskManager.Cancel` cancels the live runner via a token;
    /// here each runner polls this flag before/after the driver call and
    /// skips its final store update when cancelled).
    pub task_cancels: Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
}

impl AppState {
    /// Pool stats for `/readyz` and `/admin/status`, mirroring .NET
    /// `AccountPool.Stats()`: `{"total","available","invalid","inflight"}`
    /// plus the quota-waterfall counts (3 Nov 2026):
    /// `{"weekly","extra","exhausted"}` — accounts whose last-known quota
    /// snapshot still has weekly quota left / only extra tokens left /
    /// nothing left. `available` counts accounts not cooling/in-flight
    /// regardless of quota; use `exhausted == total` (or the out-of-credit
    /// 503 on chat) to detect a dry pool.
    pub fn pool_stats(&self) -> serde_json::Value {
        let stats = self
            .pool
            .lock()
            .map(|p| p.stats())
            .unwrap_or_default();
        serde_json::json!({
            "total": stats.get("total").copied().unwrap_or(0),
            "available": stats.get("available").copied().unwrap_or(0),
            "invalid": stats.get("invalid").copied().unwrap_or(0),
            "inflight": stats.get("inflight").copied().unwrap_or(0),
            "weekly": stats.get("weekly").copied().unwrap_or(0),
            "extra": stats.get("extra").copied().unwrap_or(0),
            "exhausted": stats.get("exhausted").copied().unwrap_or(0),
        })
    }
}

/// Current UTC time in .NET `DateTimeOffset.ToString("o")` (round-trip)
/// format, e.g. `2026-11-02T05:00:00.1234567+00:00`.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::AutoSi, false)
}

/// Unix timestamp in seconds (for `/v1/models` `created` fields).
pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Unix timestamp as `f64` (for store timestamps).
pub fn now_unix_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Approx token count (`.NET JsonHelp.ApproxTokens`): `max(1, len/4)`.
pub fn approx_tokens(text: &str) -> i64 {
    std::cmp::max(1, text.len() as i64 / 4)
}

/// `chatcmpl-` / `resp_` / `msg_` ids: prefix + 24 lowercase hex chars
/// (`.NET Guid.NewGuid().ToString("N")[..24]`).
pub fn new_id24(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple().to_string()[..24].to_string())
}
