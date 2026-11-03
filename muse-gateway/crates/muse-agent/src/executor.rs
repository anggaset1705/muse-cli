//! `ToolExecutor` — port of `Tools.cs`, `ToolEditing.cs`,
//! `FileReadVersions.cs` and the file/process dispatch in `ToolProcess.cs`
//! / `BackgroundProcesses.cs`.
//!
//! Every file path is jailed inside the workspace root, edits go through a
//! prepare → approve → apply pipeline with a stale-read guard, and all
//! changes are journaled so they can be reverted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::approval;
use crate::error::json::{bool_arg, int_arg, str_arg};
use crate::error::{AgentError, AgentResult, CancellationToken};use crate::parser::ToolCall;
use crate::processes::{self, ProcessManager};
use crate::screen::{ScreenDriver, SCREEN_UNAVAILABLE};
use crate::workspace;

/// Maximum text file size (`.NET MaxTextBytes` = 2 MiB).
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

/// Text encoding detected from (or written with) a BOM, mirroring .NET
/// `StreamReader(detectEncodingFromByteOrderMarks: true)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

impl Default for TextEncoding {
    fn default() -> Self {
        TextEncoding::Utf8
    }
}

/// Decode file bytes to text, detecting a BOM. Returns `(text, encoding)`.
pub fn decode_text(bytes: &[u8]) -> AgentResult<(String, TextEncoding)> {
    if bytes.len() > MAX_TEXT_BYTES as usize {
        return Err(AgentError::Invalid(
            "text file exceeds 2 MiB".to_string(),
        ));
    }
    let (text, encoding) = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (
            String::from_utf8(bytes[3..].to_vec()).map_err(|_| {
                AgentError::Invalid("file is not valid UTF-8 text".to_string())
            })?,
            TextEncoding::Utf8Bom,
        )
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        (decode_utf16(&bytes[2..], true)?, TextEncoding::Utf16Le)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (decode_utf16(&bytes[2..], false)?, TextEncoding::Utf16Be)
    } else {
        (
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                AgentError::Invalid("file is not valid UTF-8 text".to_string())
            })?,
            TextEncoding::Utf8,
        )
    };
    if text.contains('\0') {
        return Err(AgentError::Invalid(
            "binary files are not supported".to_string(),
        ));
    }
    if text.chars().count() as u64 > MAX_TEXT_BYTES {
        return Err(AgentError::Invalid(
            "text file exceeds 2 MiB".to_string(),
        ));
    }
    Ok((text, encoding))
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> AgentResult<String> {
    if bytes.len() % 2 != 0 {
        return Err(AgentError::Invalid(
            "file is not valid UTF-16 text".to_string(),
        ));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    let text: Result<String, _> = char::decode_utf16(units).collect();
    text.map_err(|_| AgentError::Invalid("file is not valid UTF-16 text".to_string()))
}

/// Encode text back to bytes with the file's original encoding/BOM.
pub fn encode_text(text: &str, encoding: TextEncoding) -> Vec<u8> {
    match encoding {
        TextEncoding::Utf8 => text.as_bytes().to_vec(),
        TextEncoding::Utf8Bom => {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(text.as_bytes());
            out
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let mut out = if encoding == TextEncoding::Utf16Le {
                vec![0xFF, 0xFE]
            } else {
                vec![0xFE, 0xFF]
            };
            for unit in text.encode_utf16() {
                out.extend_from_slice(
                    &(if encoding == TextEncoding::Utf16Le {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    }),
                );
            }
            out
        }
    }
}

/// A prepared tool call awaiting approval/execution (`.NET ToolApproval`).
/// For file writes/edits it carries the preview diff plus the before/after
/// text; for other calls only description + preview detail.
#[derive(Debug, Clone)]
pub struct ToolApproval {
    pub call: ToolCall,
    pub description: String,
    pub preview: String,
    pub file_path: Option<String>,
    pub original_text: Option<String>,
    pub new_text: Option<String>,
    pub file_encoding: Option<TextEncoding>,
}

/// One journaled workspace change (`.NET WorkspaceChange`).
#[derive(Debug, Clone)]
pub struct WorkspaceChange {
    /// Workspace-relative path (`/` separators).
    pub path: String,
    /// Content before the first change in this session (`None` = new file).
    pub original_text: Option<String>,
    /// Current content.
    pub current_text: String,
    pub file_encoding: Option<TextEncoding>,
}

impl WorkspaceChange {
    pub fn kind(&self) -> &'static str {
        if self.original_text.is_none() {
            "added"
        } else {
            "modified"
        }
    }

    pub fn diff(&self) -> String {
        crate::text_diff::create(&self.path, self.original_text.as_deref(), &self.current_text)
    }
}

struct ReadVersion {
    hash: String,
    ranges: Vec<(i64, i64)>,
    complete: bool,
}

