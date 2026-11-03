//! HTTP routes, mirroring .NET `Endpoints.cs` + `AgentEndpoints.cs`
//! (see `INVENTORY.md` for the route-by-route contract).
//!
//! Deviations from .NET that are structural (not per-route) are:
//!
//! * D-log-1 — request logging records `client_ip: null`: the gateway does
//!   not wire axum `ConnectInfo` (loopback-only listener, so the peer is
//!   always 127.0.0.1 / ::1 anyway).
//! * D-log-2 — `GET /admin/requests` hides polls only when the `hide_polls`
//!   query parameter is present AND != `"false"` (the .NET source
//!   behaviour; `INVENTORY.md` said "default true" — the source wins).
//! * D-img-1 — image/video background tasks run on a gateway-spawned tokio
//!   task (the store has no runner). Cancelling sets a flag the runner
//!   checks before/after the driver call; a driver call already in flight
//!   cannot be interrupted mid-flight.
//! * D-agt-1 — a direct-tool call (`POST /v1/agent/sessions/{id}/tools`)
//!   or a revert while a turn is running returns 409 `session_busy`
//!   instead of running concurrently: the gateway holds the session
//!   exclusively during a turn (`.NET` locks the session the same way
//!   inside `RunTurnAsync`, but its direct-tool path could run between
//!   turns; here the turn holds the session for its whole duration).
//! * D-agt-2 — `GET /v1/agent/sessions/{id}/changes` while a turn runs
//!   returns the changes snapshot taken at checkout (stale during the
//!   turn), rather than 409.
//! * D-dash-1 — `/dashboard` is a simple admin status page titled
//!   "muse-gateway".
//! * D-scr-1 — `POST /v1/agent/screen/{action}` is REAL here, unlike
//!   .NET `AgentEndpoints` (which always throws 501: the .NET gateway
//!   host has no computer implementation). It accepts the `computer_*`
//!   tool names plus session actions `list_windows` / `authorize` /
//!   `preview` / `disarm` / `auto_authorize`; on non-Windows computer
//!   actions still answer the honest 501. `read_only` (a Rust
//!   extension to the preferences, E6) refuses every screen action
//!   except `disarm` with 403.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use muse_agent::chat_turn::{direct_tool_may_run, run_turn, TurnOptions};
use muse_agent::error::CancellationToken;
use muse_agent::parser::{validate as validate_tool_call, ToolCall};
use muse_agent::session::{done_event, AgentEvent};
use muse_browser::{ChatRequest as BrowserChatRequest, ImageRequest, InputImage, VideoRequest};
use muse_core::{ModelRegistry, ModelSpec, MuseError as CoreError, Prompt};
use muse_store::{
    account::missing_session_cookies, Account as StoreAccount, AccountStatus, BgTask,
    MediaStore, ReleaseOutcome, RequestRecord,
};

use crate::adapters::{
    acquire_for_request, apply_refresh, browser_to_core, failure_outcome, load_image_ref,
    load_image_refs, persist_agent_session, release_account, spawn_browser_chat,
    store_account_to_browser,
};
use crate::auth::{require_admin, require_agent_api, require_api, Caller};
use crate::errors::{ApiResult, MuseError};
use crate::state::{approx_tokens, new_id24, now_rfc3339, now_unix, now_unix_f64, AppState};

/// Dashboard page: simple admin status page (see D-dash-1).
const DASHBOARD_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>muse-gateway</title>
<style>body{font-family:system-ui;margin:2rem}button{margin:.25rem;padding:.4rem .8rem}pre{background:#111;color:#eee;padding:1rem;overflow:auto}</style></head>
<body><h1>muse-gateway</h1>
<p>Admin key: <input id="k" type="password" size="40" placeholder="admin key"> <button onclick="localStorage.setItem('k',document.getElementById('k').value)">save</button></p>
<p><button onclick="show('/admin/status')">status</button><button onclick="show('/admin/accounts')">accounts</button><button onclick="show('/admin/stats')">stats</button><button onclick="show('/admin/metrics')">metrics</button></p>
<pre id="out">pick an endpoint</pre>
<script>document.getElementById('k').value=localStorage.getItem('k')||'';
async function show(p){const r=await fetch(p,{headers:{Authorization:'Bearer '+document.getElementById('k').value}});document.getElementById('out').textContent=await r.text();}</script>
</body></html>"#;

/// The router: every route from CONTRACTS.md §2a+§2b.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // Public.
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/media/{name}", get(media_get))
        .route("/dashboard", get(dashboard))
        // OpenAI-compatible (api auth).
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/responses", post(responses))
        .route("/v1/images/generations", post(image_generations))
        .route("/v1/images/generations/{taskId}", get(image_task_get))
        .route("/v1/images/edits", post(image_edits))
        .route("/v1/videos", post(video_create))
        .route("/v1/videos/{taskId}", get(video_task_get))
        // Admin (admin auth only).
        .route("/admin/status", get(admin_status))
        .route("/admin/accounts", get(admin_accounts).post(admin_account_add))
        .route("/admin/accounts/import", post(admin_accounts_import))
        .route(
            "/admin/accounts/{id}",
            patch(admin_account_patch).delete(admin_account_delete),
        )
        .route("/admin/accounts/{id}/reset", post(admin_account_reset))
        .route("/admin/accounts/{id}/renew", post(admin_account_renew))
        .route("/admin/accounts/{id}/quota", get(admin_account_quota))
        .route("/admin/extension.zip", get(admin_extension_zip))
        .route("/admin/tasks", get(admin_tasks))
        .route("/admin/tasks/{taskId}/cancel", post(admin_task_cancel))
        .route("/admin/keys", get(admin_keys).post(admin_key_add))
        .route("/admin/keys/usage", get(admin_keys_usage))
        .route("/admin/keys/{keyId}", patch(admin_key_patch).delete(admin_key_delete))
        .route("/admin/requests", get(admin_requests))
        .route("/admin/stats", get(admin_stats))
        .route("/admin/metrics", get(admin_metrics))
        // Agent (agent api auth).
        .route("/v1/agent/sessions", post(agent_session_create).get(agent_sessions_list))
        .route(
            "/v1/agent/sessions/{id}",
            get(agent_session_get).delete(agent_session_delete),
        )
        .route("/v1/agent/sessions/{id}/chat", post(agent_chat))
        .route("/v1/agent/sessions/{id}/cancel", post(agent_cancel))
        .route("/v1/agent/sessions/{id}/tools", post(agent_direct_tool))
        .route(
            "/v1/agent/sessions/{id}/approvals/{approvalId}",
            post(agent_approve),
        )
        .route("/v1/agent/sessions/{id}/changes", get(agent_changes))
        .route("/v1/agent/sessions/{id}/revert", post(agent_revert))
        .route("/v1/agent/skills", get(agent_skills_list).post(agent_skill_import))
        .route(
            "/v1/agent/skills/{alias}",
            get(agent_skill_get).delete(agent_skill_delete),
        )
        .route("/v1/agent/skills/{alias}/enabled", post(agent_skill_enabled))
        .route("/v1/agent/preferences", get(agent_preferences_get).put(agent_preferences_put))
        .route("/v1/agent/screen/status", get(agent_screen_status))
        .route("/v1/agent/screen/{action}", post(agent_screen_action))
        .fallback(not_found)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// `.NET JsonHelp.DotNetNumber`: integral doubles serialize without a
/// decimal point (`1761972245`, not `1761972245.0`).
fn dotnet_number(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
        Value::Number((v as i64).into())
    } else {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// Per-request logging context (`.NET Endpoints.LogInfo`): only `/v1/*`
/// requests are recorded. SSE handlers pass their `LogCtx` into the
/// background task, which logs when the stream ends.
struct LogCtx {
    state: Arc<AppState>,
    method: &'static str,
    path: String,
    model: Option<String>,
    key_id: Option<String>,
    key_name: Option<String>,
    account_id: Arc<Mutex<Option<String>>>,
    stream: bool,
    poll: bool,
    task_id: Option<String>,
    started: Instant,
}

impl LogCtx {
    fn new(
        state: &Arc<AppState>,
        method: &'static str,
        path: &str,
        caller: Option<&Caller>,
    ) -> Self {
        LogCtx {
            state: Arc::clone(state),
            method,
            path: path.to_string(),
            model: None,
            key_id: caller.map(|c| c.key_id.clone()),
            key_name: caller.map(|c| c.key_name.clone()),
            account_id: Arc::new(Mutex::new(None)),
            stream: false,
            poll: false,
            task_id: None,
            started: Instant::now(),
        }
    }

    fn log(&self, status: i32, error: Option<String>) {
        let account_id = self.account_id.lock().ok().and_then(|g| g.clone());
        let record = RequestRecord {
            ts: now_unix_f64(),
            method: self.method.to_string(),
            path: self.path.clone(),
            model: self.model.clone(),
            key_id: self.key_id.clone(),
            key_name: self.key_name.clone(),
            account_id,
            status_code: status,
            latency_ms: self.started.elapsed().as_millis() as i64,
            stream: self.stream,
            poll: self.poll,
            error,
            client_ip: None, // D-log-1: no ConnectInfo wiring (loopback-only).
            task_id: self.task_id.clone(),
        };
        let _ = self.state.requests.add(&record);
    }
}

/// OpenAI-style JSON success body.
fn json_ok(status: StatusCode, body: Value) -> (StatusCode, Json<Value>) {
    (status, Json(body))
}

/// Optional string field.
fn opt_str<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(|v| v.as_str())
}

/// `messages` must be a non-empty array of objects (`.NET Endpoints`).
fn messages_of(body: &Value) -> ApiResult<Vec<Value>> {
    match body.get("messages") {
        Some(Value::Array(items)) if !items.is_empty() => {
            for m in items {
                if !m.is_object() {
                    return Err(MuseError::invalid("messages must be an array of objects"));
                }
            }
            Ok(items.clone())
        }
        _ => Err(MuseError::invalid("messages must be a non-empty list")),
    }
}

/// Resolve the model for a kind, recording the canonical id for logging.
fn resolve_model(body: &Value, kind: &str, log: &mut LogCtx) -> ApiResult<ModelSpec> {
    let spec = ModelRegistry::resolve(opt_str(body, "model"), kind)?;
    log.model = Some(spec.id.to_string());
    Ok(spec)
}

/// Build a browser chat request from flattened messages (shared by
/// `/v1/chat/completions` and `/v1/responses`). `model` and `images` are
/// filled in by the caller after this returns.
fn build_chat_request(
    state: &AppState,
    messages: &[Value],
    body: &Value,
) -> ApiResult<BrowserChatRequest> {
    let flat = Prompt::flatten(messages)?;
    if flat.text.trim().is_empty() && flat.images.is_empty() {
        return Err(MuseError::invalid("messages contain no text or images"));
    }
    let local_tools = body
        .get("_muse_local_tools")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reply_language = opt_str(body, "_muse_reply_language").unwrap_or("English");
    let prompt = if local_tools && state.driver.name() == "browser" {
        Prompt::for_local_executor(messages, false, reply_language)?
    } else {
        flat.text.clone()
    };
    Ok(BrowserChatRequest {
        prompt,
        model: String::new(),
        images: Vec::new(),
        reply_language: reply_language.to_string(),
        conversation_hint: opt_str(body, "user").map(|s| s.to_string()),
        turns: Prompt::message_turns(messages)?,
        local_tools,
        image_batch_id: Prompt::image_batch_id(messages),
    })
}

/// Load image refs for a chat request (async; errors are `invalid`).
/// `load_image_refs` already returns `Vec<InputImage>`.
async fn chat_images(state: &AppState, refs: &[String]) -> ApiResult<Vec<InputImage>> {
    Ok(load_image_refs(refs, &state.http_client).await?)
}

/// Release a request lease (no-op on the anonymous path), applying any
/// refreshed session the driver reported back on success.
fn release_lease(state: &AppState, leased: &Option<StoreAccount>, outcome: ReleaseOutcome) {
    if let Some(a) = leased {
        let success = matches!(outcome, ReleaseOutcome::Success);
        release_account(&state.pool, &a.id, outcome);
        if success {
            apply_refresh(&state.pool, &state.driver, &a.id);
        }
    }
}

/// Turn a `mpsc::UnboundedReceiver` into a `Stream` for `Sse::new`.
fn channel_stream<T: Send + 'static>(
    rx: mpsc::UnboundedReceiver<T>,
) -> impl futures_util::Stream<Item = T> + Send {
    futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

/// SSE response with the `.NET` no-cache headers.
fn sse_response(
    stream: impl futures_util::Stream<Item = Result<Event, Infallible>> + Send + 'static,
) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    (headers, Sse::new(stream)).into_response()
}

