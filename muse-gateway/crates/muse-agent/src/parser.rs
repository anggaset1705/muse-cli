//! `ToolCallParser` — port of `ToolCallParser.cs` (205 lines).
//!
//! Parses the planner JSON Muse sends (```json / ```tool code fences, or a
//! bare JSON object/array) into [`ToolCall`]s, normalises legacy/camelCase
//! names and argument aliases, and maps the wire `actions` envelope
//! (`scan`, `inspect_text`, …) to internal tool names exactly as
//! CONTRACTS.md §8 / `ToolCallParser.cs:142-147` require.
//!
//! Failure policy (mirrors .NET): if ANY block/action in a reply is
//! invalid, the whole batch is rejected — only `__parse_error__` calls are
//! returned, and nothing is executed.

use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

use crate::error::json::{bool_arg, int_arg, str_arg};

/// One parsed tool call. `args` is the JSON object minus the `name` field.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub args: Map<String, Value>,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, args: Map<String, Value>) -> Self {
        Self {
            name: name.into(),
            args,
        }
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        let mut args = Map::new();
        args.insert(
            "error".to_string(),
            Value::String(message.into()),
        );
        Self {
            name: "__parse_error__".to_string(),
            args,
        }
    }

    pub fn error_message(&self) -> String {
        str_arg(&self.args, "error", "")
    }
}

/// Internal tool names the executor understands (`.NET Names`, case-sensitive).
pub const KNOWN_TOOLS: &[&str] = &[
    "read_file",
    "read_files",
    "list_files",
    "search_files",
    "write_file",
    "edit_file",
    "run_command",
    "process_status",
    "process_input",
    "process_stop",
    "git_status",
    "git_diff",
    "computer_list_windows",
    "computer_inspect",
    "computer_screenshot",
    "computer_click",
    "computer_drag",
    "computer_type",
    "computer_key",
    "computer_scroll",
    "computer_invoke",
    "computer_set_value",
    "computer_focus",
    "computer_assert",
    "computer_toggle",
    "computer_select",
    "computer_expand",
];

fn is_known(name: &str) -> bool {
    KNOWN_TOOLS.contains(&name)
}

/// Legacy/camelCase aliases (`.NET Aliases`, case-insensitive keys).
fn alias_map() -> &'static [(&'static str, &'static str)] {
    &[
        ("readFile", "read_file"),
        ("writeFile", "write_file"),
        ("editFile", "edit_file"),
        ("list_directory", "list_files"),
        ("listFiles", "list_files"),
        ("searchFiles", "search_files"),
        ("execute_command", "run_command"),
        ("run_terminal_cmd", "run_command"),
        ("runCommand", "run_command"),
        ("run_cmd", "run_command"),
        ("run_powershell", "run_command"),
        ("replace_in_file", "edit_file"),
        ("search_replace", "edit_file"),
    ]
}

/// Wire `actions` envelope names → internal names (CONTRACTS.md §8).
fn action_map(action: &str) -> Option<&'static str> {
    Some(match action {
        "scan" => "list_files",
        "inspect_text" => "read_file",
        "inspect_texts" => "read_files",
        "find_text" => "search_files",
        "replace_text" => "edit_file",
        "save_text" => "write_file",
        "terminal" => "run_command",
        "poll_process" => "process_status",
        "send_process_input" => "process_input",
        "stop_process" => "process_stop",
        "ui_tree" => "computer_inspect",
        "ui_image" => "computer_screenshot",
        "ui_press" => "computer_invoke",
        "ui_fill" => "computer_set_value",
        "ui_type" => "computer_type",
        "ui_key" => "computer_key",
        "ui_click" => "computer_click",
        "ui_drag" => "computer_drag",
        "ui_focus" => "computer_focus",
        "ui_scroll" => "computer_scroll",
        "ui_assert" => "computer_assert",
        "ui_toggle" => "computer_toggle",
        "ui_select" => "computer_select",
        "ui_expand" => "computer_expand",
        _ => return None,
    })
}

/// Argument renames applied by `.NET Normalize`
/// (`(source, target)`; only when the target is absent).
const ARG_RENAMES: &[(&str, &str)] = &[
    ("file_path", "path"),
    ("directory", "path"),
    ("cmd", "command"),
    ("working_directory", "workdir"),
    ("cwd", "workdir"),
    ("search", "old_text"),
    ("old_string", "old_text"),
    ("replace", "new_text"),
    ("new_string", "new_text"),
    ("new_content", "content"),
    ("expected_sha256", "expected_hash"),
];