/// The agent's tools. Every file path is jailed inside the workspace root.
pub struct ToolExecutor {
    root: PathBuf,
    pub shell: String,
    changes: Mutex<HashMap<String, WorkspaceChange>>,
    read_versions: Mutex<HashMap<String, ReadVersion>>,
    processes: ProcessManager,
    screen: Option<Arc<dyn ScreenDriver>>,
    last_file_change_applied: Mutex<bool>,
    last_command_changed_files: Mutex<Vec<String>>,
    last_computer_image: Mutex<Option<String>>,
}

impl ToolExecutor {
    /// Create an executor for `root` (must exist; symlinks are rejected,
    /// like .NET's constructor which throws when the root is missing or
    /// linked).
    pub fn new(root: impl AsRef<Path>, shell: &str) -> AgentResult<Self> {
        let root = root.as_ref();
        let canonical = std::fs::canonicalize(root).map_err(|_| {
            AgentError::Invalid(format!("workspace not found: {}", root.display()))
        })?;
        // .NET `RejectLinks(Root)`: the root itself must not be a link.
        if std::fs::symlink_metadata(root)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(AgentError::Invalid(format!(
                "symbolic links and junctions are not allowed: {}",
                root.display()
            )));
        }
        Ok(Self {
            root: canonical,
            shell: shell.to_string(),
            changes: Mutex::new(HashMap::new()),
            read_versions: Mutex::new(HashMap::new()),
            processes: ProcessManager::new(),
            screen: None,
            last_file_change_applied: Mutex::new(false),
            last_command_changed_files: Mutex::new(Vec::new()),
            last_computer_image: Mutex::new(None),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Attach the screen driver (`muse-screen`); `None` means computer
    /// tools report unavailable, like the gateway host.
    pub fn set_screen(&mut self, screen: Option<Arc<dyn ScreenDriver>>) {
        self.screen = screen;
    }

    pub fn screen_context(&self) -> String {
        match &self.screen {
            Some(driver) => driver.context(),
            None => "unavailable".to_string(),
        }
    }

    /// Language-aware [`ToolExecutor::screen_context`]: the chooser tab
    /// is named "Layar" for Bahasa Indonesia, "Screen" otherwise.
    pub fn screen_context_for(&self, reply_language: &str) -> String {
        match &self.screen {
            Some(driver) => driver.context_for(reply_language),
            None => "unavailable".to_string(),
        }
    }

    /// Resolve a workspace-relative (or absolute) path, jailing it.
    pub fn resolve_path(&self, path: &str) -> AgentResult<PathBuf> {
        workspace::resolve(&self.root, path)
    }

    pub fn last_file_change_applied(&self) -> bool {
        *self.last_file_change_applied.lock().unwrap()
    }

    pub fn last_command_changed_files(&self) -> Vec<String> {
        self.last_command_changed_files.lock().unwrap().clone()
    }

    pub fn last_computer_image(&self) -> Option<String> {
        self.last_computer_image.lock().unwrap().clone()
    }

    /// Reset per-turn state (`.NET BeginAgentTurn`): clear read versions
    /// and the last computer image.
    pub fn begin_agent_turn(&self) {
        self.read_versions.lock().unwrap().clear();
        *self.last_computer_image.lock().unwrap() = None;
    }

    /// All journaled changes, ordered by path.
    pub fn changes(&self) -> Vec<WorkspaceChange> {
        let guard = self.changes.lock().unwrap();
        let mut values: Vec<_> = guard.values().cloned().collect();
        values.sort_by(|a, b| a.path.cmp(&b.path));
        values
    }

    // -- text file helpers -------------------------------------------------

    /// Read a whole text file (`.NET ReadTextAsync`).
    pub async fn read_text_async(
        &self,
        path: &Path,
        _cancel: &CancellationToken,
    ) -> AgentResult<String> {
        let (text, _) = read_text_file_async(path).await?;
        Ok(text)
    }

    fn record_read(&self, full: &Path, text: &str, start: i64, end: i64, total: i64) {
        let hash = workspace::content_hash(text);
        let key = full.to_string_lossy().to_string();
        let mut guard = self.read_versions.lock().unwrap();
        let version = guard.entry(key).or_insert_with(|| ReadVersion {
            hash: hash.clone(),
            ranges: Vec::new(),
            complete: false,
        });
        if version.hash != hash {
            *version = ReadVersion {
                hash,
                ranges: Vec::new(),
                complete: false,
            };
        }
        version.ranges.push((start, end));
        let mut covered = 0i64;
        let mut ranges = version.ranges.clone();
        ranges.sort();
        for (s, e) in ranges {
            if s > covered + 1 {
                break;
            }
            covered = covered.max(e);
        }
        version.complete = covered >= total;
    }

    // -- dispatch ----------------------------------------------------------

    /// Execute one call end-to-end: prepare, then run. Non-cancellation
    /// failures come back as `"error: …"` strings (`.NET ExecuteAsync`).
    pub async fn execute(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
        output: Option<&mut (dyn FnMut(&str) + Send)>,
    ) -> Result<String, AgentError> {
        match self.prepare(call, cancel).await {
            Ok(prepared) => self.execute_prepared(&prepared, cancel, output).await,
            Err(AgentError::Cancelled) => Err(AgentError::Cancelled),
            Err(err) => Ok(format!("error: {err}")),
        }
    }

    /// Execute an already-prepared call (`.NET ExecutePreparedAsync`).
    pub async fn execute_prepared(
        &self,
        prepared: &ToolApproval,
        cancel: &CancellationToken,
        output: Option<&mut (dyn FnMut(&str) + Send)>,
    ) -> Result<String, AgentError> {
        cancel.throw_if_cancelled()?;
        let call = &prepared.call;
        let result = match call.name.as_str() {
            "list_files" => self.list_files(
                &str_arg(&call.args, "path", "."),
                bool_arg(&call.args, "recursive", false),
                int_arg(&call.args, "offset", 0),
                int_arg(&call.args, "limit", 200),
                cancel,
            ),
            "read_file" => {
                self.read_file(
                    &str_arg(&call.args, "path", ""),
                    int_arg(&call.args, "start_line", 1),
                    int_arg(&call.args, "max_lines", 1000),
                    cancel,
                    bool_arg(&call.args, "details", false),
                )
                .await
            }
            "read_files" => self.read_files(&call.args, cancel).await,
            "write_file" | "edit_file" => self.apply_prepared(prepared, cancel).await,
            "search_files" => self.search_files(&call.args, cancel).await,
            "git_status" => {
                processes::run_git(&["status", "--short", "--branch"], &self.root, cancel).await
            }
            "git_diff" => {
                processes::run_git(
                    &["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--", "."],
                    &self.root,
                    cancel,
                )
                .await
            }
            "run_command" if bool_arg(&call.args, "background", false) => {
                self.start_process(&call.args, cancel).await
            }
            "process_status" | "process_input" | "process_stop" => {
                self.processes.tool(&call.name, &call.args, cancel).await
            }
            "run_command" => {
                self.run_command(
                    &str_arg(&call.args, "command", ""),
                    &str_arg(&call.args, "shell", ""),
                    &str_arg(&call.args, "workdir", "."),
                    int_arg(&call.args, "timeout_seconds", 120),
                    cancel,
                    output,
                )
                .await
            }
            name if name.starts_with("computer_") => self.computer_async(call, cancel).await,
            "__parse_error__" => Ok(format!("error: {}", str_arg(&call.args, "error", ""))),
            _ => Ok(format!(
                "error: unknown tool '{}'. Available tools: list_files, read_file, search_files, write_file, edit_file, run_command, git_status, git_diff.",
                call.name
            )),
        };
        match result {
            Ok(text) => Ok(text),
            Err(AgentError::Cancelled) => Err(AgentError::Cancelled),
            Err(err) => Ok(format!("error: {err}")),
        }
    }

    /// Does this call need user confirmation in Ask mode
    /// (`.NET NeedsConfirmation`)?
    pub fn needs_confirmation(call: &ToolCall) -> bool {
        matches!(
            call.name.as_str(),
            "write_file" | "edit_file" | "run_command" | "process_input" | "process_stop"
        )
    }

    /// One-line human description of a call (`.NET Describe`).
    pub fn describe(call: &ToolCall) -> String {
        match call.name.as_str() {
            "write_file" => format!(
                "write_file {} ({} chars)",
                str_arg(&call.args, "path", ""),
                str_arg(&call.args, "content", "").chars().count()
            ),
            "edit_file" => format!("edit_file {}", str_arg(&call.args, "path", "")),
            "run_command" => format!("run_command: {}", str_arg(&call.args, "command", "")),
            _ => {
                let parts: Vec<String> = call
                    .args
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                format!("{} {}", call.name, parts.join(", "))
            }
        }
    }

    // -- file tools ---------------------------------------------------------

    fn list_files(
        &self,
        path: &str,
        recursive: bool,
        offset: i64,
        limit: i64,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        let dir = self.resolve_path(path)?;
        if !dir.is_dir() {
            return Ok(format!("error: directory not found: {path}"));
        }
        if offset < 0 || limit < 1 {
            return Ok("error: offset must be nonnegative and limit must be positive".to_string());
        }
        let limit = limit.min(1000);
        let found = workspace::enumerate(&dir, recursive, cancel)?;
        let page: Vec<&PathBuf> = found
            .iter()
            .skip(offset as usize)
            .take(limit as usize + 1)
            .collect();
        let mut lines: Vec<String> = Vec::new();
        for entry in page.iter().take(limit as usize) {
            let rel = workspace::relative_to(&self.root, entry);
            lines.push(if entry.is_dir() {
                format!("{rel}/")
            } else {
                rel
            });
        }
        let mut out = lines.join("\n");
        if page.len() > limit as usize {
            out += &format!(
                "\n[more entries; next_offset={}; use offset]",
                offset + limit
            );
        }
        Ok(out)
    }

    async fn read_file(
        &self,
        path: &str,
        start: i64,
        max_lines: i64,
        cancel: &CancellationToken,
        details: bool,
    ) -> AgentResult<String> {
        let full = self.resolve_path(path)?;
        if !full.is_file() {
            return Ok(format!("error: file not found: {path}"));
        }
        let text = self.read_text_async(&full, cancel).await?;
        if start < 1 || max_lines < 1 {
            return Ok("error: start_line and max_lines must be positive".to_string());
        }
        let lines: Vec<&str> = text.split('\n').collect();
        let total = lines.len() as i64;
        if start > total {
            return Ok(format!(
                "error: start_line {start} exceeds total lines {total}"
            ));
        }
        let mut output = String::new();
        let mut next = start;
        let take = max_lines.min(4000);
        for line in lines.iter().skip((start - 1) as usize).take(take as usize) {
            let rendered = if details {
                format!("{next:5}: {line}")
            } else {
                (*line).to_string()
            };
            if output.len() + rendered.len() > 48_000 {
                if next == start {
                    return Ok(format!(
                        "error: line {next} exceeds read page size; use search_files to narrow down the content"
                    ));
                }
                break;
            }
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&rendered);
            next += 1;
        }
        self.record_read(&full, &text, start, next - 1, total);
        let header = if details {
            format!(
                "File: {}\nLines: {}-{}/{}\ncontent_sha256: {}\n\n",
                workspace::relative_to(&self.root, &full),
                start,
                next - 1,
                total,
                workspace::content_hash(&text)
            )
        } else {
            String::new()
        };
        let mut result = header + &output;
        if next <= total {
            result += &format!(
                "\n[more lines; total={total}; next_start_line={next}; use start_line]"
            );
        }
        Ok(result)
    }

    async fn read_files(
        &self,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        let paths = match args.get("paths") {
            Some(Value::Array(paths)) if (1..=8).contains(&paths.len()) => paths.clone(),
            _ => return Ok("error: paths must contain 1 to 8 files".to_string()),
        };
        let mut output = String::new();
        for path in &paths {
            let name = match path {
                Value::String(s) => s.clone(),
                _ => return Ok("error: paths must be strings".to_string()),
            };
            let content = self
                .read_file(
                    &name,
                    int_arg(args, "start_line", 1),
                    int_arg(args, "max_lines", 300),
                    cancel,
                    true,
                )
                .await?;
            if output.len() + content.len() > 60_000 {
                output.push_str("\n[batch limit reached; read remaining files individually]\n");
                break;
            }
            output.push_str(&content);
            output.push_str("\n\n");
        }
        Ok(output)
    }

    async fn search_files(
        &self,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        let query = str_arg(args, "query", "");
        if query.is_empty() {
            return Ok("error: query must not be empty".to_string());
        }
        let path = str_arg(args, "path", ".");
        let scope = self.resolve_path(&path)?;
        let single_file = scope.is_file();
        if !single_file && !scope.is_dir() {
            return Ok(format!(
                "error: search path not found: {path}. Use list_files to locate the file or directory."
            ));
        }
        let files: Vec<PathBuf> = if single_file {
            vec![scope]
        } else {
            workspace::enumerate(&scope, true, cancel)?
                .into_iter()
                .filter(|p| p.is_file())
                .collect()
        };
        let case_sensitive = bool_arg(args, "case_sensitive", false);
        let needle = if case_sensitive {
            query.clone()
        } else {
            query.to_lowercase()
        };
        let mut matches: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        for file in files {
            cancel.throw_if_cancelled()?;
            if scanned >= 2000 || matches.len() >= 100 {
                break;
            }
            scanned += 1;
            let text = match self.read_text_async(&file, cancel).await {
                Ok(text) => text,
                Err(_) if !single_file => continue,
                Err(err) => return Err(err),
            };
            for (i, line) in text.split('\n').enumerate() {
                if matches.len() >= 100 {
                    break;
                }
                let haystack = if case_sensitive {
                    line.to_string()
                } else {
                    line.to_lowercase()
                };
                if haystack.contains(&needle) {
                    let rel = workspace::relative_to(&self.root, &file);
                    let shown = crate::error::json::truncate(
                        line.trim_end_matches('\r'),
                        300,
                    );
                    matches.push(format!("{}:{}: {}", rel, i + 1, shown));
                }
            }
        }
        if matches.is_empty() {
            Ok(format!("No matches (scanned {scanned} files)."))
        } else {
            Ok(format!(
                "{}\n[{} matches; limit 100; scanned {scanned} files]",
                matches.join("\n"),
                matches.len()
            ))
        }
    }
}

async fn read_text_file_async(path: &Path) -> AgentResult<(String, TextEncoding)> {
    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_TEXT_BYTES {
        return Err(AgentError::Invalid(
            "text file exceeds 2 MiB".to_string(),
        ));
    }
    let bytes = std::fs::read(path)?;
    decode_text(&bytes)
}

// ---------------------------------------------------------------------------
// Prepare / apply / revert (ToolEditing.cs + FileReadVersions.cs)
// ---------------------------------------------------------------------------

impl ToolExecutor {
    /// Build the approval preview for a call (`.NET PrepareAsync`).
    /// File writes/edits compute the after-text and diff; other calls get
    /// a description + detail preview.
    pub async fn prepare(
        &self,
        original: &ToolCall,
        cancel: &CancellationToken,
    ) -> AgentResult<ToolApproval> {
        cancel.throw_if_cancelled()?;
        let call = original.clone();
        if !matches!(call.name.as_str(), "write_file" | "edit_file") {
            let detail = if call.name == "run_command" {
                let workdir = self.resolve_path(&str_arg(&call.args, "workdir", "."))?;
                let (shell_file, _) =
                    crate::shell::resolve_shell(&str_arg(&call.args, "shell", ""), &self.shell, "")?;
                let timeout = int_arg(&call.args, "timeout_seconds", 120).clamp(1, 3600);
                format!(
                    "Workspace: {}\nShell: {shell_file}\nTimeout: {timeout}s\n\n{}\n\nThe shell command runs with your permissions and can access locations outside the project folder.",
                    workdir.display(),
                    str_arg(&call.args, "command", "")
                )
            } else {
                Self::describe(&call)
            };
            return Ok(ToolApproval {
                description: Self::describe(&call),
                preview: detail,
                call,
                file_path: None,
                original_text: None,
                new_text: None,
                file_encoding: None,
            });
        }
        let path = str_arg(&call.args, "path", "");
        if path.trim().is_empty() {
            return Err(AgentError::Invalid("path is required".to_string()));
        }
        let full = self.resolve_path(&path)?;
        let (before, encoding) = if full.is_file() {
            let (text, enc) = read_text_file_async(&full).await?;
            (Some(text), enc)
        } else {
            (None, TextEncoding::default())
        };
        let after: String;
        if call.name == "write_file" {
            match call.args.get("content") {
                Some(Value::String(content)) => after = content.clone(),
                _ => {
                    return Err(AgentError::Invalid(
                        "content must be a string".to_string(),
                    ))
                }
            }
        } else {
            let Some(before_text) = &before else {
                return Err(AgentError::Invalid(format!("file not found: {path}")));
            };
            after = self.apply_edits(before_text, &call.args)?;
        }
        if after.as_bytes().len() as u64 > MAX_TEXT_BYTES {
            return Err(AgentError::Invalid(
                "text file exceeds 2 MiB".to_string(),
            ));
        }
        let preview = crate::text_diff::create(&path, before.as_deref(), &after);
        Ok(ToolApproval {
            description: Self::describe(&call),
            preview,
            call,
            file_path: Some(full.to_string_lossy().to_string()),
            original_text: before,
            new_text: Some(after),
            file_encoding: Some(encoding),
        })
    }

