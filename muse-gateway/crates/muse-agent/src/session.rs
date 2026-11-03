//! Agent sessions — port of the `AgentSession` / `AgentSessionManager` /
//! `AgentPreferences` classes inside `AgentEndpoints.cs`, plus the SSE
//! event shapes from CONTRACTS.md §7.
//!
//! History persistence (`ChatSessionStore`) belongs to the `muse-store`
//! crate per CONTRACTS.md §11; this manager keeps sessions in memory
//! only, and the gateway wires persistence at integration time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::error::{AgentError, AgentResult, CancellationToken};
use crate::executor::ToolExecutor;
use crate::goal::GoalRun;

/// One loop event. The gateway serialises these as SSE `data:` frames;
/// `kind` values mirror .NET `AgentEvent.Kind`.
#[derive(Debug, Clone)]
pub struct AgentEvent {
    pub kind: String,
    pub text: String,
    pub tool: Option<String>,
    pub path: Option<String>,
}

impl AgentEvent {
    pub fn new(kind: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            text: text.into(),
            tool: None,
            path: None,
        }
    }

    pub fn with_tool(mut self, tool: impl Into<String>, path: Option<String>) -> Self {
        self.tool = Some(tool.into());
        self.path = path;
        self
    }

    /// JSON payload of one SSE frame (`.NET AgentEndpoints.Observe`).
    pub fn to_json(&self) -> Value {
        let mut obj = json!({ "event": self.kind, "text": self.text });
        if let Some(tool) = &self.tool {
            obj["tool"] = Value::String(tool.clone());
        }
        if let Some(path) = &self.path {
            obj["path"] = Value::String(path.clone());
        }
        obj
    }

    /// One SSE frame: `data: {json}\n\n`.
    pub fn to_sse(&self) -> String {
        format!("data: {}\n\n", self.to_json())
    }
}

/// The terminal `done` event (CONTRACTS.md §7 / `AgentEndpoints.cs:360-404`).
/// NOTE: .NET's streamed `done` object has no `session_id` field — the
/// contract example shows one, so it is included here (additive only).
pub fn done_event(
    ok: bool,
    status: &str,
    summary: &str,
    session_id: &str,
) -> Value {
    json!({
        "event": "done",
        "ok": ok,
        "summary": summary,
        "status": status,
        "session_id": session_id,
    })
}

/// The stream terminator, sent after `done`.
pub const SSE_DONE: &str = "data: [DONE]\n\n";

/// Whether an event kind is a turn outcome (`.NET AgentLoop` observer).
pub fn is_turn_outcome(kind: &str) -> bool {
    matches!(
        kind,
        "completed" | "incomplete" | "cancelled" | "declined" | "waiting_input"
    )
}

/// Agent preferences, persisted under the data dir
/// (`agent-preferences.json`) — `.NET AgentPreferences`.
/// Serialized PascalCase like .NET (`JsonSerializer.Serialize` default)
/// — `agent-preferences.json` must stay readable by the .NET gateway;
/// snake_case spellings are accepted on read (integrasi 2 Nov 2026).
///
/// `read_only` is a Rust extension with no .NET counterpart: WinForms
/// keeps the Read-only permission mode in the UI only
/// (`_permissions.SelectedIndex == 1`); here it is persisted so the
/// gateway can enforce it (E6 — screen control refused while
/// read-only, at both the HTTP screen endpoint and the `computer_*`
/// loop tools). It defaults to false and is ignored by .NET.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AgentPreferences {
    #[serde(default, alias = "turbo")]
    pub turbo: bool,
    #[serde(default, alias = "read_only", alias = "readonly")]
    pub read_only: bool,
    #[serde(default = "default_reply_language", alias = "reply_language")]
    pub reply_language: String,
    #[serde(default = "default_true", alias = "follow_question_language")]
    pub follow_question_language: bool,
    #[serde(default = "default_ui_language", alias = "ui_language")]
    pub ui_language: String,
}

fn default_reply_language() -> String {
    "English".to_string()
}
fn default_ui_language() -> String {
    "English".to_string()
}
fn default_true() -> bool {
    true
}

impl Default for AgentPreferences {
    fn default() -> Self {
        Self {
            turbo: false,
            read_only: false,
            reply_language: "English".to_string(),
            follow_question_language: true,
            ui_language: "English".to_string(),
        }
    }
}

impl AgentPreferences {
    pub fn view(&self) -> Value {
        json!({
            "turbo": self.turbo,
            "read_only": self.read_only,
            "reply_language": self.reply_language,
            "follow_question_language": self.follow_question_language,
            "ui_language": self.ui_language,
        })
    }
}