/// The api-key / admin-key from settings, for the auth helpers.
fn api_key_of(state: &AppState) -> String {
    state.settings.api_key.clone()
}
fn admin_key_of(state: &AppState) -> String {
    state.settings.effective_admin_key().to_string()
}

fn api_auth(state: &AppState, headers: &HeaderMap) -> ApiResult<Caller> {
    require_api(headers, &admin_key_of(state), &api_key_of(state), &state.keys)
}

fn admin_auth(state: &AppState, headers: &HeaderMap) -> ApiResult<Caller> {
    require_admin(headers, &admin_key_of(state))
}

fn agent_auth(state: &AppState, headers: &HeaderMap) -> ApiResult<Caller> {
    require_agent_api(headers, &admin_key_of(state), &api_key_of(state), &state.keys)
}

/// 404 for unknown routes (`.NET` returns an OpenAI-style error body).
async fn not_found() -> MuseError {
    MuseError::not_found("route not found")
}

// ---------------------------------------------------------------------------
// Public: health / ready / media / dashboard
// ---------------------------------------------------------------------------

async fn healthz() -> Json<Value> {
    Json(json!({"ok": true, "version": "0.9.1"}))
}

async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    let health = state.driver.health().await.unwrap_or_else(|e| {
        json!({"driver": state.driver.name(), "ok": false, "error": e.message})
    });
    let ready = health
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        && (state.pool_stats()["available"].as_u64().unwrap_or(0) > 0
            || !state.driver.requires_account());
    let body = json!({"ok": ready, "driver": health, "accounts": state.pool_stats()});
    if ready {
        (StatusCode::OK, Json(body)).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
    }
}

async fn dashboard() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DASHBOARD_HTML,
    )
        .into_response()
}

async fn media_get(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let path = state.media.path_of(&name)?;
    let mime = MediaStore::mime_of(&name).to_string();
    let data = tokio::fs::read(&path)
        .await
        .map_err(|e| MuseError::internal(e.to_string()))?;
    let total = data.len() as u64;
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert("cache-control", HeaderValue::from_static("public, max-age=86400"));
    resp_headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")));
    // Single-range support (`.NET` `Results.File` range handling).
    if let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if let Some(spec) = range.strip_prefix("bytes=") {
            let mut parts = spec.splitn(2, '-');
            let start: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let end: u64 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(total.saturating_sub(1));
            if start < total && end >= start && end < total {
                let slice = data[start as usize..=end as usize].to_vec();
                resp_headers.insert(
                    "content-range",
                    HeaderValue::from_str(&format!("bytes {start}-{end}/{total}"))
                        .unwrap_or_else(|_| HeaderValue::from_static("bytes 0-0/0")),
                );
                resp_headers.insert(
                    header::CONTENT_LENGTH,
                    HeaderValue::from_str(&slice.len().to_string())
                        .unwrap_or_else(|_| HeaderValue::from_static("0")),
                );
                return Ok((StatusCode::PARTIAL_CONTENT, resp_headers, slice).into_response());
            }
        }
    }
    resp_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&total.to_string()).unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    Ok((StatusCode::OK, resp_headers, data).into_response())
}

// ---------------------------------------------------------------------------
// OpenAI-compatible: models / chat / responses
// ---------------------------------------------------------------------------

async fn list_models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = api_auth(&state, &headers)?;
    let created = now_unix();
    let mut data: Vec<Value> = ModelRegistry::models()
        .iter()
        .map(|m| {
            json!({"id": m.id, "object": "model", "created": created, "owned_by": m.owned_by, "description": m.description, "kind": m.kind})
        })
        .collect();
    for (alias, target) in ModelRegistry::aliases() {
        data.push(json!({"id": alias, "object": "model", "created": created, "owned_by": "muse", "alias_of": target}));
    }
    Ok(Json(json!({"object": "list", "data": data})))
}

/// Wait for the next delta, emitting `: keepalive` comments while waiting
/// when a heartbeat interval is configured (`.NET` `FirstDelta`).
/// Returns `None` when the SSE sink is gone (stop waiting) or the channel
/// closed; `Some(None)` is never returned — channel close yields `Some`
/// of `None`... see callers: this returns the raw `Option<Result<..>>`
/// from the receiver, where `None` means the channel closed.
async fn next_delta_with_heartbeat(
    rx: &mut mpsc::UnboundedReceiver<Result<String, CoreError>>,
    heartbeat_secs: f64,
    tx: &mpsc::UnboundedSender<Result<Event, Infallible>>,
) -> Option<Result<String, CoreError>> {
    loop {
        if heartbeat_secs > 0.0 {
            tokio::select! {
                item = rx.recv() => return item,
                _ = tokio::time::sleep(std::time::Duration::from_secs_f64(heartbeat_secs)) => {
                    if tx.send(Ok(Event::default().comment("keepalive"))).is_err() {
                        return None;
                    }
                }
            }
        } else {
            return rx.recv().await;
        }
    }
}

async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let caller = api_auth(&state, &headers)?;
    let mut log = LogCtx::new(&state, "POST", "/v1/chat/completions", Some(&caller));
    let messages = messages_of(&body)?;
    let spec = resolve_model(&body, "chat", &mut log)?;
    let flat = Prompt::flatten(&messages)?;
    let mut req = build_chat_request(&state, &messages, &body)?;
    req.model = spec.id.to_string();
    req.images = chat_images(&state, &flat.images).await?;
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    log.stream = stream;
    let hint = opt_str(&body, "user").map(|s| s.to_string());
    let account_out = Arc::clone(&log.account_id);
    let mut rx = spawn_browser_chat(Arc::clone(&state), req, hint, "chat", account_out);
    if !stream {
        let mut text = String::new();
        while let Some(item) = rx.recv().await {
            match item {
                Ok(delta) => text.push_str(&delta),
                Err(e) => {
                    let status = e.status_code as i32;
                    let code = e.code.clone();
                    log.log(status, Some(code));
                    return Err(MuseError::from(e));
                }
            }
        }
        let prompt_tokens = approx_tokens(&flat.text);
        let completion_tokens = approx_tokens(&text);
        let resp = json!({
            "id": new_id24("chatcmpl-"),
            "object": "chat.completion",
            "created": now_unix(),
            "model": spec.id,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens, "total_tokens": prompt_tokens + completion_tokens},
        });
        log.log(200, None);
        return Ok((StatusCode::OK, Json(resp)).into_response());
    }
    // Streaming: drive the browser stream in a task, translate deltas to
    // OpenAI chunks, log when the stream ends.
    let (tx, sse_rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let heartbeat = state.settings.stream_heartbeat_interval;
    let include_usage = body
        .get("stream_options")
        .and_then(|o| o.get("include_usage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let model_id = spec.id.to_string();
    let prompt_tokens = approx_tokens(&flat.text);
    tokio::spawn(async move {
        let id = new_id24("chatcmpl-");
        let created = now_unix();
        let mut completion_tokens: i64 = 0;
        let mut first = true;
        loop {
            let item = if first {
                next_delta_with_heartbeat(&mut rx, heartbeat, &tx).await
            } else {
                rx.recv().await
            };
            match item {
                None => break, // channel closed = Done
                Some(Err(e)) => {
                    let status = e.status_code as i32;
                    let code = e.code.clone();
                    let _ = tx.send(Ok(Event::default()
                        .json_data(e.to_body())
                        .unwrap_or_else(|_| Event::default())));
                    let _ = tx.send(Ok(Event::default().data("[DONE]")));
                    log.log(status, Some(code));
                    return;
                }
                Some(Ok(delta)) => {
                    completion_tokens += approx_tokens(&delta);
                    let chunk = if first {
                        first = false;
                        json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model_id, "choices": [{"index": 0, "delta": {"role": "assistant", "content": delta}, "finish_reason": null}]})
                    } else {
                        json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model_id, "choices": [{"index": 0, "delta": {"content": delta}, "finish_reason": null}]})
                    };
                    if tx
                        .send(Ok(Event::default().json_data(chunk).unwrap_or_else(|_| Event::default())))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
        let final_chunk = json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model_id, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]});
        if tx
            .send(Ok(Event::default().json_data(final_chunk).unwrap_or_else(|_| Event::default())))
            .is_err()
        {
            return;
        }
        if include_usage {
            let usage_chunk = json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model_id, "choices": [], "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens, "total_tokens": prompt_tokens + completion_tokens}});
            let _ = tx.send(Ok(Event::default().json_data(usage_chunk).unwrap_or_else(|_| Event::default())));
        }
        let _ = tx.send(Ok(Event::default().data("[DONE]")));
        log.log(200, None);
    });
    Ok(sse_response(channel_stream(sse_rx)))
}

