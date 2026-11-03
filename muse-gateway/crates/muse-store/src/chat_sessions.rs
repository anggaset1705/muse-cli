//! Chat sessions (ChatSessions.cs).
//!
//! One JSON file per conversation, under `<data>/agent-sessions/<sha256>/`
//! where `<sha256>` is the UPPERCASE hex SHA-256 of the canonical workspace
//! path (`ChatSessionStore.DirectoryFor`, ChatSessions.cs:20-22). The file
//! name is `<session-id>.json`, the id being a 32-hex-char GUID (`"N"`
//! format). Session files are written indented, atomically (temp file +
//! rename), and rejected when larger than 8,000,000 characters.
//!
//! Field names (CONTRACTS.md §6, koreksi 2 Nov 2026): PascalCase on
//! write (`Id`, `Title`, `Workspace`, `Model`, `UpdatedAt`, `History`,
//! `Events`, `Goal`); snake_case spellings are accepted on read, matching
//! the .NET default serialization. `updated_at` is an ISO-8601 timestamp with offset
//! (`.NET DateTimeOffset`); this crate stores it as `DateTime<Utc>`,
//! which parses any offset and normalizes to UTC on write.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{new_guid_n, sha256_hex_upper, StoreError};
use crate::json::{read_file_to_string, write_atomic};

/// Maximum serialized session size in characters (ChatSessions.cs:47).
pub const MAX_SESSION_CHARS: usize = 8_000_000;
/// Maximum sessions `list` returns (ChatSessions.cs:62).
pub const MAX_LIST_SESSIONS: usize = 100;

/// `AgentEvent` (AgentLoop.cs:5): `record AgentEvent(string Kind, string Text, string? Tool = null, string? Path = null)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AgentEvent {
    #[serde(default, alias = "kind")]
    pub kind: String,
    #[serde(default, alias = "text")]
    pub text: String,
    #[serde(default, alias = "tool")]
    pub tool: Option<String>,
    #[serde(default, alias = "path")]
    pub path: Option<String>,
}

/// `GoalRun` (GoalRun.cs:7-15): the persisted objective, never desktop
/// permissions or stale test evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GoalRun {
    #[serde(default, alias = "objective")]
    pub objective: String,
    #[serde(default = "default_goal_status", alias = "status")]
    pub status: String,
    #[serde(default = "default_goal_summary", alias = "summary")]
    pub summary: String,
    #[serde(
        default = "Utc::now",
        alias = "updated_at",
        serialize_with = "ser_datetime_dotnet",
        deserialize_with = "de_datetime"
    )]
    pub updated_at: DateTime<Utc>,
    #[serde(default, alias = "pending_question")]
    pub pending_question: Option<String>,
    #[serde(default, alias = "follow_ups")]
    pub follow_ups: Vec<String>,
}

fn default_goal_status() -> String {
    "paused".to_string()
}

fn default_goal_summary() -> String {
    "Not verified yet.".to_string()
}

/// `ChatSession` (ChatSessions.cs:5-16).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChatSession {
    #[serde(default = "new_session_id", alias = "id")]
    pub id: String,
    #[serde(default = "default_title", alias = "title")]
    pub title: String,
    #[serde(default, alias = "workspace")]
    pub workspace: String,
    #[serde(default = "default_model", alias = "model")]
    pub model: String,
    #[serde(
        default = "Utc::now",
        alias = "updated_at",
        serialize_with = "ser_datetime_dotnet",
        deserialize_with = "de_datetime"
    )]
    pub updated_at: DateTime<Utc>,
    #[serde(default, alias = "history")]
    pub history: Vec<Map<String, Value>>,
    #[serde(default, alias = "events")]
    pub events: Vec<AgentEvent>,
    #[serde(default, alias = "goal")]
    pub goal: Option<GoalRun>,
}

fn default_title() -> String {
    "New conversation".to_string()
}

fn default_model() -> String {
    "muse-chat".to_string()
}

/// Serialize a `DateTime<Utc>` the way `System.Text.Json` writes a
/// `DateTimeOffset`: `2026-11-02T06:24:05.1234567+00:00` — up to 7
/// fractional digits (ticks), `+00:00` offset, fraction omitted when zero.
fn ser_datetime_dotnet<S>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use chrono::Timelike;
    let base = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    let nanos = dt.nanosecond();
    if nanos == 0 {
        s.serialize_str(&format!("{base}+00:00"))
    } else {
        // .NET ticks are 100 ns; truncate the nanoseconds to 7 digits.
        s.serialize_str(&format!("{base}.{:07}+00:00", nanos / 100))
    }
}

/// Deserialize an ISO-8601 timestamp with any offset (`Z`, `+00:00`,
/// `+07:00`, …) into UTC. Accepts the .NET 7-digit form and chrono's
/// 9-digit form alike.
fn de_datetime<'de, D>(d: D) -> Result<DateTime<Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(serde::de::Error::custom)
}

