//! Adaptors between crates (integration layer, 2 Nov 2026).
//!
//! The worker crates keep their own public APIs; this module is where
//! they meet:
//!
//! * [`AgentDriverAdapter`] — implements the agent loop's `MuseDriver`
//!   trait by delegating to the browser driver (`muse-browser`), with
//!   the .NET `Gateway` account-lease + failover semantics
//!   (`Gateway.cs`): acquire a pooled account (or the anonymous account
//!   when the driver needs none and the pool is empty), stream deltas,
//!   retry only before the first delta on a retryable upstream error,
//!   release the lease with the matching outcome.
//! * [`ScreenDriverAdapter`] — implements the agent's `ScreenDriver`
//!   trait by delegating to `muse-screen`'s `DesktopComputer`.
//! * [`store_account_to_browser`] — converts the canonical
//!   `muse-store::Account` into the browser driver's local `Account`
//!   wire type (the driver keeps its own minimal type; the conversion
//!   lives here, in the gateway, not in either worker crate).
//! * [`load_image_ref`] / [`load_image_refs`] — `.NET MediaHelp`
//!   equivalents (data URL / public http(s) URL / raw base64, 20 MiB cap).
//! * [`persist_agent_session`] / [`restore_persisted_sessions`] —
//!   agent-session persistence through `muse-store::ChatSessionStore`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use muse_agent::driver::{ChatRequest as AgentChatRequest, DeltaSink, MuseDriver as AgentDriver};
use muse_agent::error::{AgentError, AgentResult, CancellationToken};
use muse_agent::screen::{ScreenDriver as AgentScreenDriver, ScreenResult as AgentScreenResult};
use muse_agent::session::{AgentEvent as AgentSessionEvent, AgentSession};
use muse_browser::{
    types::SessionRefresh, ChatEvent as BrowserChatEvent, ChatRequest as BrowserChatRequest,
    InputImage, MuseDriver as BrowserDriver,
};
use muse_core::{MuseError as CoreError, Prompt};
use muse_store::error::now_unix;
use muse_store::{Account as StoreAccount, AccountPool, ChatSessionStore, ReleaseOutcome};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// store Account -> browser Account
// ---------------------------------------------------------------------------

/// Convert the canonical store account into the browser driver's local
/// account type. Cookie values are strings in both; expiries are
/// `f64` unix seconds in the store and `i64` in the driver; timestamps
/// are `f64` in the store and ISO strings in the driver (the driver only
/// reads `last_used_at`/`cooldown_until` as opaque strings, so they are
/// rendered as plain numbers).
pub fn store_account_to_browser(acc: &StoreAccount) -> muse_browser::Account {
    let cookies: HashMap<String, String> = acc
        .cookies
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
        .collect();
    let cookie_expires: HashMap<String, i64> = acc
        .cookie_expires
        .iter()
        .filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f as i64)))
        .collect();
    muse_browser::Account {
        id: acc.id.clone(),
        label: acc.label.clone(),
        enabled: acc.enabled,
        status: acc.status.as_str().to_string(),
        cookies,
        cookie_expires,
        created_at: format!("{}", acc.created_at as i64),
        last_used_at: if acc.last_used_at > 0.0 {
            Some(format!("{}", acc.last_used_at as i64))
        } else {
            None
        },
        cooldown_until: if acc.cooldown_until > 0.0 {
            Some(format!("{}", acc.cooldown_until as i64))
        } else {
            None
        },
        last_error: acc.last_error.clone(),
        success_count: acc.success_count as u64,
        fail_count: acc.fail_count as u64,
        meta: acc.meta.clone(),
    }
}

/// The anonymous account `.NET Gateway` uses when the driver needs no
/// account and the pool is empty (`Gateway.Anonymous`).
pub fn anonymous_account() -> muse_browser::Account {
    let mut acc = muse_browser::Account::new("anonymous");
    acc.label = "driver without accounts".to_string();
    acc
}