    /// Apply the edit list of an `edit_file` call to `before`
    /// (`.NET PrepareAsync` edit loop): each `old_text` must match exactly
    /// once, matching LF-normalised text and mapping the range back to the
    /// original (CRLF-aware).
    fn apply_edits(&self, before: &str, args: &Map<String, Value>) -> AgentResult<String> {
        let edits: Vec<Map<String, Value>> = match args.get("edits") {
            Some(Value::Array(batch)) => batch
                .iter()
                .map(|item| match item {
                    Value::Object(map) => Ok(map.clone()),
                    _ => Err(AgentError::Invalid(
                        "each edit must be an object".to_string(),
                    )),
                })
                .collect::<AgentResult<Vec<_>>>()?,
            _ => vec![args.clone()],
        };
        if edits.is_empty() || edits.len() > 100 {
            return Err(AgentError::Invalid(
                "edits must contain 1 to 100 replacements".to_string(),
            ));
        }
        let mut after = before.to_string();
        for edit in &edits {
            let old = str_arg(edit, "old_text", &str_arg(edit, "search", ""));
            if old.is_empty() {
                return Err(AgentError::Invalid(
                    "old_text must not be empty".to_string(),
                ));
            }
            let replacement = match edit.get("new_text").or_else(|| edit.get("replace")) {
                Some(Value::String(s)) => s.clone(),
                _ => {
                    return Err(AgentError::Invalid(
                        "new_text must be a string".to_string(),
                    ))
                }
            };
            // Match normalised text exactly once and map its range back.
            let normalized = after.replace("\r\n", "\n");
            let old_norm = old.replace("\r\n", "\n");
            let Some(index) = normalized.find(&old_norm) else {
                return Err(AgentError::Invalid(
                    "old_text not found in file; read the latest content first".to_string(),
                ));
            };
            if normalized[index + old_norm.len()..].contains(&old_norm) {
                return Err(AgentError::Invalid(
                    "old_text matches more than once; make it more specific".to_string(),
                ));
            }
            let start = original_offset(&after, index);
            let end = original_offset(&after, index + old_norm.len());
            let newline = if after.contains("\r\n") { "\r\n" } else { "\n" };
            let replacement = replacement.replace("\r\n", "\n").replace('\n', newline);
            after = format!("{}{}{}", &after[..start], replacement, &after[end..]);
        }
        Ok(after)
    }