/// `.NET ResponsesToMessages`: `instructions` becomes a system message;
/// `input` (string or array of items) becomes messages. Function-call item
/// types are rejected — muse.ai has no function calling.
fn responses_messages(body: &Value) -> ApiResult<Vec<Value>> {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(instructions) = opt_str(body, "instructions") {
        if !instructions.trim().is_empty() {
            messages.push(json!({"role": "system", "content": instructions}));
        }
    }
    match body.get("input") {
        Some(Value::String(s)) => {
            messages.push(json!({"role": "user", "content": s}));
        }
        Some(Value::Array(items)) => {
            for item in items {
                match item {
                    Value::String(s) => {
                        messages.push(json!({"role": "user", "content": s}));
                    }
                    Value::Object(_) => {
                        if let Some(t) = item.get("type").and_then(|v| v.as_str()) {
                            match t {
                                "function_call" | "function_call_output" | "reasoning"
                                | "computer_call" | "file_search_call" | "web_search_call" => {
                                    return Err(MuseError::invalid(
                                        "muse.ai has no function calling",
                                    ));
                                }
                                "message" => {
                                    let role = item
                                        .get("role")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("user");
                                    let mut text = String::new();
                                    if let Some(Value::Array(parts)) = item.get("content") {
                                        for part in parts {
                                            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                                                if !text.is_empty() {
                                                    text.push('\n');
                                                }
                                                text.push_str(t);
                                            }
                                        }
                                    }
                                    messages.push(json!({"role": role, "content": text}));
                                }
                                _ => {
                                    return Err(MuseError::invalid(format!(
                                        "unsupported response input item type '{t}'"
                                    )));
                                }
                            }
                        } else if item.get("role").is_some() {
                            messages.push(item.clone());
                        } else {
                            return Err(MuseError::invalid(
                                "unsupported response input item",
                            ));
                        }
                    }
                    _ => {
                        return Err(MuseError::invalid("unsupported response input item"));
                    }
                }
            }
        }
        _ => {}
    }
    if messages.is_empty() {
        return Err(MuseError::invalid("input must not be empty"));
    }
    Ok(messages)
}

fn response_object(
    resp_id: &str,
    msg_id: &str,
    model: &str,
    status: &str,
    text: &str,
    prompt_tokens: i64,
    completion_tokens: i64,
) -> Value {
    json!({
        "id": resp_id,
        "object": "response",
        "created_at": now_unix(),
        "model": model,
        "status": status,
        "output": [{"id": msg_id, "type": "message", "role": "assistant", "status": if status == "completed" { "completed" } else { "in_progress" }, "content": [{"type": "output_text", "text": text, "annotations": []}]}],
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "tools": [],
        "usage": {"input_tokens": prompt_tokens, "output_tokens": completion_tokens, "total_tokens": prompt_tokens + completion_tokens},
    })
}