/// Acquire a leased account for one request, or the anonymous account when
/// the driver needs none and the pool is empty (`.NET Gateway` anonymous
/// path). Returns `(leased store account, browser wire account)`; the
/// caller releases the lease with [`release_account`] when the first
/// element is `Some`.
pub async fn acquire_for_request(
    pool: &Arc<Mutex<AccountPool>>,
    requires_account: bool,
    hint: Option<&str>,
    kind: Option<&str>,
) -> Result<(Option<StoreAccount>, muse_browser::Account), CoreError> {
    let pool_empty = pool
        .lock()
        .map(|g| g.store.all().is_empty())
        .unwrap_or(true);
    if !requires_account && pool_empty {
        return Ok((None, anonymous_account()));
    }
    let acc = acquire_account(pool, &[], hint, kind).await?;
    let browser = store_account_to_browser(&acc);
    Ok((Some(acc), browser))
}

/// Apply a refreshed session the driver reported back into the pool
/// (`.NET BrowserDriver` mutates `account.Cookies` directly; in Rust the
/// gateway reads it back via `refreshed_session`). Standalone version of
/// `AgentDriverAdapter::apply_refresh` for the gateway's own chat/image/
/// video paths.
pub fn apply_refresh(
    pool: &Arc<Mutex<AccountPool>>,
    driver: &Arc<dyn muse_browser::MuseDriver>,
    account_id: &str,
) {
    let Some(refresh) = driver.refreshed_session(account_id) else {
        return;
    };
    let Ok(mut guard) = pool.lock() else {
        return;
    };
    let Some(mut acc) = guard.store.get(&refresh.account_id) else {
        return;
    };
    for (k, v) in refresh.cookies {
        acc.cookies.insert(k, serde_json::Value::String(v));
    }
    for (k, v) in refresh.cookie_expires {
        acc.cookie_expires
            .insert(k, serde_json::Value::Number(v.into()));
    }
    let _ = guard.store.upsert(acc);
}

// ---------------------------------------------------------------------------
// account acquire / release (shared by the agent adaptor and the gateway
// chat handlers) — mirrors .NET `AccountPool.AcquireAsync`'s wait loop
// (Store.cs:183-201): poll every 50 ms until `acquire_timeout`, but fail
// fast with NoAccount when no usable account exists at all, and also on
// terminal out-of-credit (quota waterfall — waiting cannot help there
// until a weekly reset).
// ---------------------------------------------------------------------------