    /// Stale-read guard (`.NET PrepareForAgentAsync`): a write/edit is only
    /// allowed when the file was read in this turn and the snapshot matches
    /// the current content; whole-file replacement additionally requires a
    /// complete read and forbids large/partial replacements.
    pub async fn prepare_for_agent(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> AgentResult<ToolApproval> {
        let prepared = self.prepare(call, cancel).await?;
        if !matches!(call.name.as_str(), "write_file" | "edit_file")
            || prepared.original_text.is_none()
        {
            return Ok(prepared);
        }
        let original = prepared.original_text.clone().unwrap();
        let hash = workspace::content_hash(&original);
        {
            let guard = self.read_versions.lock().unwrap();
            let full = prepared.file_path.clone().unwrap();
            match guard.get(&full) {
                Some(version) if version.hash == hash => {
                    if call.name == "write_file" && !version.complete {
                        return Err(AgentError::Invalid(
                            "Replacing a whole file requires reading all its pages first. Use edit_file for a targeted change instead.".to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(AgentError::Invalid(
                        "Read the latest file with read_file before changing it; the prior snapshot is missing or stale.".to_string(),
                    ));
                }
            }
        }
        if call.name == "write_file" {
            let new_text = prepared.new_text.clone().unwrap();
            if original.len() > 48_000
                || (original.len() >= 1000 && new_text.len() < original.len() / 2)
            {
                return Err(AgentError::Invalid(
                    "A large or partial whole-file replacement is not allowed for agent writes. Use targeted edit_file replacements so unrelated code cannot be silently discarded.".to_string(),
                ));
            }
        }
        let expected = str_arg(&call.args, "expected_hash", "");
        if !expected.is_empty() && !hash.eq_ignore_ascii_case(&expected) {
            return Err(AgentError::Invalid(
                "expected_hash does not match the current file; read it again before editing."
                    .to_string(),
            ));
        }
        Ok(prepared)
    }

    /// Commit a prepared file change (`.NET ApplyPreparedAsync`): verify
    /// the file is unchanged since preview, write atomically via a temp
    /// file + rename, verify, and journal the change.
    async fn apply_prepared(
        &self,
        change: &ToolApproval,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        *self.last_file_change_applied.lock().unwrap() = false;
        let full = self.resolve_path(
            &change
                .file_path
                .clone()
                .ok_or_else(|| AgentError::Invalid("missing prepared file change".to_string()))?,
        )?;
        let current = if full.is_file() {
            Some(self.read_text_async(&full, cancel).await?)
        } else {
            None
        };
        if current != change.original_text {
            return Ok(
                "error: file changed after preview; read the file and request a new edit"
                    .to_string(),
            );
        }
        if current.as_deref() == change.new_text.as_deref() {
            return Ok(format!(
                "unchanged: requested content already matches {}",
                workspace::relative_to(&self.root, &full)
            ));
        }
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let new_text = change.new_text.clone().unwrap();
        let encoding = change.file_encoding.unwrap_or_default();
        let temp = full.with_file_name(format!(
            "{}.muse-{}.tmp",
            full.file_name().unwrap_or_default().to_string_lossy(),
            uuid::Uuid::new_v4().simple()
        ));
        let write_result: AgentResult<()> = async {
            std::fs::write(&temp, encode_text(&new_text, encoding))?;
            cancel.throw_if_cancelled()?;
            self.resolve_path(
                &change
                    .file_path
                    .clone()
                    .ok_or_else(|| AgentError::Invalid("missing prepared file change".to_string()))?,
            )?;
            let latest = if full.is_file() {
                Some(self.read_text_async(&full, cancel).await?)
            } else {
                None
            };
            if latest != change.original_text {
                return Err(AgentError::Invalid(
                    "file changed while applying edit; request a new preview".to_string(),
                ));
            }
            std::fs::rename(&temp, &full)?;
            Ok(())
        }
        .await;
        if std::fs::metadata(&temp).is_ok() {
            let _ = std::fs::remove_file(&temp);
        }
        write_result?;
        // Once committed, record the journal even if cancellation arrives now.
        let verified = self
            .read_text_async(&full, &CancellationToken::new())
            .await?;
        if verified != new_text {
            return Ok(
                "error: file changed immediately after saving; re-read it before continuing"
                    .to_string(),
            );
        }
        *self.last_file_change_applied.lock().unwrap() = true;
        let lines = verified.split('\n').count() as i64;
        self.record_read(&full, &verified, 1, lines, lines);
        let relative = workspace::relative_to(&self.root, &full);
        {
            let mut guard = self.changes.lock().unwrap();
            let original = guard
                .get(&relative)
                .and_then(|c| c.original_text.clone())
                .or_else(|| change.original_text.clone());
            if original.as_deref() == Some(new_text.as_str()) {
                guard.remove(&relative);
            } else {
                guard.insert(
                    relative.clone(),
                    WorkspaceChange {
                        path: relative.clone(),
                        original_text: original,
                        current_text: new_text.clone(),
                        file_encoding: change.file_encoding,
                    },
                );
            }
        }
        if change.call.name == "write_file" {
            Ok(format!("wrote {} ({} chars)", relative, new_text.chars().count()))
        } else {
            let count = match change.call.args.get("edits") {
                Some(Value::Array(edits)) => edits.len(),
                _ => 1,
            };
            Ok(format!("edited {relative} ({count} replacement(s))"))
        }
    }

    /// Undo one journaled change (`.NET RevertAsync`): refuses when the
    /// file changed outside the agent since the change was recorded.
    pub async fn revert(
        &self,
        path: &str,
        cancel: &CancellationToken,
    ) -> AgentResult<()> {
        let change = {
            let guard = self.changes.lock().unwrap();
            match guard.get(path) {
                Some(change) => change.clone(),
                None => {
                    return Err(AgentError::Invalid(format!(
                        "no tracked change for this file"
                    )))
                }
            }
        };
        let full = self.resolve_path(path)?;
        if !full.is_file()
            || self.read_text_async(&full, cancel).await? != change.current_text
        {
            return Err(AgentError::Invalid(
                "The file changed outside the agent. Reload and check before undoing."
                    .to_string(),
            ));
        }
        match &change.original_text {
            None => {
                cancel.throw_if_cancelled()?;
                std::fs::remove_file(&full)?;
            }
            Some(original) => {
                let temp = full.with_file_name(format!(
                    "{}.muse-undo-{}.tmp",
                    full.file_name().unwrap_or_default().to_string_lossy(),
                    uuid::Uuid::new_v4().simple()
                ));
                let result: AgentResult<()> = async {
                    std::fs::write(
                        &temp,
                        encode_text(original, change.file_encoding.unwrap_or_default()),
                    )?;
                    if self.read_text_async(&full, cancel).await? != change.current_text {
                        return Err(AgentError::Invalid(
                            "File changed while preparing undo; no overwrite was performed."
                                .to_string(),
                        ));
                    }
                    cancel.throw_if_cancelled()?;
                    self.resolve_path(path)?;
                    std::fs::rename(&temp, &full)?;
                    Ok(())
                }
                .await;
                if std::fs::metadata(&temp).is_ok() {
                    let _ = std::fs::remove_file(&temp);
                }
                result?;
            }
        }
        self.changes.lock().unwrap().remove(path);
        Ok(())
    }

    /// Workspace context block for the planner system prompt
    /// (`.NET WorkspaceContextAsync`).
    pub async fn workspace_context(&self, cancel: &CancellationToken) -> AgentResult<String> {
        let listing: Vec<String> = workspace::enumerate(&self.root, false, cancel)?
            .into_iter()
            .take(80)
            .map(|p| {
                let rel = workspace::relative_to(&self.root, &p);
                if p.is_dir() {
                    format!("{rel}/")
                } else {
                    rel
                }
            })
            .collect();
        let os = if cfg!(windows) {
            "Win32NT"
        } else if cfg!(target_os = "macos") {
            "MacOSX"
        } else {
            "Unix"
        };
        let mut context = format!(
            "Workspace: {}\nOS: {os}; shell: {}\nProject entries (generated folders omitted):\n{}\n",
            self.root.display(),
            self.shell,
            listing.join("\n")
        );
        context += &format!("\nComputer tools: {}\n", self.screen_context());
        let instructions = self.resolve_path("AGENTS.md")?;
        if instructions.is_file() {
            let text = self.read_text_async(&instructions, cancel).await?;
            context += &format!(
                "\nProject AGENTS.md:\n{}",
                crate::error::json::truncate(&text, 12_000)
            );
        }
        Ok(context)
    }

    // -- commands -----------------------------------------------------------

    /// Run a shell command and report which workspace files changed
    /// (`.NET RunCommandAsync`).
    async fn run_command(
        &self,
        command: &str,
        shell: &str,
        workdir: &str,
        timeout_seconds: i64,
        cancel: &CancellationToken,
        output: Option<&mut (dyn FnMut(&str) + Send)>,
    ) -> AgentResult<String> {
        if command.trim().is_empty() {
            return Ok("error: command must not be empty".to_string());
        }
        *self.last_command_changed_files.lock().unwrap() = Vec::new();
        let before = self.snapshot_workspace(cancel).await;
        let dir = self.resolve_path(workdir)?;
        let result =
            processes::run_shell(command, shell, &dir, timeout_seconds, cancel, output).await?;
        let after = self.snapshot_workspace(cancel).await;
        let mut changed: Vec<String> = before
            .files
            .keys()
            .filter(|path| {
                !self
                    .resolve_path(path)
                    .map(|full| full.is_file())
                    .unwrap_or(false)
                    || after
                        .files
                        .get(*path)
                        .map(|hash| hash != &before.files[*path])
                        .unwrap_or(false)
            })
            .cloned()
            .collect();
        if !before.truncated {
            changed.extend(
                after
                    .files
                    .keys()
                    .filter(|path| !before.files.contains_key(*path))
                    .cloned(),
            );
        }
        changed.sort();
        changed.dedup();
        changed.truncate(100);
        *self.last_command_changed_files.lock().unwrap() = changed.clone();
        if changed.is_empty() {
            Ok(result)
        } else {
            Ok(format!(
                "{result}\nworkspace_files_changed_during_command={}",
                serde_json::to_string(&changed).unwrap_or_else(|_| "[]".to_string())
            ))
        }
    }

    /// Hash the workspace (up to 2000 files / 64 MiB) for change detection.
    async fn snapshot_workspace(&self, cancel: &CancellationToken) -> WorkspaceSnapshot {
        let mut hashes: HashMap<String, String> = HashMap::new();
        let mut bytes: u64 = 0;
        let mut truncated = false;
        let files = match workspace::enumerate(&self.root, true, cancel) {
            Ok(files) => files,
            Err(_) => return WorkspaceSnapshot {
                files: hashes,
                truncated: true,
            },
        };
        for path in files.into_iter().filter(|p| p.is_file()) {
            if hashes.len() >= 2000 || bytes >= 64 * 1024 * 1024 {
                truncated = true;
                break;
            }
            let resolved = match self.resolve_path(&path.to_string_lossy()) {
                Ok(p) => p,
                Err(_) => {
                    truncated = true;
                    continue;
                }
            };
            let Ok(bytes_read) = std::fs::read(&resolved) else {
                truncated = true;
                continue;
            };
            if bytes_read.len() as u64 > MAX_TEXT_BYTES {
                truncated = true;
                continue;
            }
            bytes += bytes_read.len() as u64;
            hashes.insert(
                workspace::relative_to(&self.root, &resolved),
                workspace::content_hash_bytes(&bytes_read),
            );
        }
        WorkspaceSnapshot {
            files: hashes,
            truncated,
        }
    }

    async fn start_process(
        &self,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        let command = str_arg(args, "command", "");
        let shell = str_arg(args, "shell", "");
        let workdir = self.resolve_path(&str_arg(args, "workdir", "."))?;
        let timeout = int_arg(args, "timeout_seconds", 300);
        self.processes
            .start(&command, &shell, &workdir, timeout, cancel)
            .await
    }

    /// Delegate a `computer_*` call to the attached screen driver
    /// (`.NET ComputerAsync`).
    async fn computer_async(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        *self.last_computer_image.lock().unwrap() = None;
        let Some(screen) = &self.screen else {
            return Ok(SCREEN_UNAVAILABLE.to_string());
        };
        match screen.execute(call, cancel).await {
            Ok(result) => {
                *self.last_computer_image.lock().unwrap() = result.image_data_url;
                Ok(result.text)
            }
            Err(err) => Ok(format!("error: {err}")),
        }
    }

    /// Stop all background processes (`.NET StopBackgroundProcessesAsync`).
    pub async fn stop_background_processes(&self) {
        self.processes.stop_all().await;
    }
}

struct WorkspaceSnapshot {
    files: HashMap<String, String>,
    truncated: bool,
}

/// Map a normalised (LF) offset back to the original text's byte offset
/// (`.NET OriginalOffset`): each `\r\n` before the offset adds one byte.
fn original_offset(text: &str, normalized_offset: usize) -> usize {
    let mut position = 0usize;
    let bytes = text.as_bytes();
    let mut normalized = 0usize;
    while normalized < normalized_offset && position < bytes.len() {
        if bytes[position] == b'\r'
            && position + 1 < bytes.len()
            && bytes[position + 1] == b'\n'
        {
            position += 1;
        }
        position += 1;
        normalized += 1;
    }
    position
}

/// Whether the approval flow should auto-approve this call under Turbo
/// (`.NET AgentSession.ConfirmAsync` rule, shared with the direct-tool
/// 403 path in `AgentEndpoints.DirectTool`).
pub fn turbo_auto_approves(call: &ToolCall) -> bool {
    !approval::requires_explicit_approval(call)
}
