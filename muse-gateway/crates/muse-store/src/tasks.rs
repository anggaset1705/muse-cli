//! `tasks.json`: `BgTask` + `TaskStore` (Store.cs:691-842).
//!
//! This is the **persistence** half of the .NET `TaskManager`. The execution
//! half — `Submit`'s runner closure, the async `Run` loop, `Cancel` via
//! `CancellationTokenSource`, `StopAsync` — belongs to `muse-agent`, which
//! owns the work itself; this crate stores the records and the state
//! transitions the gateway needs (create, progress, result/error, cancel).
//!
//! Wire format (CONTRACTS.md §6, koreksi 2 Nov 2026): a JSON array with
//! PascalCase fields (`Id`, `Kind`, `Status`, `Progress`, `CreatedAt`,
//! `UpdatedAt`, `Request`, `Result`, `Error`); snake_case spellings
//! accepted on read.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{now_unix, random_hex_bytes, StoreError};
use crate::json::{read_file_to_string, ser_f64_dotnet, write_atomic};

/// `BgTask` (Store.cs:692-704). `request`/`result`/`error` are arbitrary
/// JSON objects; `result`/`error` are `null` until the task finishes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BgTask {
    #[serde(default = "new_task_id", alias = "id")]
    pub id: String,
    #[serde(default, alias = "kind")]
    pub kind: String,
    #[serde(default = "default_status", alias = "status")]
    pub status: String,
    #[serde(default, alias = "progress")]
    pub progress: i32,
    #[serde(
        default = "now_unix",
        alias = "created_at",
        serialize_with = "ser_f64_dotnet"
    )]
    pub created_at: f64,
    #[serde(
        default = "now_unix",
        alias = "updated_at",
        serialize_with = "ser_f64_dotnet"
    )]
    pub updated_at: f64,
    #[serde(default, alias = "request")]
    pub request: Map<String, Value>,
    #[serde(default, alias = "result")]
    pub result: Option<Map<String, Value>>,
    #[serde(default, alias = "error")]
    pub error: Option<Map<String, Value>>,
}

fn default_status() -> String {
    "queued".to_string()
}

/// `task_` + 16 hex chars (Store.cs:694).
pub fn new_task_id() -> String {
    format!("task_{}", random_hex_bytes(8))
}

impl BgTask {
    /// `BgTask.Finished` (Store.cs:703): `succeeded`/`failed`/`cancelled`.
    pub fn finished(&self) -> bool {
        matches!(self.status.as_str(), "succeeded" | "failed" | "cancelled")
    }
}

/// `TaskManager`'s storage half (Store.cs:706-842, persistence only).
#[derive(Debug)]
pub struct TaskStore {
    file: PathBuf,
    tasks: Vec<BgTask>,
}