async fn responses(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let caller = api_auth(&state, &headers)?;
    let mut log = LogCtx::new(&state, "POST", "/v1/responses", Some(&caller));
    let messages = responses_messages(&body)?;
    let spec = resolve_model(&body, "chat", &mut log)?;
    let flat = Prompt::flatten(&messages)?;
    let mut req = build_chat_request(&state, &messages, &body)?;
    req.model = spec.id.to_string();
    req.images = chat_images(&state, &flat.images).await?;
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    log.stream = stream;
    let hint = opt_str(&body, "user").map(|s| s.to_string());
    let account_out = Arc::clone(&log.account_id);
    let mut rx = spawn_browser_chat(Arc::clone(&state), req, hint, "chat", account_out);
    let prompt_tokens = approx_tokens(&flat.text);
    let resp_id = new_id24("resp_");
    let msg_id = new_id24("msg_");
    if !stream {
        let mut text = String::new();
        while let Some(item) = rx.recv().await {
            match item {
                Ok(delta) => text.push_str(&delta),
                Err(e) => {
                    let status = e.status_code as i32;
                    let code = e.code.clone();
                    log.log(status, Some(code));
                    return Err(MuseError::from(e));
                }
            }
        }
        let completion_tokens = approx_tokens(&text);
        let resp = response_object(
            &resp_id,
            &msg_id,
            spec.id,
            "completed",
            &text,
            prompt_tokens,
            completion_tokens,
        );
        log.log(200, None);
        return Ok((StatusCode::OK, Json(resp)).into_response());
    }
    // Streaming: named events with ascending sequence numbers, no [DONE].
    let (tx, sse_rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let heartbeat = state.settings.stream_heartbeat_interval;
    let model_id = spec.id.to_string();
    tokio::spawn(async move {
        let mut seq: i64 = 0;
        let mut next_seq = || {
            let s = seq;
            seq += 1;
            s
        };
        let send = |tx: &mpsc::UnboundedSender<Result<Event, Infallible>>, v: Value| -> bool {
            tx.send(Ok(Event::default()
                .event(v.get("type").and_then(|t| t.as_str()).unwrap_or("message"))
                .json_data(v)
                .unwrap_or_else(|_| Event::default())))
            .is_ok()
        };
        let failed = |tx: &mpsc::UnboundedSender<Result<Event, Infallible>>, e: &CoreError, seq: i64| {
            let v = json!({"type": "response.failed", "sequence_number": seq, "response": {"id": resp_id, "object": "response", "status": "failed", "error": {"message": e.message, "type": e.error_type, "code": e.code}}});
            let _ = tx.send(Ok(Event::default()
                .event("response.failed")
                .json_data(v)
                .unwrap_or_else(|_| Event::default())));
        };
        // Wait for the first delta (with heartbeat); the preamble is only
        // emitted once we know the model actually answered.
        let first = next_delta_with_heartbeat(&mut rx, heartbeat, &tx).await;
        let first_delta = match first {
            None => return, // sink gone
            Some(Err(e)) => {
                let status = e.status_code as i32;
                let code = e.code.clone();
                failed(&tx, &e, next_seq());
                log.log(status, Some(code));
                return;
            }
            Some(Ok(d)) => d,
        };
        let mut text = first_delta.clone();
        let base = response_object(&resp_id, &msg_id, &model_id, "in_progress", "", prompt_tokens, 0);
        if !send(&tx, json!({"type": "response.created", "sequence_number": next_seq(), "response": base.clone()})) { return; }
        if !send(&tx, json!({"type": "response.in_progress", "sequence_number": next_seq(), "response": base})) { return; }
        if !send(&tx, json!({"type": "response.output_item.added", "sequence_number": next_seq(), "output_index": 0, "item": {"id": msg_id, "type": "message", "role": "assistant", "status": "in_progress", "content": []}})) { return; }
        if !send(&tx, json!({"type": "response.content_part.added", "sequence_number": next_seq(), "item_id": msg_id, "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}})) { return; }
        if !send(&tx, json!({"type": "response.output_text.delta", "sequence_number": next_seq(), "item_id": msg_id, "output_index": 0, "content_index": 0, "delta": first_delta})) { return; }
        loop {
            match rx.recv().await {
                None => break,
                Some(Err(e)) => {
                    let status = e.status_code as i32;
                    let code = e.code.clone();
                    failed(&tx, &e, next_seq());
                    log.log(status, Some(code));
                    return;
                }
                Some(Ok(delta)) => {
                    text.push_str(&delta);
                    if !send(&tx, json!({"type": "response.output_text.delta", "sequence_number": next_seq(), "item_id": msg_id, "output_index": 0, "content_index": 0, "delta": delta})) { return; }
                }
            }
        }
        let completion_tokens = approx_tokens(&text);
        if !send(&tx, json!({"type": "response.output_text.done", "sequence_number": next_seq(), "item_id": msg_id, "output_index": 0, "content_index": 0, "text": text})) { return; }
        if !send(&tx, json!({"type": "response.content_part.done", "sequence_number": next_seq(), "item_id": msg_id, "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": text, "annotations": []}})) { return; }
        if !send(&tx, json!({"type": "response.output_item.done", "sequence_number": next_seq(), "output_index": 0, "item": {"id": msg_id, "type": "message", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": text, "annotations": []}]}})) { return; }
        let completed = response_object(&resp_id, &msg_id, &model_id, "completed", &text, prompt_tokens, completion_tokens);
        if !send(&tx, json!({"type": "response.completed", "sequence_number": next_seq(), "response": completed})) { return; }
        log.log(200, None);
    });
    Ok(sse_response(channel_stream(sse_rx)))
}

// ---------------------------------------------------------------------------
// Images / videos
// ---------------------------------------------------------------------------

fn task_json(t: &BgTask) -> Value {
    json!({
        "id": t.id,
        "kind": t.kind,
        "status": t.status,
        "progress": t.progress,
        "created_at": dotnet_number(t.created_at),
        "updated_at": dotnet_number(t.updated_at),
        "request": t.request,
        "result": t.result,
        "error": t.error,
    })
}

fn image_task_view(t: &BgTask) -> Value {
    json!({
        "id": t.id,
        "object": "image.task",
        "status": t.status,
        "progress": t.progress,
        "created_at": t.created_at as i64,
        "model": t.request.get("model").cloned().unwrap_or(Value::Null),
        "result": t.result,
        "error": t.error,
    })
}

fn video_task_view(t: &BgTask) -> Value {
    json!({
        "id": t.id,
        "object": "video",
        "status": t.status,
        "progress": t.progress,
        "created_at": t.created_at as i64,
        "model": t.request.get("model").cloned().unwrap_or(Value::Null),
        "result": t.result,
        "error": t.error,
    })
}

/// `n` must be 1..=4 (`.NET`: `JsonHelp.Int(body["n"], present ? -1 : 1)`).
fn image_count(body: &Value) -> ApiResult<i64> {
    let n = match body.get("n") {
        None => 1,
        Some(v) => v.as_i64().unwrap_or(-1),
    };
    if !(1..=4).contains(&n) {
        return Err(MuseError::invalid("n must be between 1 and 4"));
    }
    Ok(n)
}

/// Reference images for generations/edits: string or array of strings,
/// at most 4, each non-empty (`.NET Endpoints`).
fn image_refs(body: &Value) -> ApiResult<Vec<String>> {
    let mut refs: Vec<String> = Vec::new();
    match body.get("image") {
        None => {}
        Some(Value::String(s)) => refs.push(s.clone()),
        Some(Value::Array(items)) => {
            for item in items {
                match item {
                    Value::String(s) => refs.push(s.clone()),
                    _ => return Err(MuseError::invalid("reference images must be strings")),
                }
            }
        }
        Some(_) => return Err(MuseError::invalid("image must be a string or array of strings")),
    }
    if refs.len() > 4 {
        return Err(MuseError::invalid("at most 4 reference images"));
    }
    for r in &refs {
        if r.trim().is_empty() {
            return Err(MuseError::invalid("reference images must be non-empty strings"));
        }
    }
    Ok(refs)
}

/// One synchronous image generation: acquire, generate, save to media,
/// release. Returns the `data` array entries for the response.
async fn run_image_generation(
    state: &Arc<AppState>,
    model: &str,
    prompt: &str,
    size: Option<String>,
    n: i64,
    refs: &[String],
    response_format: &str,
    account_out: Arc<Mutex<Option<String>>>,
) -> ApiResult<Vec<Value>> {
    let (store_account, browser_account) =
        acquire_for_request(&state.pool, state.driver.requires_account(), None, Some("image"))
            .await?;
    *account_out.lock().unwrap() = store_account.as_ref().map(|a| a.id.clone());
    let loaded = match chat_images(state, refs).await {
        Ok(l) => l,
        Err(e) => {
            release_lease(state, &store_account, ReleaseOutcome::Cancelled);
            return Err(e);
        }
    };
    let result = state
        .driver
        .generate_image(
            browser_account.clone(),
            ImageRequest {
                prompt: prompt.to_string(),
                model: model.to_string(),
                size,
                n: n as usize,
                references: loaded,
            },
        )
        .await;
    let result = match result {
        Ok(r) => {
            release_lease(state, &store_account, ReleaseOutcome::Success);
            r
        }
        Err(e) => {
            release_lease(state, &store_account, failure_outcome(&e));
            return Err(MuseError::from(e));
        }
    };
    let mut data: Vec<Value> = Vec::new();
    for media in result {
        let name = state.media.save(&media.data, &media.mime, "img")?;
        if response_format == "b64_json" {
            use base64::Engine;
            data.push(json!({
                "revised_prompt": media.revised_prompt,
                "b64_json": base64::engine::general_purpose::STANDARD.encode(&media.data),
            }));
        } else {
            let base = state.settings.public_base.trim_end_matches('/');
            data.push(json!({
                "revised_prompt": media.revised_prompt,
                "url": format!("{base}/v1/media/{name}"),
            }));
        }
    }
    Ok(data)
}

/// Spawn an image/video background runner (D-img-1). The runner marks the
/// task running, runs the driver call, saves media, then marks succeeded or
/// failed — unless the task was cancelled meanwhile, in which case it leaves
/// the `cancelled` record alone.
fn spawn_bg_task(
    state: Arc<AppState>,
    task_id: String,
    _kind: &'static str,
    run: impl FnOnce(Arc<AppState>, Arc<AtomicBool>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, CoreError>> + Send>> + Send + 'static,
) {
    let flag = Arc::new(AtomicBool::new(false));
    {
        let mut cancels = state.task_cancels.lock().unwrap();
        cancels.insert(task_id.clone(), Arc::clone(&flag));
    }
    tokio::spawn(async move {
        {
            if let Ok(mut tasks) = state.tasks.lock() {
                let _ = tasks.set_running(&task_id);
            }
        }
        let outcome = run(Arc::clone(&state), Arc::clone(&flag)).await;
        let cancelled = flag.load(Ordering::SeqCst);
        {
            let mut cancels = state.task_cancels.lock().unwrap();
            cancels.remove(&task_id);
        }
        if cancelled {
            return;
        }
        if let Ok(mut tasks) = state.tasks.lock() {
            match outcome {
                Ok(result) => {
                    let _ = tasks.set_progress(&task_id, 99);
                    let _ = tasks
                        .set_succeeded(&task_id, result.as_object().cloned().unwrap_or_default());
                }
                Err(e) => {
                    let _ = tasks.set_failed(&task_id, &e.message, &e.code);
                }
            }
        }
    });
}

async fn image_generations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let caller = api_auth(&state, &headers)?;
    let mut log = LogCtx::new(&state, "POST", "/v1/images/generations", Some(&caller));
    let prompt = opt_str(&body, "prompt").unwrap_or("");
    if prompt.trim().is_empty() {
        return Err(MuseError::invalid("prompt must not be empty"));
    }
    if let Some(bg) = opt_str(&body, "background") {
        if bg == "transparent" {
            return Err(MuseError::not_implemented("transparent background is not supported"));
        }
    }
    let spec = resolve_model(&body, "image", &mut log)?;
    let n = image_count(&body)?;
    let refs = image_refs(&body)?;
    let response_format = opt_str(&body, "response_format").unwrap_or("url");
    if response_format != "url" && response_format != "b64_json" {
        return Err(MuseError::invalid("response_format must be 'url' or 'b64_json'"));
    }
    let is_async = body.get("async").and_then(|v| v.as_bool()).unwrap_or(false);
    let size = opt_str(&body, "size").map(|s| s.to_string());
    let account_out = Arc::clone(&log.account_id);
    if !is_async {
        let data = run_image_generation(
            &state,
            spec.id,
            prompt,
            size,
            n,
            &refs,
            response_format,
            account_out,
        )
        .await
        .map_err(|e| {
            let status = e.status().as_u16() as i32;
            log.log(status, Some(e.0.code.clone()));
            e
        })?;
        log.log(200, None);
        return Ok((StatusCode::OK, Json(json!({"created": now_unix(), "data": data}))).into_response());
    }
    // Async: create the task, then run it in the background.
    let request = json!({
        "model": spec.id,
        "prompt": prompt,
        "size": size,
        "n": n,
        "references": refs.len(),
        "response_format": response_format,
    });
    let task = {
        let mut tasks = state.tasks.lock().map_err(|_| MuseError::internal("task store poisoned"))?;
        tasks.create("image", request.as_object().cloned().unwrap_or_default())?
    };
    log.task_id = Some(task.id.clone());
    let task_id = task.id.clone();
    let prompt_owned = prompt.to_string();
    let model_owned = spec.id.to_string();
    let format_owned = response_format.to_string();
    let refs_owned = refs.clone();
    spawn_bg_task(Arc::clone(&state), task_id.clone(), "image", move |st, flag| {
        Box::pin(async move {
            if flag.load(Ordering::SeqCst) {
                return Err(CoreError::invalid("cancelled"));
            }
            let account_out = Arc::new(Mutex::new(None));
            let data = run_image_generation(
                &st,
                &model_owned,
                &prompt_owned,
                size,
                n,
                &refs_owned,
                &format_owned,
                account_out,
            )
            .await
            .map_err(|e| e.0)?;
            Ok(json!({"data": data}))
        })
    });
    log.log(200, None);
    Ok((StatusCode::OK, Json(image_task_view(&task))).into_response())
}

async fn image_task_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let caller = api_auth(&state, &headers)?;
    let mut log = LogCtx::new(&state, "GET", "/v1/images/generations/{taskId}", Some(&caller));
    log.task_id = Some(task_id.clone());
    log.poll = true;
    let task = {
        let tasks = state.tasks.lock().map_err(|_| MuseError::internal("task store poisoned"))?;
        tasks.get(&task_id)
    };
    match task {
        Some(t) if t.kind == "image" => {
            log.log(200, None);
            Ok(Json(image_task_view(&t)))
        }
        _ => {
            log.log(404, Some("not_found".to_string()));
            Err(MuseError::not_found(format!("image task '{task_id}' not found")))
        }
    }
}

async fn image_edits(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    let caller = api_auth(&state, &headers)?;
    let mut log = LogCtx::new(&state, "POST", "/v1/images/edits", Some(&caller));
    let mut prompt = String::new();
    let mut model_name: Option<String> = None;
    let mut n: i64 = 1;
    let mut size: Option<String> = None;
    let mut response_format = "url".to_string();
    let mut refs: Vec<(Vec<u8>, String)> = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(|e| MuseError::invalid(e.to_string()))? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "prompt" => {
                prompt = field.text().await.map_err(|e| MuseError::invalid(e.to_string()))?;
            }
            "model" => {
                model_name = Some(field.text().await.map_err(|e| MuseError::invalid(e.to_string()))?);
            }
            "n" => {
                let raw = field.text().await.map_err(|e| MuseError::invalid(e.to_string()))?;
                n = raw.parse().unwrap_or(-1);
            }
            "size" => {
                size = Some(field.text().await.map_err(|e| MuseError::invalid(e.to_string()))?);
            }
            "response_format" => {
                response_format = field.text().await.map_err(|e| MuseError::invalid(e.to_string()))?;
            }
            "mask" => {
                return Err(MuseError::invalid("mask is not supported"));
            }
            "image" | "image[]" => {
                if refs.len() >= 4 {
                    return Err(MuseError::invalid("at most 4 reference images"));
                }
                let mime = field
                    .content_type()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "image/png".to_string());
                let bytes = field.bytes().await.map_err(|e| MuseError::invalid(e.to_string()))?;
                if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
                    return Err(MuseError::invalid("image must be non-empty and at most 20 MiB"));
                }
                refs.push((bytes.to_vec(), mime));
            }
            _ => {}
        }
    }
    if prompt.trim().is_empty() {
        return Err(MuseError::invalid("prompt must not be empty"));
    }
    if !(1..=4).contains(&n) {
        return Err(MuseError::invalid("n must be between 1 and 4"));
    }
    if refs.is_empty() {
        return Err(MuseError::invalid("image must not be empty"));
    }
    if response_format != "url" && response_format != "b64_json" {
        return Err(MuseError::invalid("response_format must be 'url' or 'b64_json'"));
    }
    let body = json!({"model": model_name});
    let spec = resolve_model(&body, "image", &mut log)?;
    // Multipart files are already decoded bytes: run the generation with
    // the references inline (no URL/base64 loading step).
    let (store_account, browser_account) =
        acquire_for_request(&state.pool, state.driver.requires_account(), None, Some("image"))
            .await?;
    let account_out = Arc::clone(&log.account_id);
    *account_out.lock().unwrap() = store_account.as_ref().map(|a| a.id.clone());
    let images: Vec<InputImage> = refs
        .into_iter()
        .map(|(data, mime)| InputImage { data, mime })
        .collect();
    let result = state
        .driver
        .generate_image(
            browser_account.clone(),
            ImageRequest {
                prompt: prompt.clone(),
                model: spec.id.to_string(),
                size,
                n: n as usize,
                references: images,
            },
        )
        .await;
    let result = match result {
        Ok(r) => {
            release_lease(&state, &store_account, ReleaseOutcome::Success);
            r
        }
        Err(e) => {
            release_lease(&state, &store_account, failure_outcome(&e));
            let status = e.status as i32;
            let code = e.code.clone();
            log.log(status, Some(code));
            return Err(MuseError::from(e));
        }
    };
    let mut data: Vec<Value> = Vec::new();
    for media in result {
        let name = state.media.save(&media.data, &media.mime, "img")?;
        if response_format == "b64_json" {
            use base64::Engine;
            data.push(json!({
                "revised_prompt": media.revised_prompt,
                "b64_json": base64::engine::general_purpose::STANDARD.encode(&media.data),
            }));
        } else {
            let base = state.settings.public_base.trim_end_matches('/');
            data.push(json!({
                "revised_prompt": media.revised_prompt,
                "url": format!("{base}/v1/media/{name}"),
            }));
        }
    }
    log.log(200, None);
    Ok((StatusCode::OK, Json(json!({"created": now_unix(), "data": data}))).into_response())
}

async fn video_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let caller = api_auth(&state, &headers)?;
    let mut log = LogCtx::new(&state, "POST", "/v1/videos", Some(&caller));
    let prompt = opt_str(&body, "prompt").unwrap_or("");
    if prompt.trim().is_empty() {
        return Err(MuseError::invalid("prompt must not be empty"));
    }
    let spec = resolve_model(&body, "video", &mut log)?;
    let duration: Option<u32> = body
        .get("duration")
        .or_else(|| body.get("seconds"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    if let Some(d) = duration {
        if d == 0 || d > 60 {
            return Err(MuseError::invalid("duration must be between 1 and 60"));
        }
    }
    let first_frame: Option<String> = body
        .get("image")
        .or_else(|| body.get("input_reference"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let request = json!({
        "model": spec.id,
        "prompt": prompt,
        "size": opt_str(&body, "size"),
        "duration": duration,
        "first_frame": first_frame.is_some(),
    });
    let task = {
        let mut tasks = state.tasks.lock().map_err(|_| MuseError::internal("task store poisoned"))?;
        tasks.create("video", request.as_object().cloned().unwrap_or_default())?
    };
    log.task_id = Some(task.id.clone());
    let task_id = task.id.clone();
    let prompt_owned = prompt.to_string();
    let model_owned = spec.id.to_string();
    let size_owned = opt_str(&body, "size").map(|s| s.to_string());
    spawn_bg_task(Arc::clone(&state), task_id.clone(), "video", move |st, flag| {
        Box::pin(async move {
            if flag.load(Ordering::SeqCst) {
                return Err(CoreError::invalid("cancelled"));
            }
            let (store_account, browser_account) =
                acquire_for_request(&st.pool, st.driver.requires_account(), None, Some("video"))
                    .await?;
            let first_frame = match &first_frame {
                Some(r) => Some(load_image_ref(r, &st.http_client).await?),
                None => None,
            };
            if flag.load(Ordering::SeqCst) {
                release_lease(&st, &store_account, ReleaseOutcome::Cancelled);
                return Err(CoreError::invalid("cancelled"));
            }
            let result = st
                .driver
                .generate_video(
                    browser_account.clone(),
                    VideoRequest {
                        prompt: prompt_owned,
                        model: model_owned,
                        size: size_owned,
                        duration,
                        first_frame,
                    },
                )
                .await;
            let result = match result {
                Ok(r) => {
                    release_lease(&st, &store_account, ReleaseOutcome::Success);
                    r
                }
                Err(e) => {
                    release_lease(&st, &store_account, failure_outcome(&e));
                    return Err(browser_to_core(e));
                }
            };
            let name = st
                .media
                .save(&result.data, &result.mime, "vid")
                .map_err(|e| CoreError::internal(e.to_string()))?;
            let base = st.settings.public_base.trim_end_matches('/');
            Ok(json!({
                "url": format!("{base}/v1/media/{name}"),
                "mime": result.mime,
                "width": result.width,
                "height": result.height,
            }))
        })
    });
    log.log(200, None);
    Ok(Json(video_task_view(&task)))
}

async fn video_task_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let caller = api_auth(&state, &headers)?;
    let mut log = LogCtx::new(&state, "GET", "/v1/videos/{taskId}", Some(&caller));
    log.task_id = Some(task_id.clone());
    log.poll = true;
    let task = {
        let tasks = state.tasks.lock().map_err(|_| MuseError::internal("task store poisoned"))?;
        tasks.get(&task_id)
    };
    match task {
        Some(t) if t.kind == "video" => {
            log.log(200, None);
            Ok(Json(video_task_view(&t)))
        }
        _ => {
            log.log(404, Some("not_found".to_string()));
            Err(MuseError::not_found(format!("video task '{task_id}' not found")))
        }
    }
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

async fn admin_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let health = state.driver.health().await.unwrap_or_else(|e| {
        json!({"driver": state.driver.name(), "ok": false, "error": e.message})
    });
    let caps = state.driver.capabilities();
    Ok(Json(json!({
        "version": "0.9.1",
        "driver": health,
        "capabilities": {
            "chat": caps.chat,
            "chat_images": caps.chat_images,
            "image": caps.image,
            "image_edit": caps.image_edit,
            "video": caps.video,
            "renew_session": caps.renew_session,
            "quota": caps.quota,
        },
        "accounts": state.pool_stats(),
    })))
}

/// Parse an account body (`.NET Endpoints.BuildAccount`): cookies as a
/// dict, a raw string, or an array of `{name, value, expirationDate}`.
fn build_store_account(body: &Value) -> ApiResult<StoreAccount> {
    let mut account = StoreAccount::new();
    if let Some(id) = opt_str(body, "id") {
        if !id.is_empty() {
            account.id = id.to_string();
        }
    }
    account.label = opt_str(body, "label").unwrap_or("").to_string();
    if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
        account.enabled = enabled;
    }
    let mut cookies: Map<String, Value> = Map::new();
    let mut expires: Map<String, Value> = Map::new();
    if let Some(c) = body.get("cookies") {
        match c {
            Value::Object(map) => {
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        cookies.insert(k.clone(), Value::String(s.to_string()));
                    }
                }
            }
            Value::String(raw) => {
                for pair in raw.split(';') {
                    let pair = pair.trim();
                    if pair.is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = pair.split_once('=') {
                        cookies.insert(k.trim().to_string(), Value::String(v.trim().to_string()));
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    let name = item.get("name").and_then(|v| v.as_str());
                    let value = item.get("value").and_then(|v| v.as_str());
                    if let (Some(n), Some(v)) = (name, value) {
                        cookies.insert(n.to_string(), Value::String(v.to_string()));
                        if let Some(exp) = item.get("expirationDate").and_then(|v| v.as_f64()) {
                            expires.insert(n.to_string(), Value::from(exp));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    account.cookies = cookies;
    account.cookie_expires = expires;
    if let Some(ce) = body.get("cookie_expires").and_then(|v| v.as_object()) {
        for (k, v) in ce {
            if let Some(f) = v.as_f64() {
                account.cookie_expires.insert(k.clone(), Value::from(f));
            }
        }
    }
    Ok(account)
}

async fn admin_accounts(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let accounts = {
        let pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store.all()
    };
    let data: Vec<Value> = accounts.iter().map(|a| a.public()).collect();
    Ok(Json(json!({"data": data})))
}

async fn admin_account_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let _caller = admin_auth(&state, &headers)?;
    let account = build_store_account(&body)?;
    let reuse = body.get("reuse_session").and_then(|v| v.as_bool()).unwrap_or(false);
    let saved = {
        let mut pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        if reuse {
            pool.store.upsert_session(account)?
        } else {
            pool.store.upsert(account)?
        }
    };
    let missing = missing_session_cookies(&saved.cookies);
    Ok(json_ok(StatusCode::OK, json!({"account": saved.public(), "missing_cookies": missing})))
}

async fn admin_accounts_import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let items = match body.get("accounts") {
        Some(Value::Array(items)) => items.clone(),
        _ => return Err(MuseError::invalid("accounts must be a list")),
    };
    let mut results: Vec<Value> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match build_store_account(item) {
            Ok(account) => {
                let reuse = item.get("reuse_session").and_then(|v| v.as_bool()).unwrap_or(false);
                let outcome = {
                    let mut pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
                    if reuse {
                        pool.store.upsert_session(account)
                    } else {
                        pool.store.upsert(account)
                    }
                };
                match outcome {
                    Ok(saved) => results.push(json!({"index": index, "ok": true, "account": saved.public()})),
                    Err(e) => results.push(json!({"index": index, "ok": false, "error": e.to_string(), "code": "invalid"})),
                }
            }
            Err(e) => results.push(json!({"index": index, "ok": false, "error": e.to_string(), "code": "invalid"})),
        }
    }
    Ok(Json(json!({"results": results})))
}

async fn admin_account_patch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let mut account = {
        let pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store
            .get(&id)
            .ok_or_else(|| MuseError::not_found(format!("account '{id}' not found")))?
    };
    let mut cookies_changed = false;
    if let Some(label) = opt_str(&body, "label") {
        account.label = label.to_string();
    }
    if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
        account.enabled = enabled;
    }
    if body.get("cookies").is_some() {
        let fresh = build_store_account(&body)?;
        account.cookies = fresh.cookies;
        account.cookie_expires = fresh.cookie_expires;
        account.status = AccountStatus::Active;
        account.last_error = None;
        cookies_changed = true;
    }
    if let Some(ce) = body.get("cookie_expires").and_then(|v| v.as_object()) {
        for (k, v) in ce {
            if let Some(f) = v.as_f64() {
                account.cookie_expires.insert(k.clone(), Value::from(f));
            }
        }
        cookies_changed = true;
    }
    let saved = {
        let mut pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store.upsert(account)?
    };
    if cookies_changed {
        let _ = state.driver.invalidate_account(&id).await;
    }
    Ok(Json(json!({"account": saved.public()})))
}

async fn admin_account_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let removed = {
        let mut pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store.remove(&id)?
    };
    if !removed {
        return Err(MuseError::not_found(format!("account '{id}' not found")));
    }
    // WinForms delete calls Driver.InvalidateAccount(id) (Endpoints.cs:751-757,
    // BrowserDriver.cs:892): drop the driver's tab/context + quota cache for
    // the removed account, so a deleted account cannot keep serving turns
    // from a stale session. No-op when the driver holds no session for it
    // (browser.rs invalidate_account always returns Ok); the result is
    // ignored like the PATCH handler does, so a driver hiccup cannot fail
    // a delete that already succeeded in the store.
    let _ = state.driver.invalidate_account(&id).await;
    Ok(Json(json!({"deleted": id})))
}

async fn admin_account_reset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let mut account = {
        let pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store
            .get(&id)
            .ok_or_else(|| MuseError::not_found(format!("account '{id}' not found")))?
    };
    account.status = AccountStatus::Active;
    account.cooldown_until = 0.0;
    account.last_error = None;
    account.meta.remove("kind_blocked_until");
    account.meta.remove("wake_blocked_until");
    let saved = {
        let mut pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store.upsert(account)?
    };
    Ok(Json(json!({"account": saved.public()})))
}

async fn admin_account_renew(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let account = {
        let pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store
            .get(&id)
            .ok_or_else(|| MuseError::not_found(format!("account '{id}' not found")))?
    };
    let browser_account = store_account_to_browser(&account);
    let info = state.driver.renew_session(browser_account).await?;
    let mut account = account;
    if info.ok {
        account.status = AccountStatus::Active;
        account.cooldown_until = 0.0;
        account.last_error = None;
    }
    for (k, v) in &info.cookies {
        account.cookies.insert(k.clone(), Value::String(v.clone()));
    }
    for (k, v) in &info.cookie_expires {
        account.cookie_expires.insert(k.clone(), Value::from(*v as f64));
    }
    for (k, v) in &info.meta {
        account.meta.insert(
            k.clone(),
            v.clone().map(Value::String).unwrap_or(Value::Null),
        );
    }
    let meta_json: Map<String, Value> = info
        .meta
        .iter()
        .map(|(k, v)| (k.clone(), v.clone().map(Value::String).unwrap_or(Value::Null)))
        .collect();
    let saved = {
        let mut pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.note_wake(&id, &info.meta);
        pool.store.upsert(account)?
    };
    Ok(Json(json!({"ok": info.ok, "account": saved.public(), "meta": meta_json})))
}

async fn admin_account_quota(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let mut account = {
        let pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store
            .get(&id)
            .ok_or_else(|| MuseError::not_found(format!("account '{id}' not found")))?
    };
    let browser_account = store_account_to_browser(&account);
    let snapshot = state.driver.quota(browser_account).await?;
    let mut quota: Map<String, Value> = snapshot.into_iter().collect();
    quota.insert("checked_at".to_string(), Value::from(now_unix_f64()));
    account.meta.insert("quota".to_string(), Value::Object(quota.clone()));
    {
        let mut pool = state.pool.lock().map_err(|_| MuseError::internal("account pool poisoned"))?;
        pool.store.upsert(account)?;
    }
    Ok(Json(json!({"account_id": id, "quota": quota})))
}

async fn admin_extension_zip(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    Err(MuseError::not_found("the browser extension is not bundled with this server"))
}

async fn admin_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    let tasks = {
        let tasks = state.tasks.lock().map_err(|_| MuseError::internal("task store poisoned"))?;
        tasks.list(limit)
    };
    let data: Vec<Value> = tasks.iter().map(task_json).collect();
    Ok(Json(json!({"data": data})))
}