/// Normalise one call: alias the name, lowercase it when it matches a
/// known tool case-insensitively, rename argument keys, default the shell
/// for `run_cmd` / `run_powershell`.
pub fn normalize(call: &ToolCall) -> ToolCall {
    let mut name = call.name.clone();
    for (alias, target) in alias_map() {
        if alias.eq_ignore_ascii_case(&name) {
            name = (*target).to_string();
            break;
        }
    }
    let lowered = name.to_lowercase();
    if is_known(&lowered) {
        name = lowered;
    }
    let mut args = call.args.clone();
    if !args.contains_key("shell") && call.name.eq_ignore_ascii_case("run_cmd") {
        args.insert("shell".to_string(), Value::String("cmd".to_string()));
    }
    if !args.contains_key("shell") && call.name.eq_ignore_ascii_case("run_powershell") {
        args.insert(
            "shell".to_string(),
            Value::String("powershell".to_string()),
        );
    }
    for (source, target) in ARG_RENAMES {
        if !args.contains_key(*target) {
            if let Some(value) = args.get(*source).cloned() {
                args.insert((*target).to_string(), value);
            }
        }
    }
    ToolCall { name, args }
}

/// Validate one call: known names pass through, anything else becomes a
/// `__parse_error__` (`.NET Validate`). `__parse_error__` passes unchanged.
pub fn validate(call: &ToolCall) -> ToolCall {
    if call.name == "__parse_error__" {
        return call.clone();
    }
    let normalized = normalize(call);
    if is_known(&normalized.name) {
        normalized
    } else {
        ToolCall::parse_error(format!(
            "Unknown local tool: {}. No actions were executed.",
            normalized.name
        ))
    }
}

fn fences_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t]*(?:```|~~~)(?P<language>[^\r\n]*)\r?\n(?P<body>[\s\S]*?)(?:^[ \t]*(?:```|~~~)[ \t]*$|\z)",
        )
        .expect("fences regex")
    })
}

fn incomplete_tool_block_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^\s*```tool\s*$").expect("incomplete tool regex"))
}

fn looks_like_plan_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let mut names: Vec<String> = KNOWN_TOOLS.iter().map(|s| regex::escape(s)).collect();
        for (alias, _) in alias_map() {
            names.push(regex::escape(alias));
        }
        Regex::new(&format!(
            r#""(?:name|tool)"\s*:\s*"(?:{names})""#,
            names = names.join("|")
        ))
        .expect("looks-like-plan regex")
    })
}

fn arguments_name_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#""(?:name|tool|function)"\s*:"#).expect("arguments name regex")
    })
}

fn json_error(err: &str) -> ToolCall {
    ToolCall::parse_error(format!(
        "Invalid local plan JSON: {err} No actions from this response were executed. \
         Return ONE action inside a ```json code fence. JSON-escape source-code quotes (\\\"), \
         backslashes (\\\\), and newlines (\\n); raw JSON outside a code fence can lose escapes \
         during Markdown rendering. Use cwd=\".\" and relative paths or forward slashes."
    ))
}

/// Does this text look like a planner payload (used to decide whether a
/// JSON syntax error is worth reporting instead of silently ignoring)?
fn looks_like_plan(text: &str) -> bool {
    text.contains("\"tool_calls\"")
        || text.contains("\"actions\"")
        || looks_like_plan_regex().is_match(text)
        || (text.contains("\"arguments\"") && arguments_name_regex().is_match(text))
}

/// Reject the entire response if any block/action is invalid (`.NET Validated`).
fn validated(calls: Vec<ToolCall>) -> Vec<ToolCall> {
    if calls.iter().any(|c| c.name == "__parse_error__") {
        calls
            .into_iter()
            .filter(|c| c.name == "__parse_error__")
            .collect()
    } else {
        calls
    }
}