/// Shared registry of pending approvals for one session. It is an `Arc`
/// so the gateway can answer an approval (`.NET AgentEndpoints.Approve`)
/// while a turn holds the session elsewhere: the turn registers the
/// oneshot sender here, the approve endpoint takes it out and answers.
/// The same shape is used for the manager-level run-token map below.
pub type ApprovalsRegistry = Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>;

/// One agent session: workspace + tools + conversation history + goal.
pub struct AgentSession {
    pub id: String,
    pub workspace: PathBuf,
    pub model: String,
    pub reply_language: String,
    pub turbo: bool,
    /// Screen control refused while true (E6). Sourced from
    /// `AgentPreferences` at creation and on `apply_preferences`.
    pub read_only: bool,
    pub max_steps: Option<u32>,
    pub title: String,
    pub tools: ToolExecutor,
    pub history: Vec<Value>,
    pub goal: Option<GoalRun>,
    pub updated_at: DateTime<Utc>,
    pub running: bool,
    pub(crate) run_token: Option<CancellationToken>,
    /// Shared with the manager so approvals can be answered while a turn
    /// is running (see `ApprovalsRegistry`).
    pub approvals: ApprovalsRegistry,
    pub last_outcome: Option<(String, String)>,
    pub skills_directory: Option<PathBuf>,
}

impl AgentSession {
    /// Create a session. The workspace directory is created when missing
    /// (`.NET AgentSession` constructor: first chat must succeed with a
    /// fresh default workspace).
    pub fn new(
        workspace: impl AsRef<Path>,
        shell: &str,
        model: &str,
        reply_language: &str,
        turbo: bool,
        max_steps: Option<u32>,
    ) -> AgentResult<Self> {
        let workspace = workspace.as_ref();
        std::fs::create_dir_all(workspace)?;
        let canonical = std::fs::canonicalize(workspace)?;
        Ok(Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            workspace: canonical.clone(),
            model: if model.is_empty() {
                "muse-chat".to_string()
            } else {
                model.to_string()
            },
            reply_language: if reply_language.is_empty() {
                "English".to_string()
            } else {
                reply_language.to_string()
            },
            turbo,
            read_only: false,
            max_steps,
            title: "New conversation".to_string(),
            tools: ToolExecutor::new(canonical, shell)?,
            history: Vec::new(),
            goal: None,
            updated_at: Utc::now(),
            running: false,
            run_token: None,
            approvals: Arc::new(Mutex::new(HashMap::new())),
            last_outcome: None,
            skills_directory: None,
        })
    }

    /// Cancel the running turn (`.NET AgentSession.Cancel`).
    /// Returns false when nothing was running.
    pub fn cancel(&self) -> bool {
        match &self.run_token {
            Some(token) if !token.is_cancelled() => {
                token.cancel();
                true
            }
            _ => false,
        }
    }

    /// Answer a pending approval (`.NET AgentEndpoints.Approve`).
    /// Returns false when the id is unknown or already answered.
    pub fn approve(&self, approval_id: &str, approved: bool) -> bool {
        let sender = self
            .approvals
            .lock()
            .unwrap()
            .remove(approval_id);
        match sender {
            Some(sender) => sender.send(approved).is_ok(),
            None => false,
        }
    }

    /// `SessionView` (`.NET AgentEndpoints.SessionView`).
    pub fn view(&self) -> Value {
        json!({
            "session_id": self.id,
            "workspace": self.workspace.to_string_lossy(),
            "model": self.model,
            "reply_language": self.reply_language,
            "turbo": self.turbo,
            "read_only": self.read_only,
            "running": self.running,
            "history_count": self.history.len(),
            "goal": self.goal.as_ref().map(|g| serde_json::to_value(g).unwrap_or(Value::Null)).unwrap_or(Value::Null),
            "updated_at": format_updated_at(self.updated_at),
        })
    }

    /// `ChangesView` (`.NET AgentEndpoints.ChangesView`).
    pub fn changes_view(&self) -> Value {
        let data: Vec<Value> = self
            .tools
            .changes()
            .iter()
            .map(|change| {
                json!({
                    "path": change.path,
                    "kind": change.kind(),
                    "diff": change.diff(),
                })
            })
            .collect();
        json!({
            "session_id": self.id,
            "changes": data,
        })
    }

    /// The 403 approval-required body (`.NET AgentEndpoints.DirectTool`).
    pub fn approval_required_body(
        &self,
        approval: &crate::executor::ToolApproval,
    ) -> Value {
        json!({
            "ok": false,
            "error": "approval_required",
            "tool": approval.call.name,
            "description": approval.description,
            "preview": approval.preview,
            "message": "This tool needs approval. Resend with {\"approved\": true} to run it.",
        })
    }
}

