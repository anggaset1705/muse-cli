//! Conversation context — a simplified port of `ConversationContext.cs`.
//!
//! The .NET implementation rolls evicted messages into an extractive
//! archive memory (`APP_CONTEXT_ARCHIVE:` JSON with notes, preferences
//! and turn outcomes) and re-attaches it to every request. This port
//! implements the two load-bearing behaviours — `FitContext` (96k char
//! budget, at most one image) and dropping messages beyond the recent
//! budget — and returns a small snapshot of recent excerpts instead of
//! the full archive format.
//!
//! DEVIATION (recorded for the coordinator): the extractive archive
//! memory (`Compact` notes / `Snapshot` memory JSON / `Recall`) is NOT
//! ported. History still survives across turns in the session; only the
//! long-term archive packet is missing, so very long conversations lose
//! older detail instead of compressing it. The wire shape of the
//! `APP_STATE` packet is otherwise kept.

use serde_json::{json, Map, Value};
/// Maximum recent messages kept verbatim (`.NET MaxRecentMessages`).
pub const MAX_RECENT_MESSAGES: usize = 64;
/// Recent-text budget (`.NET RecentCharacterBudget`).
pub const RECENT_CHARACTER_BUDGET: usize = 48_000;
/// Context budget for a single planner request (`.NET FitContext`, 96k).
pub const FIT_CHARACTER_BUDGET: usize = 96_000;

/// Plain text of a message (content string, or joined text parts).
pub fn message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    p.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Does this message carry an image part?
pub fn message_has_image(message: &Value) -> bool {
    matches!(message.get("content"), Some(Value::Array(parts)) if parts.iter().any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url")))
}

/// Strip image parts, replacing them with a placeholder
/// (`.NET AgentLoop.WithoutImage`).
pub fn without_image(message: &Value) -> Value {
    let mut copy = message.clone();
    if let Some(Value::Array(parts)) = message.get("content") {
        let text = parts
            .iter()
            .map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("image_url") {
                    "[An image was attached in this turn. Reattach it if the original image is needed again.]".to_string()
                } else {
                    p.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Value::Object(map) = &mut copy {
            map.insert("content".to_string(), Value::String(text));
        }
    }
    copy
}

/// Drop messages older than the recent budget, keeping the protected
/// prefix (usually the system prompt). Returns true when anything was
/// dropped. Simplified `.NET Compact`: no archive notes are produced.
pub fn compact(messages: &mut Vec<Value>, protected_prefix: usize) -> bool {
    if messages.len() <= protected_prefix {
        return false;
    }
    let history = &messages[protected_prefix..];
    let mut start = history.len();
    let mut size = 0usize;
    while start > 0 && history.len() - start < MAX_RECENT_MESSAGES {
        let cost = message_text(&history[start - 1]).len() + 100;
        if size + cost > RECENT_CHARACTER_BUDGET && start < history.len() {
            break;
        }
        size += cost;
        start -= 1;
    }
    if start == 0 {
        return false;
    }
    let dropped = start;
    let mut kept: Vec<Value> = messages.drain(..protected_prefix).collect();
    kept.push(json!({
        "role": "system",
        "content": "Older messages were omitted for context limits. Re-read project files when needed; do not assume their contents."
    }));
    let rest: Vec<Value> = messages.drain(..).skip(dropped).collect();
    kept.extend(rest);
    *messages = kept;
    true
}

/// Fit messages into the 96k planner-request budget (`.NET FitContext`):
/// newest-first selection, at most one image, system prompt always kept.
/// Returns the fitted list (system prompt first).
pub fn fit_context(messages: &[Value]) -> Vec<Value> {
    if messages.is_empty() {
        return Vec::new();
    }
    let mut selected: Vec<Value> = Vec::new();
    let mut size = messages[0].to_string().len();
    let mut kept_image = false;
    for original in messages.iter().skip(1).rev() {
        let has_image = message_has_image(original);
        let message = if has_image && kept_image {
            without_image(original)
        } else {
            original.clone()
        };
        let length = if has_image {
            without_image(&message).to_string().len() + 2048
        } else {
            message.to_string().len()
        };
        if size + length > FIT_CHARACTER_BUDGET && !selected.is_empty() {
            break;
        }
        size += length;
        selected.push(message);
        if has_image {
            kept_image = true;
        }
    }
    selected.reverse();
    let mut out = vec![messages[0].clone()];
    if selected.len() < messages.len() - 1 {
        out.push(json!({
            "role": "system",
            "content": "Older messages were omitted for context limits. Re-read project files when needed; do not assume their contents."
        }));
    }
    out.extend(selected);
    out
}

/// A small snapshot of recent conversation excerpts for the APP_STATE
/// packet. This is a simplified stand-in for .NET
/// `ConversationContext.Snapshot` (which returns the full archive memory).
pub fn snapshot(messages: &[Value]) -> Value {
    let recent: Vec<Value> = messages
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|m| {
            let mut obj = Map::new();
            obj.insert(
                "role".to_string(),
                m.get("role").cloned().unwrap_or(Value::Null),
            );
            let text = message_text(m);
            let clipped: String = text.chars().take(700).collect();
            obj.insert("text".to_string(), Value::String(clipped));
            Value::Object(obj)
        })
        .collect();
    json!({
        "version": 1,
        "recent": recent,
    })
}

/// Is this a user request message (not a tool receipt / executor packet)?
/// Mirrors the .NET `IsUserRequest` prefix checks, used to find the
/// initial request.
pub fn is_user_request(message: &Value) -> bool {
    if message.get("role").and_then(|r| r.as_str()) != Some("user") {
        return false;
    }
    let text = message_text(message);
    ![
        "Tool result for ",
        "LOCAL EXECUTOR:",
        "APP_STATE:",
        "APP_PROTOCOL_REPAIR:",
        "The user ",
        "Background terminal result:",
    ]
    .iter()
    .any(|p| text.starts_with(p))
}

/// Observation key for repeated-observation detection
/// (`.NET AgentLoop.ObservationKey`): computer inspect/screenshot results
/// are normalised by dropping volatile ids.
pub fn observation_key(call: &crate::parser::ToolCall, result: &str) -> String {
    if matches!(call.name.as_str(), "computer_inspect" | "computer_screenshot") {
        if let Ok(Value::Object(mut state)) = serde_json::from_str::<Value>(result) {
            if call.name == "computer_screenshot"
                && state
                    .get("image_fingerprint")
                    .and_then(|v| v.as_str())
                    .map(|s| s.is_empty())
                    .unwrap_or(true)
            {
                return format!("{}{}{}", call.name, args_json(&call.args), result);
            }
            state.remove("snapshot_id");
            state.remove("inspection_id");
            if let Some(Value::Array(elements)) = state.get_mut("elements") {
                for element in elements.iter_mut() {
                    if let Value::Object(map) = element {
                        map.remove("element_id");
                    }
                }
            }
            let mut args = call.args.clone();
            args.remove("element_id");
            args.remove("snapshot_id");
            return format!(
                "{}{}{}",
                call.name,
                Value::Object(args).to_string(),
                Value::Object(state).to_string()
            );
        }
    }
    format!("{}{}{}", call.name, args_json(&call.args), result)
}

fn args_json(args: &Map<String, Value>) -> String {
    Value::Object(args.clone()).to_string()
}

/// Re-exported for the chat turn.
pub use crate::parser::strip_tool_blocks;