/// Parse all tool calls out of one model reply.
pub fn parse(text: &str) -> Vec<ToolCall> {
    let mut calls: Vec<ToolCall> = Vec::new();
    for fence in fences_regex().captures_iter(text) {
        let language = fence
            .name("language")
            .map(|m| m.as_str().trim().to_lowercase())
            .unwrap_or_default();
        if language != "tool" && language != "json" && !language.is_empty() {
            continue;
        }
        let body = fence
            .name("body")
            .map(|m| m.as_str().trim())
            .unwrap_or_default();
        match serde_json::from_str::<Value>(body) {
            Ok(node) => extract(&node, &mut calls, language == "tool"),
            Err(err) => {
                if language == "tool" || looks_like_plan(body) {
                    calls.push(json_error(&err.to_string()));
                }
            }
        }
    }
    if !calls.is_empty() {
        return validated(calls);
    }
    let raw = text.trim();
    if raw.starts_with('{') || raw.starts_with('[') {
        match serde_json::from_str::<Value>(raw) {
            Ok(node) => extract(&node, &mut calls, false),
            Err(err) => {
                if looks_like_plan(raw) {
                    calls.push(json_error(&err.to_string()));
                }
            }
        }
    }
    if calls.is_empty() && incomplete_tool_block_regex().is_match(text) {
        calls.push(ToolCall::parse_error("Incomplete local tool block"));
    }
    validated(calls)
}

fn extract(node: &Value, calls: &mut Vec<ToolCall>, explicit_tool: bool) {
    if let Value::Array(items) = node {
        for item in items {
            extract(item, calls, explicit_tool);
        }
        return;
    }
    let obj = match node {
        Value::Object(obj) => obj,
        _ => {
            if explicit_tool {
                calls.push(ToolCall::parse_error("Tool must be a JSON object"));
            }
            return;
        }
    };
    let status = obj.get("status").and_then(|v| v.as_str());
    if status == Some("needs_input") {
        let question = obj
            .get("reply")
            .or_else(|| obj.get("summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if question.trim().is_empty() {
            calls.push(ToolCall::parse_error(
                "needs_input requires a non-empty question in summary or reply.",
            ));
            return;
        }
        let has_actions = matches!(obj.get("actions"), Some(Value::Array(a)) if !a.is_empty())
            || matches!(obj.get("tool_calls"), Some(Value::Array(a)) if !a.is_empty());
        if has_actions {
            calls.push(ToolCall::parse_error(
                "needs_input cannot contain actions. Ask for input OR request local operations, not both.",
            ));
            return;
        }
    }
    if obj.contains_key("actions") && obj.contains_key("tool_calls") {
        calls.push(ToolCall::parse_error(
            "Use one actions or tool_calls envelope, not both. No actions were executed.",
        ));
        return;
    }
    let actions_not_array = obj.contains_key("actions")
        && !matches!(obj.get("actions"), Some(Value::Array(_)));
    let calls_not_array = obj.contains_key("tool_calls")
        && !matches!(obj.get("tool_calls"), Some(Value::Array(_)));
    if actions_not_array || calls_not_array {
        calls.push(ToolCall::parse_error(
            "actions/tool_calls must be JSON arrays. No actions were executed.",
        ));
        return;
    }
    if let Some(Value::Array(plan)) = obj.get("actions") {
        for item in plan {
            let step = match item {
                Value::Object(step) => step,
                _ => {
                    calls.push(ToolCall::parse_error("Application action must be an object"));
                    continue;
                }
            };
            let operation = step
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let Some(action_tool) = action_map(&operation) else {
                calls.push(ToolCall::parse_error(format!(
                    "Unknown application action: {operation}"
                )));
                continue;
            };
            let mut action_args = step.clone();
            action_args.remove("action");
            if operation == "replace_text" {
                if let Some(find) = action_args.get("find").cloned() {
                    action_args.insert("old_text".to_string(), find);
                }
            }
            if operation == "save_text" {
                if let Some(content) = action_args.get("text").cloned() {
                    action_args.insert("content".to_string(), content);
                }
            }
            if operation == "terminal" {
                if let Some(script) = action_args.get("script").cloned() {
                    action_args.insert("command".to_string(), script);
                }
            }
            calls.push(normalize(&ToolCall::new(action_tool, action_args)));
        }
        return;
    }
    if let Some(Value::Array(envelope)) = obj.get("tool_calls") {
        for item in envelope {
            extract(item, calls, true);
        }
        return;
    }
    let function = obj.get("function").and_then(|v| v.as_object());
    let name = function
        .and_then(|f| f.get("name"))
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("tool"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
        if explicit_tool {
            calls.push(ToolCall::parse_error("Tool name is required"));
        }
        return;
    };
    let arguments = function
        .and_then(|f| f.get("arguments"))
        .or_else(|| obj.get("arguments"))
        .or_else(|| obj.get("parameters"));
    let args: Map<String, Value>;
    if let Some(Value::String(encoded)) = arguments {
        match serde_json::from_str::<Value>(encoded) {
            Ok(Value::Object(map)) => args = map,
            Ok(_) | Err(_) => {
                calls.push(ToolCall::parse_error(
                    "Tool arguments must be a JSON object",
                ));
                return;
            }
        }
    } else if let Some(Value::Object(map)) = arguments {
        args = map.clone();
    } else if arguments.is_some()
        || obj.contains_key("arguments")
        || obj.contains_key("parameters")
        || function.map(|f| f.contains_key("arguments")).unwrap_or(false)
    {
        calls.push(ToolCall::parse_error(
            "Tool arguments must be a JSON object or an encoded JSON object. No actions were executed.",
        ));
        return;
    } else {
        args = {
            let mut map = obj.clone();
            map.remove("name");
            map.remove("tool");
            map.remove("id");
            map
        };
    }
    let call = normalize(&ToolCall::new(name, args));
    let has_args = arguments.is_some() || function.is_some();
    if !is_known(&call.name) && (explicit_tool || has_args) {
        calls.push(validate(&call));
    } else if explicit_tool || is_known(&call.name) {
        calls.push(call);
    }
}

/// Extract the final reply text from a planner response: if any fence (or
/// the whole text) parses as a `{status: completed|done|needs_input,
/// reply|summary}` object, return that reply; otherwise return the text
/// unchanged (`.NET PlannerReply`).
pub fn planner_reply(text: &str) -> String {
    let mut candidates: Vec<String> = fences_regex()
        .captures_iter(text)
        .filter_map(|c| c.name("body").map(|m| m.as_str().to_string()))
        .collect();
    candidates.push(text.trim().to_string());
    for candidate in candidates {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&candidate) {
            let status = obj.get("status").and_then(|v| v.as_str());
            if matches!(status, Some("completed") | Some("done") | Some("needs_input")) {
                if let Some(Value::String(reply)) = obj
                    .get("reply")
                    .or_else(|| obj.get("summary"))
                {
                    return reply.clone();
                }
            }
        }
    }
    text.to_string()
}