async fn admin_task_cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let cancelled = {
        let mut tasks = state.tasks.lock().map_err(|_| MuseError::internal("task store poisoned"))?;
        tasks.cancel(&task_id)?
    };
    if cancelled {
        if let Some(flag) = state.task_cancels.lock().unwrap().get(&task_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
    Ok(Json(json!({"cancelled": cancelled})))
}

fn key_usage_for(state: &AppState, key_id: &str) -> Value {
    match state.requests.key_usage() {
        Ok(Value::Array(rows)) => rows
            .iter()
            .find(|r| r.get("key_id").and_then(|v| v.as_str()) == Some(key_id))
            .cloned()
            .unwrap_or(json!({"key_id": key_id, "total": 0, "requests_24h": 0, "errors_24h": 0, "last_request_at": Value::Null})),
        _ => json!({"key_id": key_id, "total": 0, "requests_24h": 0, "errors_24h": 0, "last_request_at": Value::Null}),
    }
}

async fn admin_keys(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let keys = {
        let keys = state.keys.lock().map_err(|_| MuseError::internal("key store poisoned"))?;
        keys.all()
    };
    let data: Vec<Value> = keys
        .iter()
        .map(|k| {
            let mut v = k.public();
            v["usage"] = key_usage_for(&state, &k.id);
            v
        })
        .collect();
    Ok(Json(json!({"data": data, "builtin": [
        {"id": "legacy", "usage": key_usage_for(&state, "legacy")},
        {"id": "admin", "usage": key_usage_for(&state, "admin")},
    ]})))
}

async fn admin_keys_usage(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let usage = state.requests.key_usage()?;
    Ok(Json(json!({"since": now_unix() - 86400, "data": usage})))
}

async fn admin_key_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let _caller = admin_auth(&state, &headers)?;
    let name = opt_str(&body, "name").unwrap_or("");
    if name.is_empty() || name.chars().count() > 100 {
        return Err(MuseError::invalid("name must be 1-100 characters"));
    }
    let note = opt_str(&body, "note").unwrap_or("");
    let (key, plaintext) = {
        let mut keys = state.keys.lock().map_err(|_| MuseError::internal("key store poisoned"))?;
        keys.create(name, note)?
    };
    Ok(json_ok(StatusCode::CREATED, json!({"key": key.public(), "api_key": plaintext})))
}