impl TaskStore {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        TaskStore {
            file: file.into(),
            tasks: Vec::new(),
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `TaskManager.Load` (Store.cs:718-737): missing file → empty store;
    /// every unfinished task is marked `failed` with
    /// `{message: "interrupted by server restart", code: "interrupted"}`,
    /// then the store is persisted.
    pub fn load(&mut self) -> Result<(), StoreError> {
        if !self.file.exists() {
            return Ok(());
        }
        let text = read_file_to_string(&self.file)?;
        let list: Vec<BgTask> =
            serde_json::from_str(&text).map_err(|e| StoreError::json_path(&self.file, e))?;
        self.tasks.clear();
        let mut changed = false;
        for mut t in list {
            if !t.finished() {
                t.status = "failed".to_string();
                let mut err = Map::new();
                err.insert(
                    "message".to_string(),
                    Value::String("interrupted by server restart".to_string()),
                );
                err.insert("code".to_string(), Value::String("interrupted".to_string()));
                t.error = Some(err);
                changed = true;
            }
            if let Some(existing) = self.tasks.iter_mut().find(|x| x.id == t.id) {
                *existing = t;
            } else {
                self.tasks.push(t);
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    /// `TaskManager.Persist` (Store.cs:739-749): keep the last 500 tasks by
    /// `created_at`, indented JSON, atomic via `<file>.tmp`.
    pub fn persist(&self) -> Result<(), StoreError> {
        let mut tasks = self.tasks.clone();
        tasks.sort_by(|a, b| {
            a.created_at
                .partial_cmp(&b.created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let keep: Vec<&BgTask> = tasks.iter().rev().take(500).collect();
        let mut keep: Vec<BgTask> = keep.into_iter().cloned().collect();
        keep.reverse();
        let json = serde_json::to_string_pretty(&keep).map_err(StoreError::JsonPlain)?;
        let tmp_name = format!(
            "{}.tmp",
            self.file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "tasks.json".to_string())
        );
        write_atomic(&self.file, &json, &tmp_name)
    }

    /// `TaskManager.Get` (Store.cs:754): returns a snapshot (a clone), so
    /// callers cannot mutate the stored record in place.
    pub fn get(&self, id: &str) -> Option<BgTask> {
        self.tasks.iter().find(|t| t.id == id).cloned()
    }

    /// `TaskManager.List` (Store.cs:755-758): newest first, capped at `limit`.
    pub fn list(&self, limit: usize) -> Vec<BgTask> {
        let mut tasks = self.tasks.clone();
        tasks.sort_by(|a, b| {
            b.created_at
                .partial_cmp(&a.created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        tasks.truncate(limit);
        tasks
    }

    /// Create a record for a new background task (`TaskManager.Submit`'s
    /// record half, Store.cs:760-774): status `queued`, progress 0.
    pub fn create(&mut self, kind: &str, request: Map<String, Value>) -> Result<BgTask, StoreError> {
        let task = BgTask {
            id: new_task_id(),
            kind: kind.to_string(),
            status: "queued".to_string(),
            progress: 0,
            created_at: now_unix(),
            updated_at: now_unix(),
            request,
            result: None,
            error: None,
        };
        self.tasks.push(task.clone());
        self.persist()?;
        Ok(task)
    }

    fn mutate(&mut self, id: &str, f: impl FnOnce(&mut BgTask)) -> Result<Option<BgTask>, StoreError> {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return Ok(None);
        };
        f(task);
        task.updated_at = now_unix();
        let out = task.clone();
        self.persist()?;
        Ok(Some(out))
    }

    /// Mark a task `running` (the `Run` loop's first step, Store.cs:780).
    pub fn set_running(&mut self, id: &str) -> Result<Option<BgTask>, StoreError> {
        self.mutate(id, |t| t.status = "running".to_string())
    }

    /// Progress callback inside `Run` (Store.cs:788-795): progress only
    /// moves forward and is capped at 99 until the task finishes.
    pub fn set_progress(&mut self, id: &str, p: i32) -> Result<Option<BgTask>, StoreError> {
        self.mutate(id, |t| {
            t.progress = t.progress.max(p.min(99));
        })
    }

    /// Successful completion (Store.cs:799-806): result stored, status
    /// `succeeded`, progress 100.
    pub fn set_succeeded(
        &mut self,
        id: &str,
        result: Map<String, Value>,
    ) -> Result<Option<BgTask>, StoreError> {
        self.mutate(id, |t| {
            t.result = Some(result);
            t.status = "succeeded".to_string();
            t.progress = 100;
        })
    }

    /// Failure (Store.cs:810-826): `{message, code}` error object, status
    /// `failed`. `.NET` uses the `MuseException` code, else
    /// `"internal_error"`.
    pub fn set_failed(
        &mut self,
        id: &str,
        message: &str,
        code: &str,
    ) -> Result<Option<BgTask>, StoreError> {
        let message = message.to_string();
        let code = code.to_string();
        self.mutate(id, |t| {
            let mut err = Map::new();
            err.insert("message".to_string(), Value::String(message));
            err.insert("code".to_string(), Value::String(code));
            t.error = Some(err);
            t.status = "failed".to_string();
        })
    }

    /// `TaskManager.Cancel` (Store.cs:834-843) for a task that is not
    /// running under a live runner: marks it `cancelled` when it is not
    /// already finished. Returns `false` when the task does not exist or
    /// is already terminal. (Cancelling a live runner is `muse-agent`'s job.)
    pub fn cancel(&mut self, id: &str) -> Result<bool, StoreError> {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return Ok(false);
        };
        if task.finished() {
            return Ok(false);
        }
        task.status = "cancelled".to_string();
        task.updated_at = now_unix();
        self.persist()?;
        Ok(true)
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
    fn lifecycle_roundtrip() {
        let dir = tmpdir("tasks");
        let file = dir.join("tasks.json");
        let mut store = TaskStore::new(&file);
        let task = store.create("image", Map::new()).unwrap();
        assert_eq!(task.status, "queued");
        assert_eq!(task.progress, 0);
        assert!(task.id.starts_with("task_"));
        assert_eq!(task.id.len(), 5 + 16);
        assert!(!task.finished());
        store.set_running(&task.id).unwrap();
        store.set_progress(&task.id, 42).unwrap();
        // Progress never goes backwards and caps at 99.
        store.set_progress(&task.id, 10).unwrap();
        assert_eq!(store.get(&task.id).unwrap().progress, 42);
        store.set_progress(&task.id, 150).unwrap();
        assert_eq!(store.get(&task.id).unwrap().progress, 99);
        let mut result = Map::new();
        result.insert("url".into(), Value::String("https://x/y.png".into()));
        store.set_succeeded(&task.id, result).unwrap();
        let done = store.get(&task.id).unwrap();
        assert_eq!(done.status, "succeeded");
        assert_eq!(done.progress, 100);
        assert!(done.finished());
        assert_eq!(done.result.as_ref().unwrap()["url"], "https://x/y.png");

        // Reload: finished tasks stay finished; file has contract fields.
        let mut store2 = TaskStore::new(&file);
        store2.load().unwrap();
        let back = store2.get(&task.id).unwrap();
        assert_eq!(back.status, "succeeded");
        let text = std::fs::read_to_string(&file).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["Kind"], "image");
        assert_eq!(v[0]["Result"]["url"], "https://x/y.png");

        // Cancel of a terminal task is a no-op false.
        assert!(!store2.cancel(&task.id).unwrap());
    }

    #[test]
    fn load_marks_unfinished_interrupted() {
        let dir = tmpdir("tasks-interrupted");
        let file = dir.join("tasks.json");
        std::fs::write(
            &file,
            r#"[{"id": "task_0123456789abcdef", "kind": "video", "status": "running", "progress": 30, "created_at": 1761972245, "updated_at": 1761972245, "request": {}, "result": null, "error": null}]"#,
        )
        .unwrap();
        let mut store = TaskStore::new(&file);
        store.load().unwrap();
        let back = store.get("task_0123456789abcdef").unwrap();
        assert_eq!(back.status, "failed");
        let err = back.error.unwrap();
        assert_eq!(err["message"], "interrupted by server restart");
        assert_eq!(err["code"], "interrupted");
    }

    #[test]
    fn persist_keeps_last_500() {
        let dir = tmpdir("tasks-500");
        let file = dir.join("tasks.json");
        let mut store = TaskStore::new(&file);
        for i in 0..505 {
            let mut req = Map::new();
            req.insert("i".into(), Value::Number(i.into()));
            store.create("k", req).unwrap();
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 500);
    }
}