/// A session id is a GUID in `"N"` format: 32 lowercase hex chars.
pub fn new_session_id() -> String {
    new_guid_n()
}

fn is_valid_session_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

impl ChatSession {
    pub fn new(workspace: &str) -> Self {
        ChatSession {
            id: new_session_id(),
            title: default_title(),
            workspace: workspace.to_string(),
            model: default_model(),
            updated_at: Utc::now(),
            history: Vec::new(),
            events: Vec::new(),
            goal: None,
        }
    }
}

/// `ChatSessionStore` (ChatSessions.cs:19-74).
#[derive(Debug, Clone)]
pub struct ChatSessionStore {
    root: PathBuf,
}

impl ChatSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ChatSessionStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `ChatSessionStore.Canonical` (ChatSessions.cs:15-18): the workspace
    /// path made absolute with trailing separators trimmed; uppercased on
    /// Windows only.
    pub fn canonical_workspace(workspace: &str) -> String {
        let path = Path::new(workspace);
        let absolute = std::path::absolute(path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| workspace.to_string());
        let trimmed = absolute.trim_end_matches(['/', '\\']);
        #[cfg(windows)]
        {
            trimmed.to_uppercase()
        }
        #[cfg(not(windows))]
        {
            trimmed.to_string()
        }
    }

    /// `ChatSessionStore.DirectoryFor` (ChatSessions.cs:20-22): the
    /// per-workspace directory under the root, named by the UPPERCASE hex
    /// SHA-256 of the canonical workspace path.
    pub fn directory_for(&self, workspace: &str) -> PathBuf {
        let canonical = Self::canonical_workspace(workspace);
        self.root
            .join(sha256_hex_upper(canonical.as_bytes()))
    }

    /// `ChatSessionStore.FileFor` (ChatSessions.cs:24-28): validates the id
    /// is a GUID in `"N"` format.
    pub fn file_for(&self, workspace: &str, id: &str) -> Result<PathBuf, StoreError> {
        if !is_valid_session_id(id) {
            return Err(StoreError::invalid("Invalid session ID"));
        }
        Ok(self.directory_for(workspace).join(format!("{id}.json")))
    }

    /// `ChatSessionStore.SaveAsync` (ChatSessions.cs:30-45): stamps
    /// `updated_at` to now, writes indented JSON atomically, refuses
    /// sessions over 8,000,000 characters.
    pub fn save(&self, session: &mut ChatSession) -> Result<(), StoreError> {
        let file = self.file_for(&session.workspace, &session.id)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::io_path(dir, e))?;
        }
        session.updated_at = Utc::now();
        let json = serde_json::to_string_pretty(session).map_err(StoreError::JsonPlain)?;
        if json.len() > MAX_SESSION_CHARS {
            return Err(StoreError::invalid(
                "Chat history exceeds 8 million characters; start a new conversation",
            ));
        }
        let tmp_name = format!(
            "{}.{}.tmp",
            file.file_name().unwrap().to_string_lossy(),
            new_guid_n()
        );
        write_atomic(&file, &json, &tmp_name)
    }

    /// `ChatSessionStore.LoadAsync` (ChatSessions.cs:47-53): the stored id
    /// must match the requested id and the stored workspace must be the
    /// requested workspace (canonically), else the session is refused.
    pub fn load(&self, workspace: &str, id: &str) -> Result<ChatSession, StoreError> {
        let file = self.file_for(workspace, id)?;
        let text = read_file_to_string(&file)?;
        let session: ChatSession =
            serde_json::from_str(&text).map_err(|e| StoreError::json_path(&file, e))?;
        if session.id != id {
            return Err(StoreError::invalid("Session does not belong to this workspace"));
        }
        if Self::canonical_workspace(&session.workspace) != Self::canonical_workspace(workspace)
        {
            return Err(StoreError::invalid("Session does not belong to this workspace"));
        }
        Ok(session)
    }

    /// `ChatSessionStore.ListAsync` (ChatSessions.cs:55-70): up to 100
    /// files, newest-written first; broken files are skipped; the result
    /// is ordered by `updated_at`, newest first.
    pub fn list(&self, workspace: &str) -> Vec<ChatSession> {
        let directory = self.directory_for(workspace);
        if !directory.is_dir() {
            return Vec::new();
        }
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    let mtime = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    files.push((path, mtime));
                }
            }
        }
        files.sort_by(|a, b| b.1.cmp(&a.1));
        files.truncate(MAX_LIST_SESSIONS);
        let mut sessions: Vec<ChatSession> = Vec::new();
        for (file, _) in files {
            let Some(stem) = file.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if let Ok(session) = self.load(workspace, &stem) {
                sessions.push(session);
            }
            // Broken files are skipped, like the .NET catch of
            // JsonException / IOException / InvalidOperationException /
            // ArgumentException.
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("muse-store-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmpdir("chat");
        let store = ChatSessionStore::new(dir.join("agent-sessions"));
        let workspace = dir.join("ws").to_string_lossy().into_owned();
        let mut session = ChatSession::new(&workspace);
        session.title = "Fix the bug".to_string();
        let mut msg = Map::new();
        msg.insert("role".into(), Value::String("user".into()));
        msg.insert("content".into(), Value::String("hello".into()));
        session.history.push(msg);
        session.events.push(AgentEvent {
            kind: "completed".into(),
            text: "done".into(),
            tool: Some("read_file".into()),
            path: None,
        });
        session.goal = Some(GoalRun {
            objective: "Ship it".into(),
            status: "running".into(),
            summary: "Not verified yet.".into(),
            updated_at: Utc::now(),
            pending_question: None,
            follow_ups: vec!["more context".into()],
        });
        let id = session.id.clone();
        store.save(&mut session).unwrap();

        // File layout: <root>/<UPPERCASE-SHA256>/<id>.json, contract fields.
        let file = store.file_for(&workspace, &id).unwrap();
        assert!(file.exists());
        let dir_name = file
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(dir_name.len(), 64);
        assert!(dir_name.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(dir_name, dir_name.to_uppercase());
        let text = std::fs::read_to_string(&file).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        // PascalCase on disk (CONTRACTS.md §6); history entries are raw
        // message maps, so their keys stay lowercase.
        assert_eq!(v["Title"], "Fix the bug");
        assert_eq!(v["Model"], "muse-chat");
        assert_eq!(v["History"][0]["role"], "user");
        assert_eq!(v["Events"][0]["Kind"], "completed");
        assert_eq!(v["Events"][0]["Path"], Value::Null);
        assert_eq!(v["Goal"]["Objective"], "Ship it");
        assert_eq!(v["Goal"]["FollowUps"][0], "more context");

        let back = store.load(&workspace, &id).unwrap();
        assert_eq!(back.id, id);
        assert_eq!(back.title, "Fix the bug");
        assert_eq!(back.history.len(), 1);
        assert_eq!(back.events[0].tool.as_deref(), Some("read_file"));
        assert_eq!(back.goal.as_ref().unwrap().status, "running");
    }

    #[test]
    fn workspace_isolation_and_validation() {
        let dir = tmpdir("chat-iso");
        let store = ChatSessionStore::new(dir.join("agent-sessions"));
        let ws_a = dir.join("a").to_string_lossy().into_owned();
        let ws_b = dir.join("b").to_string_lossy().into_owned();
        assert_ne!(store.directory_for(&ws_a), store.directory_for(&ws_b));
        let mut session = ChatSession::new(&ws_a);
        store.save(&mut session).unwrap();
        // Same id under another workspace does not exist.
        assert!(store.load(&ws_b, &session.id).is_err());
        // Invalid ids are rejected before touching the disk.
        assert!(store.load(&ws_a, "not-a-guid").is_err());
        assert!(store.load(&ws_a, "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
        // A session file whose stored workspace differs is refused.
        let file = store.file_for(&ws_a, &session.id).unwrap();
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        v["Workspace"] = Value::String(ws_b.clone());
        std::fs::write(&file, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        assert!(store.load(&ws_a, &session.id).is_err());
    }

    #[test]
    fn list_orders_by_updated_at_and_skips_broken() {
        let dir = tmpdir("chat-list");
        let store = ChatSessionStore::new(dir.join("agent-sessions"));
        let ws = dir.join("ws").to_string_lossy().into_owned();
        let mut s1 = ChatSession::new(&ws);
        s1.title = "first".into();
        store.save(&mut s1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut s2 = ChatSession::new(&ws);
        s2.title = "second".into();
        store.save(&mut s2).unwrap();
        // A broken file in the same directory is skipped.
        let broken = store
            .directory_for(&ws)
            .join(format!("{}.json", new_session_id()));
        std::fs::write(&broken, "{broken json").unwrap();
        let list = store.list(&ws);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title, "second");
        assert_eq!(list[1].title, "first");
        assert!(store.list(&dir.join("other").to_string_lossy()).is_empty());
    }

    #[test]
    fn reads_dotnet_shaped_file() {
        let dir = tmpdir("chat-dotnet");
        let store = ChatSessionStore::new(dir.join("agent-sessions"));
        let ws = dir.join("ws").to_string_lossy().into_owned();
        let id = new_session_id();
        let file = store.file_for(&ws, &id).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            format!(
                r#"{{"Id": "{id}", "Title": "Net", "Workspace": "{}", "Model": "muse-chat", "UpdatedAt": "2026-11-02T06:24:05.1234567+00:00", "History": [], "Events": [{{"Kind": "completed", "Text": "ok", "Tool": null, "Path": null}}], "Goal": null}}"#,
                ws.replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let back = store.load(&ws, &id).unwrap();
        assert_eq!(back.title, "Net");
        assert_eq!(back.events[0].kind, "completed");
    }
}
