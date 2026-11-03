//! Integration tests for muse-agent: parser, executor files, processes,
//! approval Ask vs Turbo, sessions, and chat turns with scripted drivers.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use muse_agent::approval;
use muse_agent::chat_turn::{direct_tool_may_run, run_turn, TurnOptions};
use muse_agent::driver::{ChatRequest, DeltaSink, MockDriver, MuseDriver};
use muse_agent::error::{AgentError, AgentResult, CancellationToken};
use muse_agent::executor::ToolExecutor;
use muse_agent::parser::{self, ToolCall};
use muse_agent::session::{done_event, AgentEvent, AgentSession, AgentSessionManager};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_workspace(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "muse-agent-test-{}-{}-{}",
        std::process::id(),
        n,
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall::new(
        name,
        args.as_object().cloned().unwrap_or_default(),
    )
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[test]
fn parser_fence_json_actions_envelope() {
    let text = "Here is the plan:\n```json\n{\"actions\":[{\"action\":\"scan\",\"path\":\".\",\"recursive\":true},{\"action\":\"inspect_text\",\"path\":\"src/main.rs\"}]}\n```\n";
    let calls = parser::parse(text);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "list_files");
    assert_eq!(calls[0].args["path"], ".");
    assert_eq!(calls[0].args["recursive"], true);
    assert_eq!(calls[1].name, "read_file");
    assert_eq!(calls[1].args["path"], "src/main.rs");
}

#[test]
fn parser_fence_tool_legacy() {
    let text = "```tool\n{\"name\": \"readFile\", \"file_path\": \"a.txt\"}\n```";
    let calls = parser::parse(text);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].args["path"], "a.txt");
}

#[test]
fn parser_bare_json_tool_calls() {
    let text = "{\"tool_calls\":[{\"name\":\"write_file\",\"arguments\":{\"path\":\"a.txt\",\"content\":\"hi\"}}]}";
    let calls = parser::parse(text);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "write_file");
    assert_eq!(calls[0].args["content"], "hi");
}

#[test]
fn parser_bare_json_single_object_args() {
    let calls = parser::parse("{\"name\":\"list_files\",\"path\":\".\",\"recursive\":false}");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "list_files");
}

#[test]
fn parser_unknown_action_rejects_whole_batch() {
    let text = "```json\n{\"actions\":[{\"action\":\"scan\",\"path\":\".\"},{\"action\":\"explode\",\"path\":\".\"}]}\n```";
    let calls = parser::parse(text);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "__parse_error__");
    assert!(calls[0].error_message().contains("Unknown application action: explode"));
}

#[test]
fn parser_invalid_json_plan_is_parse_error() {
    let text = "```json\n{\"tool_calls\":[{\"name\":\"read_file\",}\n```";
    let calls = parser::parse(text);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "__parse_error__");
}

#[test]
fn parser_non_plan_text_is_ignored() {
    assert!(parser::parse("Hello, how are you?").is_empty());
    assert!(parser::parse("```python\nprint('hi')\n```").is_empty());
}

#[test]
fn parser_actions_arg_mapping() {
    let text = "```json\n{\"actions\":[{\"action\":\"save_text\",\"path\":\"a.txt\",\"text\":\"hello\"},{\"action\":\"replace_text\",\"path\":\"a.txt\",\"find\":\"x\",\"new_text\":\"y\"},{\"action\":\"terminal\",\"script\":\"echo hi\"}]}\n```";
    let calls = parser::parse(text);
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].name, "write_file");
    assert_eq!(calls[0].args["content"], "hello");
    assert_eq!(calls[1].name, "edit_file");
    assert_eq!(calls[1].args["old_text"], "x");
    assert_eq!(calls[2].name, "run_command");
    assert_eq!(calls[2].args["command"], "echo hi");
}

#[test]
fn parser_validate_unknown_tool() {
    let validated = parser::validate(&call("explode", json!({})));
    assert_eq!(validated.name, "__parse_error__");
    assert!(validated.error_message().contains("Unknown local tool: explode"));
    let ok = parser::validate(&call("listFiles", json!({"path": "."})));
    assert_eq!(ok.name, "list_files");
}

#[test]
fn parser_planner_reply_and_needs_input() {
    assert_eq!(
        parser::planner_reply("{\"status\":\"completed\",\"summary\":\"All done.\"}"),
        "All done."
    );
    assert!(parser::needs_input("{\"status\":\"needs_input\",\"reply\":\"Which file?\"}"));
    assert!(!parser::needs_input("{\"status\":\"completed\",\"summary\":\"Done.\"}"));
    assert_eq!(
        parser::strip_tool_blocks("Hello\n```json\n{\"tool_calls\":[{\"name\":\"list_files\",\"path\":\".\"}]}\n```\nbye"),
        "Hello\n\nbye"
    );
}