async fn admin_key_patch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let mut key = {
        let keys = state.keys.lock().map_err(|_| MuseError::internal("key store poisoned"))?;
        keys.get(&key_id)
            .ok_or_else(|| MuseError::not_found(format!("key '{key_id}' not found")))?
    };
    if let Some(name) = opt_str(&body, "name") {
        if name.is_empty() || name.chars().count() > 100 {
            return Err(MuseError::invalid("name must be 1-100 characters"));
        }
        key.name = name.to_string();
    }
    if let Some(note) = opt_str(&body, "note") {
        key.note = note.to_string();
    }
    if let Some(revoked) = body.get("revoked").and_then(|v| v.as_bool()) {
        key.revoked = revoked;
    }
    {
        let mut keys = state.keys.lock().map_err(|_| MuseError::internal("key store poisoned"))?;
        if !keys.update(&key)? {
            return Err(MuseError::not_found(format!("key '{key_id}' not found")));
        }
    }
    Ok(Json(json!({"key": key.public()})))
}

async fn admin_key_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let removed = {
        let mut keys = state.keys.lock().map_err(|_| MuseError::internal("key store poisoned"))?;
        keys.remove(&key_id)?
    };
    if !removed {
        return Err(MuseError::not_found(format!("key '{key_id}' not found")));
    }
    Ok(Json(json!({"deleted": key_id})))
}

async fn admin_requests(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let key_id = query.get("key_id").cloned();
    let account_id = query.get("account_id").cloned();
    let status = query.get("status").cloned();
    let path = query.get("path").cloned();
    let since = query.get("since").and_then(|v| v.parse::<f64>().ok());
    // D-log-2: hide polls only when the parameter is present and != "false".
    let hide_polls = query
        .get("hide_polls")
        .map(|v| v != "false")
        .unwrap_or(false);
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 500);
    let offset = query
        .get("offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let (rows, total) = state.requests.query(
        key_id.as_deref(),
        account_id.as_deref(),
        status.as_deref(),
        path.as_deref(),
        since,
        hide_polls,
        limit as i64,
        offset as i64,
    )?;
    let data: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "ts": dotnet_number(r.ts),
                "method": r.method,
                "path": r.path,
                "model": r.model,
                "key_id": r.key_id,
                "key_name": r.key_name,
                "account_id": r.account_id,
                "status_code": r.status_code,
                "latency_ms": r.latency_ms,
                "stream": r.stream,
                "poll": r.poll,
                "error": r.error,
                "client_ip": r.client_ip,
                "task_id": r.task_id,
            })
        })
        .collect();
    Ok(Json(json!({"data": data, "total": total})))
}

async fn admin_stats(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let _caller = admin_auth(&state, &headers)?;
    let window = query.get("window").map(|s| s.as_str()).unwrap_or("24h");
    let window = match window {
        "1h" | "24h" | "7d" => window,
        _ => "24h",
    };
    let mut stats = state.requests.stats(window)?;
    stats["now"] = Value::String(now_rfc3339());
    Ok(Json(stats))
}

async fn admin_metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Response> {
    let _caller = admin_auth(&state, &headers)?;
    let stats = state.requests.stats("24h").unwrap_or(json!({}));
    let mut lines: Vec<String> = Vec::new();
    lines.push("# HELP muse_gateway_info Gateway info".to_string());
    lines.push("# TYPE muse_gateway_info gauge".to_string());
    lines.push(format!("muse_gateway_info{{version=\"0.9.1\",driver=\"{}\"}} 1", state.driver.name()));
    let total = stats.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
    let errors = stats.get("errors").and_then(|v| v.as_i64()).unwrap_or(0);
    let server_errors = stats.get("server_errors").and_then(|v| v.as_i64()).unwrap_or(0);
    lines.push("# HELP requests_window_total Requests in window".to_string());
    lines.push("# TYPE requests_window_total counter".to_string());
    lines.push(format!("requests_window_total {total}"));
    lines.push(format!("requests_window_errors {errors}"));
    lines.push(format!("requests_window_server_errors {server_errors}"));
    if total > 0 {
        lines.push(format!("requests_window_error_rate {:.4}", errors as f64 / total as f64));
    }
    if let Some(p50) = stats.get("latency_ms").and_then(|v| v.get("p50")).and_then(|v| v.as_i64()) {
        lines.push(format!("request_latency_ms{{quantile=\"0.5\"}} {p50}"));
    }
    if let Some(p95) = stats.get("latency_ms").and_then(|v| v.get("p95")).and_then(|v| v.as_i64()) {
        lines.push(format!("request_latency_ms{{quantile=\"0.95\"}} {p95}"));
    }
    let mut by = |key: &str, label: &str, field: &str| {
        if let Some(Value::Array(rows)) = stats.get(key) {
            for row in rows {
                let name = row
                    .get(field)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                let count = row.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
                lines.push(format!("requests_by_{label}{{{label}=\"{name}\"}} {count}"));
            }
        }
    };
    by("by_status", "status", "status_code");
    by("by_key", "key", "key_id");
    by("by_account", "account", "account_id");
    by("by_model", "model", "model");
    let pool = state.pool_stats();
    for (k, v) in [
        ("total", "total"),
        ("available", "available"),
        ("invalid", "invalid"),
        ("inflight", "inflight"),
    ] {
        let n = pool.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        lines.push(format!("accounts{{state=\"{v}\"}} {n}"));
    }
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        lines.join("\n") + "\n",
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// One agent event as SSE JSON. `approval_required` events carry their
/// payload as a JSON string inside `text` (`.NET AgentSession.ConfirmAsync`
/// via `chat_turn::confirm`); the object is emitted directly, not wrapped
/// as a string.
fn agent_event_json(ev: &AgentEvent) -> Value {
    if ev.kind == "approval_required" {
        if let Ok(parsed) = serde_json::from_str::<Value>(&ev.text) {
            return parsed;
        }
    }
    ev.to_json()
}

struct TurnSink {
    tx: mpsc::UnboundedSender<AgentEvent>,
    events: Vec<AgentEvent>,
}

impl muse_agent::chat_turn::EventSink for TurnSink {
    fn emit(&mut self, event: AgentEvent) {
        self.events.push(event.clone());
        let _ = self.tx.send(event);
    }
}

async fn agent_session_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let _caller = agent_auth(&state, &headers)?;
    let workspace = opt_str(&body, "workspace").unwrap_or("");
    let shell = opt_str(&body, "shell");
    let model = opt_str(&body, "model");
    let reply_language = opt_str(&body, "reply_language");
    let turbo = body.get("turbo").and_then(|v| v.as_bool());
    let max_steps = body
        .get("max_steps")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let mut mgr = state.sessions.lock().await;
    let id = mgr.create(workspace, shell, model, reply_language, turbo, max_steps)?;
    let view = {
        let session = mgr.get_mut(&id)?;
        session
            .tools
            .set_screen(Some(Arc::clone(&state.screen_driver)));
        session.view()
    };
    let session_ref = mgr.get(&id)?;
    persist_agent_session(&state.chat_sessions, session_ref, &[]);
    Ok((StatusCode::CREATED, Json(view)))
}

