//! Agent preferences (AgentEndpoints.cs:70-165).
//!
//! Persisted at `<data>/agent-preferences.json`. The .NET
//! `AgentSessionManager` loads them at startup (a corrupt file keeps the
//! defaults) and `ApplyPreferences` writes them back with plain
//! `JsonSerializer.Serialize` — i.e. **compact** JSON, PascalCase keys
//! (`Turbo`, `ReplyLanguage`, …). This crate writes that PascalCase form
//! (koreksi CONTRACTS.md §6, 2 Nov 2026) and reads both, like the other
//! stores. `View()` is the wire shape of `GET /v1/agent/preferences`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::StoreError;
use crate::json::read_file_to_string;

/// `AgentPreferences` (AgentEndpoints.cs:71-84) with .NET defaults.
///
/// `read_only` is a Rust extension (mirrors `muse-agent`): WinForms
/// keeps the Read-only permission mode in the UI only; the Rust
/// gateway persists it so screen control can be refused while
/// read-only (E6). Defaults to false; .NET ignores the unknown field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Preferences {
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

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            turbo: false,
            read_only: false,
            reply_language: "English".to_string(),
            follow_question_language: true,
            ui_language: "English".to_string(),
        }
    }
}

impl Preferences {
    /// `AgentPreferences.View()` (AgentEndpoints.cs:76-82).
    pub fn view(&self) -> Value {
        serde_json::json!({
            "turbo": self.turbo,
            "read_only": self.read_only,
            "reply_language": self.reply_language,
            "follow_question_language": self.follow_question_language,
            "ui_language": self.ui_language,
        })
    }

    /// Merge a partial update over the current preferences, mirroring
    /// `UpdatePreferences` (AgentEndpoints.cs:288-302): only fields that
    /// are present **and** of the right JSON type override; anything else
    /// keeps the current value.
    pub fn update_from_json(&self, body: &Value) -> Preferences {
        let mut out = self.clone();
        if let Some(b) = body.get("turbo").and_then(|v| v.as_bool()) {
            out.turbo = b;
        }
        if let Some(b) = body
            .get("read_only")
            .or_else(|| body.get("readonly"))
            .and_then(|v| v.as_bool())
        {
            out.read_only = b;
        }
        if let Some(s) = body.get("reply_language").and_then(|v| v.as_str()) {
            out.reply_language = s.to_string();
        }
        if let Some(b) = body
            .get("follow_question_language")
            .and_then(|v| v.as_bool())
        {
            out.follow_question_language = b;
        }
        if let Some(s) = body.get("ui_language").and_then(|v| v.as_str()) {
            out.ui_language = s.to_string();
        }
        out
    }
}

/// Load/save wrapper for the preferences file.
#[derive(Debug)]
pub struct PreferencesStore {
    file: PathBuf,
}

impl PreferencesStore {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        PreferencesStore { file: file.into() }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `LoadPreferences` (AgentEndpoints.cs:153-162): a missing or corrupt
    /// file keeps the defaults — never an error.
    pub fn load(&self) -> Preferences {
        let Ok(text) = read_file_to_string(&self.file) else {
            return Preferences::default();
        };
        serde_json::from_str::<Preferences>(&text).unwrap_or_default()
    }

    /// `ApplyPreferences`' write half (AgentEndpoints.cs:142-149):
    /// compact JSON (no indentation), written in place.
    pub fn save(&self, prefs: &Preferences) -> Result<(), StoreError> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::io_path(dir, e))?;
        }
        let json = serde_json::to_string(prefs).map_err(StoreError::JsonPlain)?;
        std::fs::write(&self.file, json).map_err(|e| StoreError::io_path(&self.file, e))
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
    fn defaults_view_save_load() {
        let dir = tmpdir("prefs");
        let file = dir.join("agent-preferences.json");
        let store = PreferencesStore::new(&file);
        // Missing file -> defaults.
        let prefs = store.load();
        assert_eq!(prefs, Preferences::default());
        assert_eq!(prefs.view()["reply_language"], "English");
        assert_eq!(prefs.view()["follow_question_language"], true);
        assert_eq!(prefs.view()["turbo"], false);
        assert_eq!(prefs.view()["read_only"], false);

        let mut changed = prefs.clone();
        changed.turbo = true;
        changed.read_only = true;
        changed.reply_language = "Bahasa Indonesia".to_string();
        store.save(&changed).unwrap();
        // Saved file is compact JSON with the contract field names.
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains('\n'));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["Turbo"], true);
        assert_eq!(v["ReadOnly"], true);
        assert_eq!(v["ReplyLanguage"], "Bahasa Indonesia");
        assert_eq!(store.load(), changed);
    }

    #[test]
    fn corrupt_file_keeps_defaults() {
        let dir = tmpdir("prefs-corrupt");
        let file = dir.join("agent-preferences.json");
        std::fs::write(&file, "{not json").unwrap();
        let store = PreferencesStore::new(&file);
        assert_eq!(store.load(), Preferences::default());
    }

    #[test]
    fn reads_dotnet_shaped_file() {
        let dir = tmpdir("prefs-dotnet");
        let file = dir.join("agent-preferences.json");
        std::fs::write(
            &file,
            r#"{"Turbo":true,"ReplyLanguage":"English","FollowQuestionLanguage":false,"UiLanguage":"English"}"#,
        )
        .unwrap();
        let store = PreferencesStore::new(&file);
        let prefs = store.load();
        assert!(prefs.turbo);
        assert!(!prefs.follow_question_language);
    }

    #[test]
    fn update_merges_only_valid_fields() {
        let prefs = Preferences::default();
        let body: Value = serde_json::json!({
            "turbo": true,
            "reply_language": 42,
            "ui_language": "Bahasa Indonesia"
        });
        let updated = prefs.update_from_json(&body);
        assert!(updated.turbo);
        assert_eq!(updated.reply_language, "English"); // wrong type: kept
        assert_eq!(updated.ui_language, "Bahasa Indonesia");
        assert!(updated.follow_question_language);
        assert!(!updated.read_only);
    }

    #[test]
    fn update_merges_read_only() {
        let prefs = Preferences::default();
        let body: Value = serde_json::json!({"read_only": true});
        assert!(prefs.update_from_json(&body).read_only);
        let body: Value = serde_json::json!({"readonly": true});
        assert!(prefs.update_from_json(&body).read_only);
        let body: Value = serde_json::json!({"read_only": "yes"});
        assert!(!prefs.update_from_json(&body).read_only); // wrong type: kept
    }
}