// ---------------------------------------------------------------------------
// Executor: files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executor_write_read_edit_revert() {
    let root = temp_workspace("files");
    std::fs::write(root.join("a.txt"), "line one\nline two\n").unwrap();
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();

    // read it first (records the read version for the stale-read guard)
    let read = ex
        .execute(&call("read_file", json!({"path": "a.txt"})), &cancel, None)
        .await
        .unwrap();
    assert!(read.contains("line one"), "{read}");

    // edit it
    let prepared = ex
        .prepare_for_agent(
            &call("edit_file", json!({"path": "a.txt", "old_text": "line two", "new_text": "line 2"})),
            &cancel,
        )
        .await
        .unwrap();
    let result = ex.execute_prepared(&prepared, &cancel, None).await.unwrap();
    assert!(result.starts_with("edited a.txt"), "{result}");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "line one\nline 2\n");

    // changes journal has one entry, kind modified
    let changes = ex.changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "a.txt");
    assert_eq!(changes[0].kind(), "modified");
    assert!(changes[0].diff().contains("-line two"));

    // revert restores the original content and clears the journal
    ex.revert("a.txt", &cancel).await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "line one\nline two\n");
    assert!(ex.changes().is_empty());

    // a file the agent created is deleted by revert (original is None)
    let prepared = ex
        .prepare_for_agent(&call("write_file", json!({"path": "new.txt", "content": "fresh\n"})), &cancel)
        .await
        .unwrap();
    let result = ex.execute_prepared(&prepared, &cancel, None).await.unwrap();
    assert!(result.starts_with("wrote new.txt"), "{result}");
    assert_eq!(ex.changes().len(), 1);
    assert_eq!(ex.changes()[0].kind(), "added");
    ex.revert("new.txt", &cancel).await.unwrap();
    assert!(!root.join("new.txt").exists());
    assert!(ex.changes().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn executor_rejects_path_outside_workspace() {
    let root = temp_workspace("jail");
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let result = ex
        .execute(
            &call("write_file", json!({"path": "../evil.txt", "content": "x"})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    assert!(result.starts_with("error: path is outside the workspace"), "{result}");
    assert!(!std::env::temp_dir().join("evil.txt").exists());
    let result = ex
        .execute(&call("read_file", json!({"path": "/etc/passwd"})), &cancel, None)
        .await
        .unwrap();
    assert!(result.starts_with("error: path is outside the workspace"), "{result}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn executor_rejects_symlink_escape() {
    let root = temp_workspace("links");
    let outside = temp_workspace("outside");
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let result = ex
        .execute(&call("read_file", json!({"path": "link.txt"})), &cancel, None)
        .await
        .unwrap();
    assert!(result.starts_with("error: symbolic links"), "{result}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn executor_stale_read_guard() {
    let root = temp_workspace("stale");
    std::fs::write(root.join("a.txt"), "v1\n").unwrap();
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();

    // Read v1, then change the file outside the agent.
    ex.execute(&call("read_file", json!({"path": "a.txt"})), &cancel, None)
        .await
        .unwrap();
    std::fs::write(root.join("a.txt"), "v2 changed outside\n").unwrap();

    // The stale snapshot must be rejected: old_text exists in the current
    // file, but the recorded read version is v1.
    let err = ex
        .prepare_for_agent(
            &call("edit_file", json!({"path": "a.txt", "old_text": "v2 changed outside", "new_text": "v3"})),
            &cancel,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stale"), "{err}");

    // Re-read, then the edit is allowed.
    ex.execute(&call("read_file", json!({"path": "a.txt"})), &cancel, None)
        .await
        .unwrap();
    let prepared = ex
        .prepare_for_agent(
            &call("edit_file", json!({"path": "a.txt", "old_text": "v2 changed outside", "new_text": "v3"})),
            &cancel,
        )
        .await
        .unwrap();
    let result = ex.execute_prepared(&prepared, &cancel, None).await.unwrap();
    assert!(result.starts_with("edited a.txt"), "{result}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn executor_whole_file_replace_needs_full_read() {
    let root = temp_workspace("fullread");
    let mut content = String::new();
    for i in 0..3000 {
        content.push_str(&format!("line {i}\n"));
    }
    std::fs::write(root.join("big.txt"), &content).unwrap();
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();

    // Read only the first page (max_lines 10) — not a complete read.
    ex.execute(
        &call("read_file", json!({"path": "big.txt", "max_lines": 10})),
        &cancel,
        None,
    )
    .await
    .unwrap();
    let err = ex
        .prepare_for_agent(
            &call("write_file", json!({"path": "big.txt", "content": "replaced\n"})),
            &cancel,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("reading all its pages"),
        "{err}"
    );

    // Read all pages, then the whole-file replacement is allowed.
    let mut start = 1;
    loop {
        let out = ex
            .execute(
                &call("read_file", json!({"path": "big.txt", "start_line": start, "max_lines": 1000})),
                &cancel,
                None,
            )
            .await
            .unwrap();
        if !out.contains("next_start_line=") {
            break;
        }
        start += 1000;
    }
    let prepared = ex
        .prepare_for_agent(
            &call("write_file", json!({"path": "big.txt", "content": content.clone()})),
            &cancel,
        )
        .await
        .unwrap();
    let result = ex.execute_prepared(&prepared, &cancel, None).await.unwrap();
    assert!(result.starts_with("unchanged:"), "{result}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn executor_list_and_search() {
    let root = temp_workspace("listsearch");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("b.txt"), "needle here\n").unwrap();
    std::fs::write(root.join("a.txt"), "nothing\n").unwrap();
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let listing = ex
        .execute(&call("list_files", json!({"path": ".", "recursive": true})), &cancel, None)
        .await
        .unwrap();
    assert!(listing.contains("a.txt"), "{listing}");
    assert!(listing.contains("sub/b.txt"), "{listing}");
    let found = ex
        .execute(&call("search_files", json!({"query": "needle"})), &cancel, None)
        .await
        .unwrap();
    assert!(found.contains("sub/b.txt:1: needle here"), "{found}");
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Executor: processes
// ---------------------------------------------------------------------------

fn process_id(result: &str) -> String {
    result
        .lines()
        .find(|l| l.starts_with("process_id="))
        .and_then(|l| l.strip_prefix("process_id="))
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn process_run_echo() {
    let root = temp_workspace("echo");
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let result = ex
        .execute(
            &call("run_command", json!({"command": "echo hello-world"})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    assert!(result.starts_with("exit_code=0"), "{result}");
    assert!(result.contains("hello-world"), "{result}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn process_run_command_reports_changed_files() {
    let root = temp_workspace("changed");
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let result = ex
        .execute(
            &call("run_command", json!({"command": "echo hi > made.txt"})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    assert!(result.contains("workspace_files_changed_during_command=[\"made.txt\"]"), "{result}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn process_background_poll_and_stop() {
    let root = temp_workspace("bg");
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let started = ex
        .execute(
            &call("run_command", json!({"command": "sleep 60", "background": true})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    assert!(started.contains("running=true"), "{started}");
    let id = process_id(&started);
    let status = ex
        .execute(
            &call("process_status", json!({"process_id": id, "wait_ms": 100})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    assert!(status.contains("running=true"), "{status}");
    let stopped = ex
        .execute(&call("process_stop", json!({"process_id": id})), &cancel, None)
        .await
        .unwrap();
    assert!(stopped.starts_with("stopped=true"), "{stopped}");
    assert!(stopped.contains("running=false"), "{stopped}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn process_background_input() {
    let root = temp_workspace("bginput");
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let started = ex
        .execute(
            &call("run_command", json!({"command": "cat", "background": true})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    let id = process_id(&started);
    ex.execute(
        &call("process_input", json!({"process_id": id, "text": "hello-stdin"})),
        &cancel,
        None,
    )
    .await
    .unwrap();
    let status = ex
        .execute(
            &call("process_status", json!({"process_id": id, "wait_ms": 2000})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    assert!(status.contains("hello-stdin"), "{status}");
    ex.execute(&call("process_stop", json!({"process_id": id})), &cancel, None)
        .await
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn process_run_command_timeout() {
    let root = temp_workspace("timeout");
    let ex = ToolExecutor::new(&root, "auto").unwrap();
    let cancel = CancellationToken::new();
    let result = ex
        .execute(
            &call("run_command", json!({"command": "sleep 30", "timeout_seconds": 1})),
            &cancel,
            None,
        )
        .await
        .unwrap();
    assert!(result.contains("error: command timed out after 1s and was killed"), "{result}");
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Approval: Ask vs Turbo
// ---------------------------------------------------------------------------

#[test]
fn approval_turbo_policy_protected_paths() {
    // Shell/process ops never need explicit approval, even in Turbo.
    assert!(!approval::requires_explicit_approval(&call("run_command", json!({"command": "ls"}))));
    assert!(!approval::requires_explicit_approval(&call("process_stop", json!({"process_id": "x"}))));
    // Plain reads never need approval.
    assert!(!approval::requires_explicit_approval(&call("read_file", json!({"path": "a.txt"}))));
    // Normal writes are auto-approved under Turbo.
    assert!(!approval::requires_explicit_approval(&call("write_file", json!({"path": "src/a.rs"}))));
    assert!(!approval::requires_explicit_approval(&call("edit_file", json!({"path": "README.md"}))));
    // Protected paths always need explicit approval.
    for path in [
        ".env",
        ".env.local",
        "AGENTS.md",
        "config/key.pem",
        "id.p12",
        "accounts.json",
        "api_key",
        ".github/workflows/ci.yml",
        ".git/config",
        "secrets/token.txt",
        "browser-sessions/a.json",
    ] {
        assert!(
            approval::requires_explicit_approval(&call("write_file", json!({"path": path}))),
            "{path} should require explicit approval"
        );
    }
}

#[test]
fn approval_direct_tool_gate() {
    let root = temp_workspace("gate");
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "English", false, None).unwrap();
    let write = call("write_file", json!({"path": "a.txt", "content": "x"}));
    let read = call("read_file", json!({"path": "a.txt"}));
    let env = call("write_file", json!({"path": ".env", "content": "x"}));

    // Ask mode: sensitive actions need approved=true.
    assert!(!direct_tool_may_run(&session, &write, false));
    assert!(direct_tool_may_run(&session, &write, true));
    assert!(direct_tool_may_run(&session, &read, false));

    // Turbo: normal writes pass, protected ones still need approval.
    session.turbo = true;
    assert!(direct_tool_may_run(&session, &write, false));
    assert!(!direct_tool_may_run(&session, &env, false));
    assert!(direct_tool_may_run(&session, &env, true));
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Sessions + views
// ---------------------------------------------------------------------------

#[test]
fn session_manager_create_list_remove() {
    let data = temp_workspace("mgr-data");
    let mut mgr = AgentSessionManager::new(Some(&data));
    let ws = temp_workspace("mgr-ws");
    let id = mgr
        .create(ws.to_str().unwrap(), None, None, None, None, None)
        .unwrap();
    assert_eq!(mgr.all().len(), 1);
    let view = mgr.get(&id).unwrap().view();
    assert_eq!(view["session_id"], id);
    assert_eq!(view["model"], "muse-chat");
    assert_eq!(view["turbo"], false);
    assert_eq!(view["running"], false);
    assert_eq!(view["history_count"], 0);
    assert!(view["goal"].is_null());
    let list = mgr.list_view();
    assert_eq!(list["object"], "list");
    assert_eq!(list["data"].as_array().unwrap().len(), 1);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(mgr.remove(&id)).unwrap();
    assert!(mgr.all().is_empty());
    assert!(mgr.get(&id).is_err());
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn session_changes_view_after_edit() {
    let root = temp_workspace("changes-view");
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let session =
            AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
        let cancel = CancellationToken::new();
        session
            .tools
            .execute(&call("read_file", json!({"path": "a.txt"})), &cancel, None)
            .await
            .unwrap();
        let prepared = session
            .tools
            .prepare_for_agent(
                &call("edit_file", json!({"path": "a.txt", "old_text": "one", "new_text": "two"})),
                &cancel,
            )
            .await
            .unwrap();
        session
            .tools
            .execute_prepared(&prepared, &cancel, None)
            .await
            .unwrap();
        let view = session.changes_view();
        assert_eq!(view["session_id"], session.id);
        let changes = view["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["path"], "a.txt");
        assert_eq!(changes[0]["kind"], "modified");
        assert!(changes[0]["diff"].as_str().unwrap().contains("-one"));
    });
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn sse_event_shapes() {
    let delta = AgentEvent::new("assistant_delta", "hel");
    assert_eq!(delta.to_sse(), "data: {\"event\":\"assistant_delta\",\"text\":\"hel\"}\n\n");
    let done = done_event(true, "completed", "Chat turn complete.", "abc123");
    assert_eq!(done["event"], "done");
    assert_eq!(done["ok"], true);
    assert_eq!(done["status"], "completed");
    assert_eq!(done["session_id"], "abc123");
    assert_eq!(done["summary"], "Chat turn complete.");
}

// ---------------------------------------------------------------------------
// Chat turns with scripted drivers
// ---------------------------------------------------------------------------

/// A driver that returns canned replies in order.
struct ScriptDriver {
    replies: Mutex<VecDeque<String>>,
}

impl ScriptDriver {
    fn new(replies: Vec<&str>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().map(|s| s.to_string()).collect()),
        }
    }
}

#[async_trait]
impl MuseDriver for ScriptDriver {
    async fn chat(
        &self,
        _req: ChatRequest,
        cancel: &CancellationToken,
        sink: &mut dyn DeltaSink,
    ) -> AgentResult<String> {
        cancel.throw_if_cancelled()?;
        let mut guard = self.replies.lock().await;
        let reply = guard.pop_front().unwrap_or_default();
        sink.on_delta(&reply);
        Ok(reply)
    }
}

#[tokio::test]
async fn chat_turn_plain_reply_completes() {
    let root = temp_workspace("turn-plain");
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    let driver = ScriptDriver::new(vec!["{\"status\":\"completed\",\"summary\":\"Hello there!\"}"]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "hello",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.status, "completed");
    assert_eq!(outcome.summary, "Chat turn complete.");
    assert!(events.iter().any(|e| e.kind == "assistant_delta"));
    assert!(events.iter().any(|e| e.kind == "assistant" && e.text == "Hello there!"));
    assert!(events.iter().any(|e| e.kind == "completed"));
    assert_eq!(session.history.len(), 2); // user + assistant
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_turbo_executes_write() {
    let root = temp_workspace("turn-turbo");
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    let driver = ScriptDriver::new(vec![
        "```json\n{\"tool_calls\":[{\"name\":\"write_file\",\"arguments\":{\"path\":\"hello.txt\",\"content\":\"hi there\"}}]}\n```",
        "{\"status\":\"completed\",\"summary\":\"Wrote hello.txt.\"}",
    ]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "create hello.txt",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.status, "completed");
    assert_eq!(std::fs::read_to_string(root.join("hello.txt")).unwrap(), "hi there");
    assert!(events.iter().any(|e| e.kind == "tool_start" && e.tool.as_deref() == Some("write_file")));
    assert!(events.iter().any(|e| e.kind == "tool_result"));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_ask_mode_declines_when_not_interactive() {
    let root = temp_workspace("turn-ask");
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "English", false, None).unwrap();
    let driver = ScriptDriver::new(vec![
        "```json\n{\"tool_calls\":[{\"name\":\"write_file\",\"arguments\":{\"path\":\"hello.txt\",\"content\":\"hi\"}}]}\n```",
    ]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "create hello.txt",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: false, // non-interactive Ask mode declines
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(!outcome.ok);
    assert_eq!(outcome.status, "declined");
    assert!(!root.join("hello.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_malformed_plan_three_strikes() {
    let root = temp_workspace("turn-bad");
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    let bad = "```json\n{\"tool_calls\":[{\"name\":\"read_file\",}\n```";
    let driver = ScriptDriver::new(vec![bad, bad, bad]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "read a file",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(!outcome.ok);
    assert_eq!(outcome.status, "incomplete");
    assert!(events.iter().filter(|e| e.kind == "protocol_error").count() >= 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_mock_driver_round_trip() {
    let root = temp_workspace("turn-mock");
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    let driver = MockDriver::new();
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "what time is it",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    // The mock reply is plain text, so the turn completes as discussion.
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.status, "completed");
    assert!(events
        .iter()
        .any(|e| e.kind == "assistant" && e.text.contains("[mock:muse-chat] You said: what time is it")));
    let deltas: String = events
        .iter()
        .filter(|e| e.kind == "assistant_delta")
        .map(|e| e.text.clone())
        .collect();
    assert!(deltas.contains("You said: what time is it"), "{deltas}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_cancel_stops_run() {
    let root = temp_workspace("turn-cancel");
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    // A driver that never answers until cancelled.
    struct HangingDriver;
    #[async_trait]
    impl MuseDriver for HangingDriver {
        async fn chat(
            &self,
            _req: ChatRequest,
            cancel: &CancellationToken,
            _sink: &mut dyn DeltaSink,
        ) -> AgentResult<String> {
            cancel.cancelled().await;
            Err(AgentError::Cancelled)
        }
    }
    let driver = HangingDriver;
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancel2.cancel();
    });
    let mut events: Vec<AgentEvent> = Vec::new();
    let outcome = run_turn(
        &mut session,
        "hello",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert_eq!(outcome.status, "cancelled");
    assert!(!outcome.ok);
    assert!(events.iter().any(|e| e.kind == "cancelled"));
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Small unit checks
// ---------------------------------------------------------------------------

#[test]
fn tool_outcome_classification() {
    assert!(muse_agent::tool_outcome::is_running("process_id=abc\nrunning=true\noutput (latest 32000 chars):\n"));
    assert!(muse_agent::tool_outcome::exited_successfully("exit_code=0\nstdout:\nok\nstderr:\n"));
    assert!(muse_agent::tool_outcome::is_failure("error: boom"));
    assert!(muse_agent::tool_outcome::is_failure("exit_code=1\nstdout:\n"));
    assert!(!muse_agent::tool_outcome::is_failure("stopped=true\nexit_code=-1\n"));
    assert!(muse_agent::tool_outcome::is_failure("exit_code=0\ntimed_out=true\n"));
}

#[test]
fn text_diff_single_hunk() {
    let diff = muse_agent::text_diff::create("a.txt", Some("one\ntwo\nthree\n"), "one\nTWO\nthree\n");
    assert!(diff.contains("--- a/a.txt"));
    assert!(diff.contains("+++ b/a.txt"));
    assert!(diff.contains("-two"));
    assert!(diff.contains("+TWO"));
    assert_eq!(muse_agent::text_diff::create("a.txt", Some("same\n"), "same\n"), "(no changes)");
    let added = muse_agent::text_diff::create("new.txt", None, "hello\n");
    assert!(added.contains("--- /dev/null"));
}

#[test]
fn system_prompt_mentions_protocol() {
    let prompt = muse_agent::system_prompt::system_prompt_for("Bahasa Indonesia");
    assert!(prompt.contains("Bahasa Indonesia"));
    assert!(prompt.contains("```json"));
    assert!(prompt.contains("read_file"));
    assert!(!prompt.contains("__REPLY_LANGUAGE__"));
}

#[test]
fn system_prompt_names_the_screen_chooser_in_the_reply_language() {
    // The old upstream Vietnamese chooser name, spelled with escapes
    // so the literal never appears in source.
    const FORBIDDEN: &str = "M\u{e0}n h\u{ec}nh";
    let id = muse_agent::system_prompt::system_prompt_for("Bahasa Indonesia");
    assert!(id.contains("Layar chooser"), "{id}");
    assert!(!id.contains(FORBIDDEN), "{id}");
    assert!(!id.contains("__SCREEN_CHOOSER__"), "{id}");
    // Launch-first rule and preferred_hint are documented for the agent.
    assert!(id.contains("start https://shellauction.net"), "{id}");
    assert!(id.contains("preferred_hint"), "{id}");
    assert!(id.contains("NOT open at all yet"), "{id}");

    let en = muse_agent::system_prompt::system_prompt_for("English");
    assert!(en.contains("Screen chooser"), "{en}");
    assert!(!en.contains(FORBIDDEN), "{en}");
    assert!(!en.contains("Layar"), "{en}");
    assert!(!en.contains("__SCREEN_CHOOSER__"), "{en}");
}

#[test]
fn goal_instructions_carry_the_launch_first_rule() {
    // The old upstream Vietnamese chooser name, spelled with escapes
    // so the literal never appears in source.
    const FORBIDDEN: &str = "M\u{e0}n h\u{ec}nh";
    let instructions = muse_agent::goal::GoalRun::instructions_for("Bahasa Indonesia");
    assert!(instructions.contains("start <url>"), "{instructions}");
    assert!(instructions.contains("auto-selected"), "{instructions}");
    assert!(!instructions.contains(FORBIDDEN), "{instructions}");
}

#[test]
fn approval_flow_via_session_registry() {
    let root = temp_workspace("registry");
    let session = AgentSession::new(&root, "auto", "muse-chat", "English", false, None).unwrap();
    // Unknown approval ids are not answerable.
    assert!(!session.approve("nope", true));
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Skill @alias wiring (.NET AgentLoop + Skills.Expand parity)
// ---------------------------------------------------------------------------

/// A driver that records every planner request and returns canned replies.
struct RecordingDriver {
    replies: Mutex<VecDeque<String>>,
    captured: Arc<Mutex<Vec<ChatRequest>>>,
}

impl RecordingDriver {
    fn new(replies: Vec<&str>) -> (Self, Arc<Mutex<Vec<ChatRequest>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                replies: Mutex::new(replies.into_iter().map(|s| s.to_string()).collect()),
                captured: Arc::clone(&captured),
            },
            captured,
        )
    }
}

#[async_trait]
impl MuseDriver for RecordingDriver {
    async fn chat(
        &self,
        req: ChatRequest,
        cancel: &CancellationToken,
        sink: &mut dyn DeltaSink,
    ) -> AgentResult<String> {
        cancel.throw_if_cancelled()?;
        self.captured.lock().await.push(req);
        let mut guard = self.replies.lock().await;
        let reply = guard.pop_front().unwrap_or_default();
        sink.on_delta(&reply);
        Ok(reply)
    }
}

fn write_skill_file(dir: &Path, file: &str, text: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file), text).unwrap();
}

/// The APP_STATE packet the turn sends to the driver (last message).
fn app_state_of(req: &ChatRequest) -> Value {
    let last = req.messages.last().expect("APP_STATE message");
    let content = last["content"].as_str().expect("APP_STATE text");
    let json = content
        .strip_prefix("APP_STATE:\n")
        .expect("APP_STATE prefix");
    serde_json::from_str(json).expect("APP_STATE json")
}

const PLAIN_DONE: &str = "{\"status\":\"completed\",\"summary\":\"Done.\"}";

#[tokio::test]
async fn chat_turn_skill_alias_applied() {
    let root = temp_workspace("turn-skill");
    let skills = root.join("skills");
    write_skill_file(
        &skills,
        "fe.md",
        "---\nname: Frontend\ndescription: Build UIs\n---\nFollow the design system.\n",
    );
    let mut session =
        AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    session.skills_directory = Some(skills);
    let (driver, captured) = RecordingDriver::new(vec![PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "use @fe please",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert!(
        events
            .iter()
            .any(|e| e.kind == "skill" && e.text == "Applying skills: @fe"),
        "events: {events:?}"
    );
    let reqs = captured.lock().await;
    assert_eq!(reqs.len(), 1);
    let system = reqs[0].messages[0]["content"].as_str().unwrap();
    assert!(system.contains("### Skill @fe — Frontend"), "{system}");
    assert!(system.contains("Follow the design system."));
    // .NET order: system prompt + context, then the skill block, then
    // CURRENT LOCAL TASK.
    let skill_at = system.find("USER SKILLS REQUESTED").expect("skill block");
    let task_at = system.find("CURRENT LOCAL TASK").expect("task marker");
    assert!(skill_at < task_at);
    assert_eq!(app_state_of(&reqs[0])["active_skills"], json!(["fe"]));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_skill_multiple_aliases_joined() {
    let root = temp_workspace("turn-skill-multi");
    let skills = root.join("skills");
    write_skill_file(
        &skills,
        "fe.md",
        "---\nname: Frontend\n---\nFollow the design system.\n",
    );
    write_skill_file(&skills, "seo.md", "# SEO\nRank the page.\n");
    let mut session =
        AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    session.skills_directory = Some(skills);
    let (driver, captured) = RecordingDriver::new(vec![PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "use @fe and @seo please",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert!(
        events
            .iter()
            .any(|e| e.kind == "skill" && e.text == "Applying skills: @fe, @seo"),
        "events: {events:?}"
    );
    let reqs = captured.lock().await;
    assert_eq!(
        app_state_of(&reqs[0])["active_skills"],
        json!(["fe", "seo"])
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_skill_disabled_skipped_silently() {
    let root = temp_workspace("turn-skill-disabled");
    let skills = root.join("skills");
    write_skill_file(
        &skills,
        "fe.md",
        "---\nname: Frontend\n---\nFollow the design system.\n",
    );
    muse_store::SkillStore::new(&skills)
        .unwrap()
        .set_enabled("fe", false)
        .unwrap();
    let mut session =
        AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    session.skills_directory = Some(skills);
    let (driver, captured) = RecordingDriver::new(vec![PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "use @fe please",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert!(
        !events.iter().any(|e| e.kind == "skill"),
        "events: {events:?}"
    );
    let reqs = captured.lock().await;
    let system = reqs[0].messages[0]["content"].as_str().unwrap();
    assert!(!system.contains("Follow the design system."));
    assert_eq!(app_state_of(&reqs[0])["active_skills"], json!([]));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_skill_unknown_alias_and_email_silent() {
    let root = temp_workspace("turn-skill-unknown");
    let skills = root.join("skills");
    write_skill_file(
        &skills,
        "fe.md",
        "---\nname: Frontend\n---\nFollow the design system.\n",
    );
    let mut session =
        AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    session.skills_directory = Some(skills);
    let (driver, captured) = RecordingDriver::new(vec![PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "ask @nope and mail a@b.com about it",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert!(
        !events.iter().any(|e| e.kind == "skill"),
        "events: {events:?}"
    );
    let reqs = captured.lock().await;
    assert_eq!(app_state_of(&reqs[0])["active_skills"], json!([]));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_skill_observation_indonesian() {
    let root = temp_workspace("turn-skill-id");
    let skills = root.join("skills");
    write_skill_file(
        &skills,
        "fe.md",
        "---\nname: Frontend\n---\nFollow the design system.\n",
    );
    let mut session = AgentSession::new(&root, "auto", "muse-chat", "Bahasa Indonesia", true, None)
        .unwrap();
    session.skills_directory = Some(skills);
    let (driver, _captured) = RecordingDriver::new(vec![PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "use @fe please",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert!(
        events
            .iter()
            .any(|e| e.kind == "skill" && e.text == "Menerapkan skill: @fe"),
        "events: {events:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_skill_goal_turn_scans_task_prompt() {
    let root = temp_workspace("turn-skill-goal");
    let skills = root.join("skills");
    write_skill_file(
        &skills,
        "fe.md",
        "---\nname: Frontend\n---\nFollow the design system.\n",
    );
    let mut session =
        AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    session.skills_directory = Some(skills);
    // A /goal turn re-prompts on missing verification, so give it two
    // plain replies; the skill block must already be in the first one.
    let (driver, captured) = RecordingDriver::new(vec![PLAIN_DONE, PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let _outcome = run_turn(
        &mut session,
        "/goal build the page using @fe",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(
        events
            .iter()
            .any(|e| e.kind == "skill" && e.text == "Applying skills: @fe"),
        "events: {events:?}"
    );
    let reqs = captured.lock().await;
    assert!(!reqs.is_empty());
    let system = reqs[0].messages[0]["content"].as_str().unwrap();
    assert!(system.contains("### Skill @fe — Frontend"), "{system}");
    assert_eq!(app_state_of(&reqs[0])["active_skills"], json!(["fe"]));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_skill_imported_between_turns_is_visible() {
    let root = temp_workspace("turn-skill-fresh");
    let skills = root.join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    let mut session =
        AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    session.skills_directory = Some(skills.clone());
    let (driver, captured) = RecordingDriver::new(vec![PLAIN_DONE, PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let options = TurnOptions {
        interactive: true,
        ..Default::default()
    };
    let first = run_turn(
        &mut session,
        "use @fe please",
        vec![],
        &driver,
        &mut events,
        options.clone(),
        &cancel,
    )
    .await;
    assert!(first.ok, "{first:?}");
    assert!(
        !events.iter().any(|e| e.kind == "skill"),
        "no skill yet: {events:?}"
    );
    // Import the skill between turns: the next turn must see it.
    write_skill_file(
        &skills,
        "fe.md",
        "---\nname: Frontend\n---\nFollow the design system.\n",
    );
    let second = run_turn(
        &mut session,
        "use @fe please",
        vec![],
        &driver,
        &mut events,
        options,
        &cancel,
    )
    .await;
    assert!(second.ok, "{second:?}");
    assert_eq!(
        events.iter().filter(|e| e.kind == "skill").count(),
        1,
        "events: {events:?}"
    );
    let reqs = captured.lock().await;
    assert_eq!(reqs.len(), 2);
    assert_eq!(app_state_of(&reqs[0])["active_skills"], json!([]));
    assert_eq!(app_state_of(&reqs[1])["active_skills"], json!(["fe"]));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chat_turn_skill_long_content_truncated() {
    let root = temp_workspace("turn-skill-trunc");
    let skills = root.join("skills");
    let long = "x".repeat(13_000);
    write_skill_file(&skills, "big.md", &format!("# Big\n{long}"));
    let mut session =
        AgentSession::new(&root, "auto", "muse-chat", "English", true, None).unwrap();
    session.skills_directory = Some(skills);
    let (driver, captured) = RecordingDriver::new(vec![PLAIN_DONE]);
    let mut events: Vec<AgentEvent> = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = run_turn(
        &mut session,
        "use @big",
        vec![],
        &driver,
        &mut events,
        TurnOptions {
            interactive: true,
            ..Default::default()
        },
        &cancel,
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    assert!(
        events
            .iter()
            .any(|e| e.kind == "skill" && e.text == "Applying skills: @big"),
        "events: {events:?}"
    );
    let reqs = captured.lock().await;
    let system = reqs[0].messages[0]["content"].as_str().unwrap();
    assert!(system.contains("[... skill content truncated ...]"));
    assert_eq!(app_state_of(&reqs[0])["active_skills"], json!(["big"]));
    let _ = std::fs::remove_dir_all(&root);
}