async fn agent_sessions_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let mgr = state.sessions.lock().await;
    let mut data: Vec<Value> = mgr.all().iter().map(|s| s.view()).collect();
    let checked = state.checked_out.lock().unwrap();
    for entry in checked.values() {
        data.push(entry.view.clone());
    }
    drop(checked);
    data.sort_by(|a, b| {
        let ua = a.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
        let ub = b.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
        ub.cmp(ua)
    });
    Ok(Json(json!({"object": "list", "data": data})))
}

async fn agent_session_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let mgr = state.sessions.lock().await;
    if let Ok(session) = mgr.get(&id) {
        return Ok(Json(session.view()));
    }
    let checked = state.checked_out.lock().unwrap();
    if let Some(entry) = checked.get(&id) {
        return Ok(Json(entry.view.clone()));
    }
    Err(MuseError::not_found(format!("agent session '{id}' not found")))
}

async fn agent_session_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let is_checked_out = state
        .checked_out
        .lock()
        .unwrap()
        .contains_key(&id);
    if is_checked_out {
        let mut mgr = state.sessions.lock().await;
        mgr.cancel(&id);
        state
            .deleted_while_running
            .lock()
            .unwrap()
            .insert(id.clone());
        state.checked_out.lock().unwrap().remove(&id);
        return Ok(Json(json!({"deleted": true, "session_id": id})));
    }
    let mut mgr = state.sessions.lock().await;
    let session = mgr
        .get(&id)
        .map_err(|_| MuseError::not_found(format!("agent session '{id}' not found")))?;
    persist_agent_session(&state.chat_sessions, session, &[]);
    mgr.remove(&id).await?;
    Ok(Json(json!({"deleted": true, "session_id": id})))
}

async fn agent_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let _caller = agent_auth(&state, &headers)?;
    let prompt = opt_str(&body, "prompt").unwrap_or("").to_string();
    let images: Vec<String> = match body.get("images") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    };
    if prompt.trim().is_empty() && images.is_empty() {
        return Err(MuseError::invalid("Content must not be empty."));
    }
    if prompt.chars().count() > 32_000 {
        return Err(MuseError::invalid("prompt must be at most 32000 characters"));
    }
    if images.len() > 4 {
        return Err(MuseError::invalid("at most 4 images"));
    }
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(true);
    let reply_language = opt_str(&body, "reply_language").map(|s| s.to_string());
    let turbo = body.get("turbo").and_then(|v| v.as_bool());
    // Checkout: register the run token, take the session out of the
    // manager, and snapshot a running view for GET/list during the turn.
    let token = CancellationToken::new();
    let mut session = {
        let mut mgr = state.sessions.lock().await;
        if mgr.get(&id).is_err() && !state.checked_out.lock().unwrap().contains_key(&id) {
            return Err(MuseError::not_found(format!("agent session '{id}' not found")));
        }
        if state.checked_out.lock().unwrap().contains_key(&id) {
            return Err(MuseError::session_busy());
        }
        mgr.register_run_token(&id, token.clone());
        let mut session = mgr
            .take(&id)
            .map_err(|_| MuseError::not_found(format!("agent session '{id}' not found")))?;
        if session.title == "New conversation" && !prompt.trim().is_empty() {
            session.title = prompt.chars().take(60).collect();
        }
        let mut view = session.view();
        view["running"] = Value::Bool(true);
        let changes = session.changes_view();
        state
            .checked_out
            .lock()
            .unwrap()
            .insert(id.clone(), crate::state::CheckedOut { view, changes });
        session
    };
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let agent_driver = Arc::clone(&state.agent_driver);
    let state_for_task = Arc::clone(&state);
    let id_for_task = id.clone();
    let turn_join = tokio::spawn(async move {
        let mut sink = TurnSink {
            tx: event_tx,
            events: Vec::new(),
        };
        let outcome = run_turn(
            &mut session,
            &prompt,
            images,
            &*agent_driver,
            &mut sink,
            TurnOptions {
                interactive: stream,
                reply_language,
                turbo,
            },
            &token,
        )
        .await;
        let history = session.history.clone();
        let goal = session.view().get("goal").cloned().unwrap_or(Value::Null);
        persist_agent_session(&state_for_task.chat_sessions, &session, &sink.events);
        let deleted = state_for_task
            .deleted_while_running
            .lock()
            .unwrap()
            .contains(&id_for_task);
        state_for_task
            .checked_out
            .lock()
            .unwrap()
            .remove(&id_for_task);
        if deleted {
            state_for_task
                .deleted_while_running
                .lock()
                .unwrap()
                .remove(&id_for_task);
        } else {
            let mut mgr = state_for_task.sessions.lock().await;
            mgr.put_back(session);
        }
        (outcome, history, goal)
    });
    if !stream {
        let (outcome, history, goal) = turn_join
            .await
            .map_err(|e| MuseError::internal(format!("turn task failed: {e}")))?;
        return Ok((
            StatusCode::OK,
            Json(json!({
                "session_id": id,
                "ok": outcome.ok,
                "summary": outcome.summary,
                "status": outcome.status,
                "history": history,
                "goal": goal,
            })),
        )
            .into_response());
    }
    // Streaming: forward events, then the done event + [DONE] once the
    // turn task finishes.
    let (tx, sse_rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let session_id = id.clone();
    tokio::spawn(async move {
        let mut join = turn_join;
        let mut events_open = true;
        let mut rx = event_rx;
        loop {
            tokio::select! {
                biased;
                msg = rx.recv(), if events_open => {
                    match msg {
                        Some(ev) => {
                            let v = agent_event_json(&ev);
                            if tx.send(Ok(Event::default().json_data(v).unwrap_or_else(|_| Event::default()))).is_err() {
                                return;
                            }
                        }
                        None => {
                            events_open = false;
                        }
                    }
                }
                done = &mut join => {
                    let (outcome, _, _) = done.unwrap_or_else(|_| (
                        muse_agent::chat_turn::TurnOutcome::incomplete("turn aborted"),
                        Vec::new(),
                        Value::Null,
                    ));
                    let v = done_event(outcome.ok, &outcome.status, &outcome.summary, &session_id);
                    let _ = tx.send(Ok(Event::default().json_data(v).unwrap_or_else(|_| Event::default())));
                    let _ = tx.send(Ok(Event::default().data("[DONE]")));
                    return;
                }
            }
        }
    });
    Ok(sse_response(channel_stream(sse_rx)))
}

async fn agent_cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let exists = {
        let mgr = state.sessions.lock().await;
        mgr.get(&id).is_ok() || state.checked_out.lock().unwrap().contains_key(&id)
    };
    if !exists {
        return Err(MuseError::not_found(format!("agent session '{id}' not found")));
    }
    let mut mgr = state.sessions.lock().await;
    let cancelled = mgr.cancel(&id);
    Ok(Json(json!({"cancelled": cancelled})))
}

async fn agent_direct_tool(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let _caller = agent_auth(&state, &headers)?;
    let name = opt_str(&body, "name").unwrap_or("").to_string();
    if name.is_empty() {
        return Err(MuseError::invalid("tool name is required"));
    }
    let args: Map<String, Value> = match body.get("arguments") {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    let approved = body.get("approved").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut mgr = state.sessions.lock().await;
    if mgr.get(&id).is_err() {
        if state.checked_out.lock().unwrap().contains_key(&id) {
            return Err(MuseError::session_busy());
        }
        return Err(MuseError::not_found(format!("agent session '{id}' not found")));
    }
    let session = mgr
        .take(&id)
        .map_err(|_| MuseError::not_found(format!("agent session '{id}' not found")))?;
    drop(mgr);
    let call = validate_tool_call(&ToolCall::new(name, args));
    // E6: screen control is refused outright in read-only mode — an
    // approval cannot override it (the 403 below is final, not an
    // `approval_required` round-trip).
    if session.read_only && call.name.starts_with("computer_") {
        let mut mgr = state.sessions.lock().await;
        mgr.put_back(session);
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({"error": {"message": muse_agent::chat_turn::READ_ONLY_SCREEN_MESSAGE, "type": "forbidden", "code": "read_only"}})),
        )
            .into_response());
    }
    let token = CancellationToken::new();
    let prepared = match session.tools.prepare_for_agent(&call, &token).await {
        Ok(p) => p,
        Err(e) => {
            let mut mgr = state.sessions.lock().await;
            mgr.put_back(session);
            return Err(MuseError::from(e));
        }
    };
    if !direct_tool_may_run(&session, &call, approved) {
        let body403 = session.approval_required_body(&prepared);
        let mut mgr = state.sessions.lock().await;
        mgr.put_back(session);
        return Ok((StatusCode::FORBIDDEN, Json(body403)).into_response());
    }
    let result = match session.tools.execute_prepared(&prepared, &token, None).await {
        Ok(r) => r,
        Err(e) => {
            let mut mgr = state.sessions.lock().await;
            mgr.put_back(session);
            return Err(MuseError::from(e));
        }
    };
    let ok = !result.starts_with("error:");
    let tool = call.name.clone();
    let mut mgr = state.sessions.lock().await;
    mgr.put_back(session);
    Ok((StatusCode::OK, Json(json!({"ok": ok, "result": result, "tool": tool}))).into_response())
}

async fn agent_approve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, approval_id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let approved = match body.get("approved") {
        Some(Value::Bool(b)) => *b,
        _ => return Err(MuseError::invalid("approved must be a boolean")),
    };
    let exists = {
        let mgr = state.sessions.lock().await;
        mgr.get(&id).is_ok() || state.checked_out.lock().unwrap().contains_key(&id)
    };
    if !exists {
        return Err(MuseError::not_found(format!("agent session '{id}' not found")));
    }
    let mgr = state.sessions.lock().await;
    if !mgr.approve(&id, &approval_id, approved) {
        return Err(MuseError::not_found(format!(
            "approval '{approval_id}' not found or already answered"
        )));
    }
    Ok(Json(json!({"approval_id": approval_id, "approved": approved})))
}

async fn agent_changes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let mgr = state.sessions.lock().await;
    if let Ok(session) = mgr.get(&id) {
        return Ok(Json(session.changes_view()));
    }
    let checked = state.checked_out.lock().unwrap();
    if let Some(entry) = checked.get(&id) {
        // D-agt-2: snapshot taken at checkout (stale during the turn).
        return Ok(Json(entry.changes.clone()));
    }
    Err(MuseError::not_found(format!("agent session '{id}' not found")))
}

async fn agent_revert(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let path = opt_str(&body, "path").unwrap_or("").to_string();
    if path.is_empty() {
        return Err(MuseError::invalid("path is required"));
    }
    let mut mgr = state.sessions.lock().await;
    if mgr.get(&id).is_err() {
        if state.checked_out.lock().unwrap().contains_key(&id) {
            return Err(MuseError::session_busy());
        }
        return Err(MuseError::not_found(format!("agent session '{id}' not found")));
    }
    let session = mgr
        .take(&id)
        .map_err(|_| MuseError::not_found(format!("agent session '{id}' not found")))?;
    drop(mgr);
    let token = CancellationToken::new();
    if let Err(e) = session.tools.revert(&path, &token).await {
        let mut mgr = state.sessions.lock().await;
        mgr.put_back(session);
        return Err(MuseError::from(e));
    }
    let mut mgr = state.sessions.lock().await;
    mgr.put_back(session);
    Ok(Json(json!({"reverted": true, "path": path})))
}