/// True when the reply asks the user for input (`.NET NeedsInput`).
pub fn needs_input(text: &str) -> bool {
    let mut candidates: Vec<String> = fences_regex()
        .captures_iter(text)
        .filter_map(|c| c.name("body").map(|m| m.as_str().to_string()))
        .collect();
    candidates.push(text.trim().to_string());
    for candidate in candidates {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&candidate) {
            if obj.get("status").and_then(|v| v.as_str()) == Some("needs_input") {
                return true;
            }
        }
    }
    crate::intent::is_clarification(&planner_reply(&strip_tool_blocks(text)))
}

/// Remove tool blocks from a reply for display (`.NET StripToolBlocks`).
/// ```tool blocks are removed outright; ```json blocks are removed only
/// when they parse as a tool plan; anything else is kept as prose.
pub fn strip_tool_blocks(text: &str) -> String {
    let closed_fence = Regex::new(r"\r?\n[ \t]*(?:```|~~~)[ \t]*$").expect("closed fence regex");
    let visible = fences_regex().replace_all(text, |caps: &regex::Captures| {
        let language = caps
            .name("language")
            .map(|m| m.as_str().trim().to_lowercase())
            .unwrap_or_default();
        if language == "tool" {
            return String::new();
        }
        if language == "json" || language.is_empty() {
            let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let closed = closed_fence.is_match(whole);
            if !closed && language == "json" {
                return String::new();
            }
            let content = caps.name("body").map(|m| m.as_str()).unwrap_or("");
            if !parse(&format!("```json\n{content}\n```")).is_empty() {
                return String::new();
            }
        }
        caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()
    });
    // During streaming, never render an incomplete protocol block as prose.
    let mut raw = visible.trim().to_string();
    if !parse(text).is_empty() {
        let lead = Regex::new(r"(?im)^[ \t]*(?:json|tool)[ \t]*(?:\r?\n|$)").expect("lead regex");
        raw = lead.replace(&raw, "").trim().to_string();
    }
    if (raw.starts_with('{') || raw.starts_with('[')) && !parse(&raw).is_empty() {
        return String::new();
    }
    raw
}

/// Re-exported argument helpers for callers that build calls by hand.
pub fn arg_str(args: &Map<String, Value>, name: &str, fallback: &str) -> String {
    str_arg(args, name, fallback)
}
pub fn arg_bool(args: &Map<String, Value>, name: &str, fallback: bool) -> bool {
    bool_arg(args, name, fallback)
}
pub fn arg_int(args: &Map<String, Value>, name: &str, fallback: i64) -> i64 {
    int_arg(args, name, fallback)
}