fn format_updated_at(t: DateTime<Utc>) -> String {
    // .NET `DateTimeOffset.ToString("o")` — ISO 8601 with offset.
    t.format("%Y-%m-%dT%H:%M:%S%.f%:z").to_string()
}

/// In-memory session registry (`.NET AgentSessionManager`).
pub struct AgentSessionManager {
    sessions: HashMap<String, AgentSession>,
    pub preferences: AgentPreferences,
    prefs_file: Option<PathBuf>,
    pub sessions_root: Option<PathBuf>,
    pub skills_directory: Option<PathBuf>,
    /// Per-session approval registries, so the approve endpoint can answer
    /// while a turn holds the session (see `ApprovalsRegistry`).
    approval_registries: HashMap<String, ApprovalsRegistry>,
    /// Per-session run tokens, so the cancel endpoint can cancel while a
    /// turn holds the session. The gateway registers the token it passes
    /// to `run_turn` here.
    run_tokens: HashMap<String, CancellationToken>,
}

impl AgentSessionManager {
    /// Create a manager rooted at `data_dir` (preferences live at
    /// `<data_dir>/agent-preferences.json`; `agent-sessions/` and
    /// `skills/` are created, like .NET). Pass `None` for a purely
    /// in-memory manager (tests).
    pub fn new(data_dir: Option<&Path>) -> Self {
        let (prefs_file, sessions_root, skills_directory) = match data_dir {
            Some(dir) => (
                Some(dir.join("agent-preferences.json")),
                Some(dir.join("agent-sessions")),
                Some(dir.join("skills")),
            ),
            None => (None, None, None),
        };
        if let Some(root) = &sessions_root {
            let _ = std::fs::create_dir_all(root);
        }
        if let Some(dir) = &skills_directory {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut mgr = Self {
            sessions: HashMap::new(),
            preferences: AgentPreferences::default(),
            prefs_file,
            sessions_root,
            skills_directory,
            approval_registries: HashMap::new(),
            run_tokens: HashMap::new(),
        };
        mgr.load_preferences();
        mgr
    }

    fn load_preferences(&mut self) {
        if let Some(file) = &self.prefs_file {
            if let Ok(text) = std::fs::read_to_string(file) {
                if let Ok(loaded) = serde_json::from_str::<AgentPreferences>(&text) {
                    self.preferences = loaded;
                }
                // Corrupt preferences file: keep defaults (.NET behaviour).
            }
        }
    }

    /// Create a session and return its id (`.NET AgentSessionManager.Create`).
    pub fn create(
        &mut self,
        workspace: &str,
        shell: Option<&str>,
        model: Option<&str>,
        reply_language: Option<&str>,
        turbo: Option<bool>,
        max_steps: Option<u32>,
    ) -> AgentResult<String> {
        if workspace.trim().is_empty() {
            return Err(AgentError::Invalid("workspace is required".to_string()));
        }
        let prefs = &self.preferences;
        let mut session = AgentSession::new(
            workspace,
            shell.unwrap_or("auto"),
            model.unwrap_or(""),
            reply_language.unwrap_or(""),
            turbo.unwrap_or(prefs.turbo),
            max_steps,
        )?;
        if reply_language.is_none() || reply_language == Some("") {
            session.reply_language = prefs.reply_language.clone();
        }
        session.read_only = prefs.read_only;
        session.skills_directory = self.skills_directory.clone();
        let id = session.id.clone();
        if self.sessions.contains_key(&id) {
            return Err(AgentError::Invalid("session id collision".to_string()));
        }
        self.approval_registries
            .insert(id.clone(), session.approvals.clone());
        self.sessions.insert(id.clone(), session);
        Ok(id)
    }

    pub fn get(&self, id: &str) -> AgentResult<&AgentSession> {
        self.sessions
            .get(id)
            .ok_or_else(|| AgentError::Invalid(format!("agent session '{id}' not found")))
    }

    pub fn get_mut(&mut self, id: &str) -> AgentResult<&mut AgentSession> {
        self.sessions
            .get_mut(id)
            .ok_or_else(|| AgentError::Invalid(format!("agent session '{id}' not found")))
    }

    /// Check a session out of the registry for exclusive use by one
    /// gateway handler (a chat turn or a direct tool call), so the turn
    /// can run without holding the manager lock — otherwise `cancel` and
    /// `approve` could not reach a running turn. The approval registry
    /// and run token stay registered under the id, so `approve`/`cancel`
    /// keep working while the session is checked out. Pair with
    /// [`Self::put_back`]; the gateway keeps a view snapshot for
    /// `GET /v1/agent/sessions/{id}` while a session is checked out.
    ///
    /// Added at integration (2 Nov 2026); the worker crate had no
    /// equivalent — the manager previously only lent `&mut` borrows.
    pub fn take(&mut self, id: &str) -> AgentResult<AgentSession> {
        self.sessions
            .remove(id)
            .ok_or_else(|| AgentError::Invalid(format!("agent session '{id}' not found")))
    }

    /// Return a checked-out session to the registry (no-op when the id
    /// was deleted while checked out — see `remove` — or when a session
    /// with the same id is already present).
    pub fn put_back(&mut self, session: AgentSession) {
        let id = session.id.clone();
        if self.sessions.contains_key(&id) {
            return;
        }
        self.approval_registries
            .insert(id.clone(), session.approvals.clone());
        self.sessions.insert(id, session);
    }

    /// All sessions, newest first.
    pub fn all(&self) -> Vec<&AgentSession> {
        let mut sessions: Vec<&AgentSession> = self.sessions.values().collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }

    /// `SessionList` (`.NET AgentEndpoints.SessionList`).
    pub fn list_view(&self) -> Value {
        let data: Vec<Value> = self.all().iter().map(|s| s.view()).collect();
        json!({ "object": "list", "data": data })
    }

    /// Remove a session: cancel its turn and stop its background
    /// processes (`.NET AgentSessionManager.RemoveAsync`). History
    /// persistence is the gateway's job (muse-store `ChatSessionStore`).
    pub async fn remove(&mut self, id: &str) -> AgentResult<()> {
        let session = self.get(id)?;
        session.cancel();
        session.tools.stop_background_processes().await;
        self.sessions.remove(id);
        self.approval_registries.remove(id);
        self.run_tokens.remove(id);
        Ok(())
    }

    /// Register the cancellation token of a running turn, so `cancel`
    /// can stop it while the turn holds the session.
    pub fn register_run_token(&mut self, id: &str, token: CancellationToken) {
        self.run_tokens.insert(id.to_string(), token);
    }

    /// Cancel a running turn via the registered token
    /// (`.NET AgentEndpoints` cancel route). Returns false when nothing
    /// was running.
    pub fn cancel(&mut self, id: &str) -> bool {
        match self.run_tokens.get(id) {
            Some(token) if !token.is_cancelled() => {
                token.cancel();
                true
            }
            _ => false,
        }
    }

    /// Answer a pending approval without holding the session
    /// (`.NET AgentEndpoints.Approve`). Returns false when unknown or
    /// already answered.
    pub fn approve(&self, id: &str, approval_id: &str, approved: bool) -> bool {
        match self.approval_registries.get(id) {
            Some(registry) => {
                let sender = registry.lock().unwrap().remove(approval_id);
                match sender {
                    Some(sender) => sender.send(approved).is_ok(),
                    None => false,
                }
            }
            None => false,
        }
    }

    /// Apply new preferences to the file and to live sessions
    /// (`.NET AgentSessionManager.ApplyPreferences`).
    pub fn apply_preferences(&mut self, prefs: AgentPreferences) {
        if let Some(file) = &self.prefs_file {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(
                file,
                serde_json::to_string(&prefs).unwrap_or_default(),
            );
        }
        self.preferences = prefs.clone();
        for session in self.sessions.values_mut() {
            session.turbo = prefs.turbo;
            session.read_only = prefs.read_only;
            session.reply_language = prefs.reply_language.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("muse-agent-session-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_only_defaults_off_and_round_trips() {
        let prefs = AgentPreferences::default();
        assert!(!prefs.read_only);
        assert_eq!(prefs.view()["read_only"], false);
        // PascalCase on disk (like .NET), snake_case on read.
        let v: Value = serde_json::to_value(&prefs).unwrap();
        assert_eq!(v["ReadOnly"], false);
        let back: AgentPreferences =
            serde_json::from_value(serde_json::json!({"ReadOnly": true})).unwrap();
        assert!(back.read_only);
        let back2: AgentPreferences =
            serde_json::from_value(serde_json::json!({"read_only": true})).unwrap();
        assert!(back2.read_only);
    }

    #[test]
    fn read_only_propagates_to_sessions() {
        let dir = tmpdir("readonly");
        let mut mgr = AgentSessionManager::new(Some(dir.as_path()));
        let mut prefs = AgentPreferences::default();
        prefs.read_only = true;
        mgr.apply_preferences(prefs);
        let ws = dir.join("ws");
        let id = mgr.create(ws.to_str().unwrap(), None, None, None, None, None).unwrap();
        assert!(mgr.get(&id).unwrap().read_only);
        assert_eq!(mgr.get(&id).unwrap().view()["read_only"], true);
        // Turning it off propagates to the live session too.
        let mut prefs = AgentPreferences::default();
        prefs.read_only = false;
        mgr.apply_preferences(prefs);
        assert!(!mgr.get(&id).unwrap().read_only);
    }
}