pub async fn acquire_account(
    pool: &Arc<Mutex<AccountPool>>,
    tried: &[String],
    hint: Option<&str>,
    kind: Option<&str>,
) -> Result<StoreAccount, CoreError> {
    let timeout = {
        let guard = pool.lock().map_err(|_| CoreError::internal("account pool poisoned"))?;
        guard.acquire_timeout
    };
    let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.0));
    loop {
        // Fail fast on terminal out-of-credit (quota waterfall, 3 Nov 2026):
        // waterfall-exhausted accounts cannot serve until their weekly
        // reset, so waiting the full acquire_timeout would only delay the
        // honest answer. Cooling / in-flight accounts are transient and
        // still worth waiting for — the loop below keeps polling for those.
        if let Some(detail) = pool
            .lock()
            .map(|g| g.out_of_credit(now_unix()))
            .unwrap_or(None)
        {
            return Err(CoreError::no_account(Some(&detail)));
        }
        let attempt = {
            let mut guard = pool.lock().map_err(|_| CoreError::internal("account pool poisoned"))?;
            // No usable account at all (ignoring in-flight) → fail fast,
            // like .NET's early NoAccount throw.
            let any_usable = guard.store.all().iter().any(|a| {
                !tried.iter().any(|t| t == &a.id)
                    && a.enabled
                    && a.status != muse_store::AccountStatus::Invalid
            });
            if !any_usable {
                return Err(CoreError::no_account(None));
            }
            match guard.acquire(tried, hint, kind) {
                Ok(acc) => Ok(acc),
                // Terminal out-of-credit must NOT be swallowed into the
                // retry loop (waiting cannot help until a weekly reset) —
                // surface the clear message immediately.
                Err(muse_store::StoreError::OutOfCredit(msg)) => {
                    return Err(CoreError::no_account(Some(&msg)));
                }
                Err(_) => Err(CoreError::no_account(None)),
            }
        };
        match attempt {
            Ok(acc) => return Ok(acc),
            Err(_) if Instant::now() >= deadline => {
                return Err(CoreError::no_account(Some(
                    "timed out waiting for an available account",
                )))
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

pub fn release_account(pool: &Arc<Mutex<AccountPool>>, id: &str, outcome: ReleaseOutcome) {
    if let Ok(mut guard) = pool.lock() {
        let _ = guard.release(id, outcome);
    }
}

/// Convert a browser driver error to the core error type, preserving the
/// `retryable` flag (the core `new()` constructor defaults it to false).
pub fn browser_to_core(e: muse_browser::MuseError) -> CoreError {
    let mut ce = CoreError::new(e.message, e.status, e.error_type, e.code);
    ce.retryable = e.retryable;
    ce
}

/// `.NET Gateway.ShouldRetry` + `Release`: only an upstream error
/// (502/429/504 or an `upstream_*` code) with `retryable` cools an
/// account; any other driver error only bumps `fail_count`/`last_error`.
/// Public: the image/video handlers in `routes.rs` release leases the
/// same way.
pub fn failure_outcome(err: &muse_browser::MuseError) -> ReleaseOutcome {
    let upstream = err.code.starts_with("upstream_")
        || matches!(err.status, 429 | 502 | 504);
    ReleaseOutcome::Failure {
        message: err.message.clone(),
        code: Some(err.code.clone()),
        retryable: upstream && err.retryable,
    }
}

// ---------------------------------------------------------------------------
// AgentDriverAdapter: agent MuseDriver over the browser driver
// ---------------------------------------------------------------------------

pub struct AgentDriverAdapter {
    browser: Arc<dyn BrowserDriver>,
    pool: Arc<Mutex<AccountPool>>,
    max_failover: i64,
    http: reqwest::Client,
}

impl AgentDriverAdapter {
    pub fn new(
        browser: Arc<dyn BrowserDriver>,
        pool: Arc<Mutex<AccountPool>>,
        max_failover: i64,
        http: reqwest::Client,
    ) -> Self {
        AgentDriverAdapter {
            browser,
            pool,
            max_failover,
            http,
        }
    }

    /// Apply a refreshed session the driver reported back into the pool
    /// (`.NET BrowserDriver` mutates `account.Cookies` directly; in Rust
    /// the gateway reads it back via `refreshed_session` — see
    /// `muse-browser` `TRADEOFFS.md`).
    fn apply_refresh(&self, refresh: SessionRefresh) {
        let Ok(mut guard) = self.pool.lock() else {
            return;
        };
        let Some(mut acc) = guard.store.get(&refresh.account_id) else {
            return;
        };
        for (k, v) in refresh.cookies {
            acc.cookies.insert(k, serde_json::Value::String(v));
        }
        for (k, v) in refresh.cookie_expires {
            acc.cookie_expires
                .insert(k, serde_json::Value::Number(v.into()));
        }
        let _ = guard.store.upsert(acc);
    }
}

#[async_trait]
impl AgentDriver for AgentDriverAdapter {
    async fn chat(
        &self,
        req: AgentChatRequest,
        cancel: &CancellationToken,
        sink: &mut dyn DeltaSink,
    ) -> AgentResult<String> {
        // The agent loop always wants the stable local-executor prompt
        // (`.NET`: the loop calls the gateway with LocalTools).
        let turns = Prompt::message_turns(&req.messages)
            .map_err(|e| AgentError::Message(e.message))?;
        let images = load_image_refs(&req.images, &self.http)
            .await
            .map_err(|e| AgentError::Message(e.message))?;
        let browser_req = BrowserChatRequest {
            prompt: req.prompt.clone(),
            model: req.model.clone(),
            images,
            reply_language: req.reply_language.clone(),
            conversation_hint: req.session_id.clone(),
            turns,
            local_tools: true,
            image_batch_id: Prompt::image_batch_id(&req.messages),
        };

        let needs_account = self.browser.requires_account();
        let pool_empty = {
            let guard = self.pool.lock().map_err(|_| AgentError::Message("account pool poisoned".into()))?;
            guard.store.all().is_empty()
        };
        let mut tried: Vec<String> = Vec::new();
        let mut attempt: i64 = 0;
        loop {
            cancel.throw_if_cancelled()?;
            let store_account: Option<StoreAccount> = if !needs_account && pool_empty {
                None
            } else {
                Some(
                    acquire_account(&self.pool, &tried, req.session_id.as_deref(), Some("chat"))
                        .await
                        .map_err(|e| AgentError::Message(e.message))?,
                )
            };
            let account = match &store_account {
                Some(a) => store_account_to_browser(a),
                None => anonymous_account(),
            };
            let account_id = account.id.clone();
            let mut stream = self.browser.chat_stream(account, browser_req.clone());
            let mut full = String::new();
            let mut emitted = false;
            let mut failure: Option<muse_browser::MuseError> = None;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        drop(stream);
                        if store_account.is_some() {
                            release_account(&self.pool, &account_id, ReleaseOutcome::Cancelled);
                        }
                        return Err(AgentError::Cancelled);
                    }
                    next = stream.next() => {
                        match next {
                            Some(Ok(BrowserChatEvent::AssistantDelta(delta))) => {
                                emitted = true;
                                full.push_str(&delta);
                                sink.on_delta(&delta);
                            }
                            Some(Ok(BrowserChatEvent::Done)) => break,
                            Some(Err(e)) => { failure = Some(e); break; }
                            None => break,
                        }
                    }
                }
            }
            match failure {
                None => {
                    if store_account.is_some() {
                        release_account(&self.pool, &account_id, ReleaseOutcome::Success);
                        if let Some(refresh) = self.browser.refreshed_session(&account_id) {
                            self.apply_refresh(refresh);
                        }
                    }
                    return Ok(full);
                }
                Some(err) => {
                    let retryable_upstream = err.retryable
                        && (err.code.starts_with("upstream_")
                            || matches!(err.status, 429 | 502 | 504));
                    if store_account.is_some() {
                        release_account(&self.pool, &account_id, failure_outcome(&err));
                        tried.push(account_id);
                    }
                    // Local operations must stay in their original
                    // conversation; never retry after output was emitted
                    // (`.NET Gateway.ChatStream`).
                    let has_alt = {
                        let guard = self.pool.lock().map_err(|_| AgentError::Message("account pool poisoned".into()))?;
                        guard.has_alternative(&tried)
                    };
                    if !emitted
                        && retryable_upstream
                        && attempt < self.max_failover
                        && has_alt
                    {
                        attempt += 1;
                        continue;
                    }
                    return Err(AgentError::Message(err.message));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ScreenDriverAdapter: agent ScreenDriver over muse-screen
// ---------------------------------------------------------------------------

pub struct ScreenDriverAdapter {
    computer: Arc<muse_screen::DesktopComputer>,
}

impl ScreenDriverAdapter {
    pub fn new(computer: Arc<muse_screen::DesktopComputer>) -> Self {
        ScreenDriverAdapter { computer }
    }
}

#[async_trait]
impl AgentScreenDriver for ScreenDriverAdapter {
    fn context(&self) -> String {
        self.computer.context()
    }

    fn context_for(&self, reply_language: &str) -> String {
        self.computer.context_for(reply_language)
    }

    async fn execute(
        &self,
        call: &muse_agent::parser::ToolCall,
        _cancel: &CancellationToken,
    ) -> AgentResult<AgentScreenResult> {
        let action = muse_screen::ScreenAction::from_tool_name(&call.name).ok_or_else(|| {
            AgentError::Message(format!("Unknown computer tool: {}", call.name))
        })?;
        let args: muse_screen::ScreenArgs =
            serde_json::from_value(serde_json::Value::Object(call.args.clone()))
                .map_err(|e| AgentError::Message(format!("invalid computer tool arguments: {e}")))?;
        // `DesktopComputer::execute` is synchronous; it does no async work
        // on non-Windows (honest NotImplemented) and blocks a worker
        // thread on Windows by design (`.NET ComputerWorker`).
        match self.computer.execute(action, args) {
            Ok(outcome) => Ok(AgentScreenResult {
                text: outcome.text,
                image_data_url: outcome.image_data_url,
            }),
            Err(e) => Err(AgentError::Message(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Image refs (.NET MediaHelp.LoadImageRef / AppState.LoadImageRefs)
// ---------------------------------------------------------------------------

pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Decode one base64 image payload (`.NET MediaHelp.DecodeImage`).
fn decode_image(payload: &str, mime: &str) -> Result<InputImage, CoreError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|_| CoreError::invalid("image must be non-empty and at most 20 MiB"))?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(CoreError::invalid(
            "image must be non-empty and at most 20 MiB",
        ));
    }
    Ok(InputImage::new(bytes, mime))
}

/// Is this IP public? (`.NET MediaHelp.IsNonPublicIp`, inverted.)
/// Rejects loopback, private (RFC 1918 + CGNAT), link-local, unspecified,
/// multicast, reserved and documentation ranges.
fn is_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_multicast() || v4.is_unspecified() || v4.is_broadcast()
            || v4.is_documentation()
            // 198.18.0.0/15 benchmarking (is_benchmarking is unstable here).
            || (v4.octets()[0] == 198 && v4.octets()[1] & 0xfe == 18)
            // 240.0.0.0/4 reserved (is_reserved is unstable here).
            || (v4.octets()[0] & 0xf0 == 240)
        {
                return false;
            }
            // CGNAT 100.64.0.0/10.
            if o[0] == 100 && (64..=127).contains(&o[1]) {
                return false;
            }
            true
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local())
        }
    }
}

/// Load one image reference: `data:` URL (base64), public `http(s)` URL
/// (max 5 redirects, 20 s, 20 MiB, no proxy — `.NET MediaHelp`), or raw
/// base64 (assumed `image/png`).
pub async fn load_image_ref(reference: &str, http: &reqwest::Client) -> Result<InputImage, CoreError> {
    let r = reference.trim();
    if r.to_ascii_lowercase().starts_with("data:") {
        let comma = r
            .find(',')
            .ok_or_else(|| CoreError::invalid("image data URL must contain base64 data"))?;
        let header = &r[..comma];
        if !header.to_ascii_lowercase().ends_with(";base64") {
            return Err(CoreError::invalid("image data URL must contain base64 data"));
        }
        let mime = header[5..]
            .split(';')
            .next()
            .unwrap_or("image/png")
            .to_string();
        return decode_image(&r[comma + 1..], &mime);
    }
    if r.to_ascii_lowercase().starts_with("http://") || r.to_ascii_lowercase().starts_with("https://") {
        let url = reqwest::Url::parse(r).map_err(|_| CoreError::invalid("invalid image_url"))?;
        if !url.username().is_empty() || !url.password().is_some_and(|p| !p.is_empty()) {
            return Err(CoreError::invalid(
                "image_url must be an http(s) URL without credentials",
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| CoreError::invalid("invalid image_url"))?
            .to_string();
        let port = url.port_or_known_default().unwrap_or(443);
        // SSRF guard: the host must resolve to public IPs only.
        let addrs = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| CoreError::invalid("image_url must resolve to a public IP address"))?;
        let mut any = false;
        for addr in addrs {
            any = true;
            if !is_public_ip(&addr.ip()) {
                return Err(CoreError::invalid(
                    "image_url must resolve to a public IP address",
                ));
            }
        }
        if !any {
            return Err(CoreError::invalid(
                "image_url must resolve to a public IP address",
            ));
        }
        let resp = http
            .get(url.clone())
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| CoreError::invalid(format!("could not fetch image_url: {e}")))?;
        if let Some(len) = resp.content_length() {
            if len > MAX_IMAGE_BYTES as u64 {
                return Err(CoreError::invalid("image exceeds 20 MiB"));
            }
        }
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/png")
            .split(';')
            .next()
            .unwrap_or("image/png")
            .trim()
            .to_string();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| CoreError::invalid(format!("could not fetch image_url: {e}")))?;
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return Err(CoreError::invalid(
                "image must be non-empty and at most 20 MiB",
            ));
        }
        return Ok(InputImage::new(bytes.to_vec(), mime));
    }
    decode_image(r, "image/png")
}

pub async fn load_image_refs(
    refs: &[String],
    http: &reqwest::Client,
) -> Result<Vec<InputImage>, CoreError> {
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        out.push(load_image_ref(r, http).await?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Agent-session persistence (muse-store ChatSessionStore)
// ---------------------------------------------------------------------------

/// Save an agent session's history/goal/events to
/// `<data>/agent-sessions/<sha256>/<id>.json` (`.NET ChatSessionStore`).
/// Best-effort: failures are logged to stderr, never fail the request.
pub fn persist_agent_session(
    store: &ChatSessionStore,
    session: &AgentSession,
    events: &[AgentSessionEvent],
) {
    let mut cs = muse_store::ChatSession {
        id: session.id.clone(),
        title: session.title.clone(),
        workspace: session.workspace.to_string_lossy().into_owned(),
        model: session.model.clone(),
        updated_at: session.updated_at,
        history: session
            .history
            .iter()
            .filter_map(|v| v.as_object().cloned())
            .collect(),
        events: events
            .iter()
            .map(|e| muse_store::AgentEvent {
                kind: e.kind.clone(),
                text: e.text.clone(),
                tool: e.tool.clone(),
                path: e.path.clone(),
            })
            .collect(),
        goal: session.goal.as_ref().map(|g| muse_store::GoalRun {
            objective: g.objective.clone(),
            status: g.status.clone(),
            summary: g.summary.clone(),
            updated_at: g
                .updated_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap_or_else(|_| chrono::Utc::now()),
            pending_question: g.pending_question.clone(),
            follow_ups: g.follow_ups.clone(),
        }),
    };
    if let Err(e) = store.save(&mut cs) {
        eprintln!("muse-gateway: failed to persist agent session {}: {e}", session.id);
    }
}

/// Restore persisted sessions at startup: scan
/// `<data>/agent-sessions/**/<id>.json`, rebuild the in-memory session
/// (new `ToolExecutor` over the stored workspace), and put it back in the
/// manager. Corrupt files and missing workspaces are skipped with a
/// stderr note; nothing here fails startup.
pub async fn restore_persisted_sessions(state: &Arc<AppState>) {
    let root = state.settings.data_dir.join("agent-sessions");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_json_files(&root, &mut files);
    if files.is_empty() {
        return;
    }
    let mut restored = 0usize;
    let mut mgr = state.sessions.lock().await;
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(stored) = serde_json::from_str::<muse_store::ChatSession>(&text) else {
            eprintln!("muse-gateway: skipping corrupt agent session file {}", file.display());
            continue;
        };
        if !std::path::Path::new(&stored.workspace).is_dir() {
            eprintln!(
                "muse-gateway: skipping persisted session {} (workspace gone: {})",
                stored.id, stored.workspace
            );
            continue;
        }
        let Ok(mut session) = AgentSession::new(
            &stored.workspace,
            "auto",
            &stored.model,
            "English",
            false,
            None,
        ) else {
            continue;
        };
        session.id = stored.id.clone();
        session.title = stored.title.clone();
        session.reply_language = "English".to_string();
        session.history = stored
            .history
            .into_iter()
            .map(serde_json::Value::Object)
            .collect();
        session.goal = stored.goal.map(|g| muse_agent::goal::GoalRun {
            objective: g.objective,
            status: g.status,
            summary: g.summary,
            updated_at: g.updated_at.to_rfc3339(),
            pending_question: g.pending_question,
            follow_ups: g.follow_ups,
        });
        session.updated_at = stored.updated_at;
        session.skills_directory = mgr.skills_directory.clone();
        mgr.put_back(session);
        restored += 1;
    }
    if restored > 0 {
        println!("muse-gateway: restored {restored} persisted agent session(s)");
    }
}

fn collect_json_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
}

/// Drives a browser chat stream in a background task and forwards deltas to a
/// channel, so the HTTP layer can stop polling at any time without leaking the
/// account lease: when the receiver is dropped the task releases the lease as
/// `Cancelled` and exits. Failover mirrors `AgentDriverAdapter::chat`: retry
/// only before the first delta, only for retryable upstream errors, only while
/// an alternative account exists and `attempts < settings.max_failover`.
/// On final failure one `Err` is sent, then the channel closes; on success the
/// channel simply closes after the last delta. The account id used is written
/// to `account_id_out` (for request logging) as soon as it is known; it stays
/// `None` on the anonymous path.
pub fn spawn_browser_chat(
    state: Arc<AppState>,
    req: BrowserChatRequest,
    hint: Option<String>,
    kind: &'static str,
    account_id_out: Arc<Mutex<Option<String>>>,
) -> mpsc::UnboundedReceiver<Result<String, CoreError>> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let max_failover = state.settings.max_failover;
        let mut tried: Vec<String> = Vec::new();
        let mut attempts: i64 = 0;
        loop {
            let acquired =
                acquire_for_request(&state.pool, state.driver.requires_account(), hint.as_deref(), Some(kind)).await;
            let (store_account, browser_account) = match acquired {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let account_id = browser_account.id.clone();
            *account_id_out.lock().unwrap() = store_account.as_ref().map(|a| a.id.clone());
            let mut stream = state.driver.chat_stream(browser_account.clone(), req.clone());
            let mut emitted = false;
            let mut failure: Option<muse_browser::MuseError> = None;
            let mut done_ok = false;
            loop {
                match stream.next().await {
                    Some(Ok(BrowserChatEvent::AssistantDelta(d))) => {
                        emitted = true;
                        if tx.send(Ok(d)).is_err() {
                            if store_account.is_some() {
                                release_account(&state.pool, &account_id, ReleaseOutcome::Cancelled);
                            }
                            return;
                        }
                    }
                    Some(Ok(BrowserChatEvent::Done)) | None => {
                        done_ok = true;
                        break;
                    }
                    Some(Err(e)) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            if done_ok {
                if store_account.is_some() {
                    release_account(&state.pool, &account_id, ReleaseOutcome::Success);
                    apply_refresh(&state.pool, &state.driver, &account_id);
                }
                return;
            }
            let e = failure.expect("failure set when not done_ok");
            if store_account.is_some() {
                release_account(&state.pool, &account_id, failure_outcome(&e));
                tried.push(account_id);
            }
            let retryable_upstream = e.retryable
                && (e.code.starts_with("upstream_") || matches!(e.status, 429 | 502 | 504));
            let alternative = state
                .pool
                .lock()
                .map(|g| g.has_alternative(&tried))
                .unwrap_or(false);
            if !emitted && retryable_upstream && attempts < max_failover && alternative {
                attempts += 1;
                continue;
            }
            let _ = tx.send(Err(browser_to_core(e)));
            return;
        }
    });
    rx
}