// ---------------------------------------------------------------------------
// Agent: skills / preferences / screen
// ---------------------------------------------------------------------------

fn skill_json(def: &muse_store::SkillDefinition, enabled: bool, with_content: bool) -> Value {
    let mut v = json!({
        "alias": def.alias,
        "name": def.name,
        "description": def.description,
        "enabled": enabled,
        "is_folder": def.is_folder,
    });
    if with_content {
        v["content"] = Value::String(def.content.clone());
    }
    v
}

async fn agent_skills_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let data: Vec<Value> = state
        .skills
        .list()
        .iter()
        .map(|d| skill_json(d, state.skills.is_enabled(&d.alias), false))
        .collect();
    Ok(Json(json!({"object": "list", "data": data})))
}

async fn agent_skill_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    match state.skills.get(&alias) {
        Some(def) => Ok(Json(skill_json(&def, state.skills.is_enabled(&alias), true))),
        None => Err(MuseError::not_found(format!("skill '{alias}' not found"))),
    }
}

fn valid_skill_filename(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    !name.chars().any(|c| c == '/' || c == '\\' || c.is_control())
}

async fn agent_skill_import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let _caller = agent_auth(&state, &headers)?;
    let content = opt_str(&body, "content").unwrap_or("");
    if content.is_empty() {
        return Err(MuseError::invalid("content is required"));
    }
    if content.len() > 1024 * 1024 {
        return Err(MuseError::invalid("content must be at most 1 MiB"));
    }
    let filename = opt_str(&body, "filename").unwrap_or("skill.md");
    if !valid_skill_filename(filename) {
        return Err(MuseError::invalid("invalid filename"));
    }
    let tmp = std::env::temp_dir().join(format!("muse-skill-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&tmp).map_err(|e| MuseError::internal(e.to_string()))?;
    let file_path = tmp.join(filename);
    std::fs::write(&file_path, content).map_err(|e| MuseError::internal(e.to_string()))?;
    let imported = state.skills.import(&file_path);
    let _ = std::fs::remove_dir_all(&tmp);
    let def = imported?;
    let enabled = state.skills.is_enabled(&def.alias);
    Ok((StatusCode::CREATED, Json(skill_json(&def, enabled, false))))
}

async fn agent_skill_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let removed = state.skills.remove(&alias)?;
    Ok(Json(json!({"removed": removed, "alias": alias})))
}

async fn agent_skill_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(alias): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let enabled = match body.get("enabled") {
        Some(Value::Bool(b)) => *b,
        _ => return Err(MuseError::invalid("enabled must be a boolean")),
    };
    if state.skills.get(&alias).is_none() {
        return Err(MuseError::not_found(format!("skill '{alias}' not found")));
    }
    state.skills.set_enabled(&alias, enabled)?;
    Ok(Json(json!({"alias": alias, "enabled": enabled})))
}

async fn agent_preferences_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let mgr = state.sessions.lock().await;
    Ok(Json(mgr.preferences.view()))
}

async fn agent_preferences_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let mut mgr = state.sessions.lock().await;
    let mut prefs = mgr.preferences.clone();
    if let Some(v) = body.get("turbo") {
        prefs.turbo = v
            .as_bool()
            .ok_or_else(|| MuseError::invalid("turbo must be a boolean"))?;
    }
    // Rust extension (E6): `read_only` refuses screen control. Accepts
    // the `readonly` spelling too.
    if let Some(v) = body.get("read_only").or_else(|| body.get("readonly")) {
        prefs.read_only = v
            .as_bool()
            .ok_or_else(|| MuseError::invalid("read_only must be a boolean"))?;
    }
    if let Some(v) = body.get("reply_language") {
        prefs.reply_language = v
            .as_str()
            .ok_or_else(|| MuseError::invalid("reply_language must be a string"))?
            .to_string();
    }
    if let Some(v) = body.get("follow_question_language") {
        prefs.follow_question_language = v
            .as_bool()
            .ok_or_else(|| MuseError::invalid("follow_question_language must be a boolean"))?;
    }
    if let Some(v) = body.get("ui_language") {
        prefs.ui_language = v
            .as_str()
            .ok_or_else(|| MuseError::invalid("ui_language must be a string"))?
            .to_string();
    }
    mgr.apply_preferences(prefs);
    Ok(Json(mgr.preferences.view()))
}

async fn agent_screen_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let status = muse_screen::status();
    Ok(Json(serde_json::to_value(status).unwrap_or(json!({}))))
}

async fn agent_screen_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(action): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let _caller = agent_auth(&state, &headers)?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    // E5: disarm (emergency stop) must always work, even in read-only
    // mode — it only revokes control, never exercises it.
    if action != "disarm" {
        // E6: read-only refuses all screen control (.NET
        // `RequestComputerAuthorization` refuses while the Read-only
        // permission mode is on).
        let read_only = {
            let mgr = state.sessions.lock().await;
            mgr.preferences.read_only
        };
        if read_only {
            return Err(MuseError::forbidden(
                muse_agent::chat_turn::READ_ONLY_SCREEN_MESSAGE,
            ));
        }
    }
    let screen = Arc::clone(&state.screen);
    let target_before =
        serde_json::to_value(screen.target()).unwrap_or(Value::Null);
    let run = move || -> Result<Value, muse_screen::ScreenError> {
        use muse_screen::ScreenError;
        match action.as_str() {
            // Session actions (E1/E2/E3/E5): the window picker dialog,
            // live preview, auto-authorization and emergency stop.
            "list_windows" => {
                let windows = screen.list_windows()?;
                Ok(json!({"windows": windows, "target": target_before.clone()}))
            }
            "authorize" => {
                let w: muse_screen::DesktopWindow =
                    serde_json::from_value(body.clone()).map_err(|e| {
                        ScreenError::InvalidInput(format!("invalid window: {e}"))
                    })?;
                screen.authorize(w)?;
                Ok(json!({"authorized": true, "target": screen.target()}))
            }
            "preview" => {
                let image = screen.preview()?;
                Ok(json!({"image_data_url": image, "target": target_before.clone()}))
            }
            "disarm" => {
                screen.disarm();
                Ok(json!({"disarmed": true, "target": Value::Null}))
            }
            "auto_authorize" => {
                let hint = body.get("preferred_hint").and_then(|v| v.as_str());
                let ok = screen.try_auto_authorize(hint)?;
                Ok(json!({"authorized": ok, "target": screen.target()}))
            }
            // `computer_*` tool actions (E7): the same names the agent
            // loop uses (`ToolCallParser.cs` wire names).
            _ => {
                let act = muse_screen::ScreenAction::from_tool_name(&action)
                    .ok_or_else(|| {
                        ScreenError::InvalidInput(format!("unknown screen action '{action}'"))
                    })?;
                let args: muse_screen::ScreenArgs = if body.is_null() {
                    // No body = all-default arguments (e.g. a bare
                    // `computer_screenshot` with no element/coordinates).
                    muse_screen::ScreenArgs::default()
                } else {
                    serde_json::from_value(body.clone()).map_err(|e| {
                        ScreenError::InvalidInput(format!("invalid screen arguments: {e}"))
                    })?
                };
                let outcome = screen.execute(act, args)?;
                let mut v = json!({"text": outcome.text, "target": screen.target()});
                if let Some(img) = outcome.image_data_url {
                    v["image_data_url"] = Value::String(img);
                }
                Ok(v)
            }
        }
    };
    // `DesktopComputer` blocks up to ~14 s on Windows (2 s queue +
    // 12 s operation timeout); never block a tokio worker for it.
    let result = tokio::task::spawn_blocking(run)
        .await
        .map_err(|e| MuseError::internal(format!("screen worker failed: {e}")))?;
    result.map(Json).map_err(screen_error_to_api)
}

/// Map a screen error to the HTTP error (status + message) the caller
/// sees. `NotImplemented` stays the honest 501 on hosts without UIA.
fn screen_error_to_api(e: muse_screen::ScreenError) -> MuseError {
    use muse_screen::ScreenError;
    match &e {
        ScreenError::NotImplemented(m) => MuseError::not_implemented(m.clone()),
        ScreenError::InvalidInput(m) => MuseError::invalid(m.clone()),
        ScreenError::StaleElement | ScreenError::ElementChanged => {
            MuseError::invalid(e.to_string())
        }
        // No usable target / target gone / host refused the input:
        // a state conflict with the current screen session.
        ScreenError::Unavailable(m) | ScreenError::Blocked(m) => MuseError::conflict(m.clone()),
        ScreenError::WindowClosed => MuseError::conflict(e.to_string()),
        ScreenError::Timeout => MuseError(muse_core::MuseError::new(
            e.to_string(),
            504,
            "server_error",
            "timeout",
        )),
        ScreenError::Uia(m) => MuseError::internal(m.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_error_to_api_statuses() {
        use muse_screen::ScreenError;
        // Honest 501 off-Windows / without UIA.
        let e = screen_error_to_api(ScreenError::NotImplemented("no UIA".to_string()));
        assert_eq!(e.status(), StatusCode::NOT_IMPLEMENTED);
        // Bad caller input.
        let e = screen_error_to_api(ScreenError::InvalidInput("bad chord".to_string()));
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
        let e = screen_error_to_api(ScreenError::StaleElement);
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
        assert!(e.to_string().contains("stale"));
        let e = screen_error_to_api(ScreenError::ElementChanged);
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
        // State conflicts with the current screen session.
        let e = screen_error_to_api(ScreenError::Unavailable("no target".to_string()));
        assert_eq!(e.status(), StatusCode::CONFLICT);
        let e = screen_error_to_api(ScreenError::WindowClosed);
        assert_eq!(e.status(), StatusCode::CONFLICT);
        assert!(e.to_string().contains("closed"));
        let e = screen_error_to_api(ScreenError::Blocked("secure window".to_string()));
        assert_eq!(e.status(), StatusCode::CONFLICT);
        // Hung provider.
        let e = screen_error_to_api(ScreenError::Timeout);
        assert_eq!(e.status(), StatusCode::GATEWAY_TIMEOUT);
        // UIA/COM failure is a server error.
        let e = screen_error_to_api(ScreenError::Uia("0x80004005".to_string()));
        assert_eq!(e.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn forbidden_is_403() {
        let e = MuseError::forbidden("nope");
        assert_eq!(e.status(), StatusCode::FORBIDDEN);
        assert_eq!(e.to_string(), "nope");
    }
}
