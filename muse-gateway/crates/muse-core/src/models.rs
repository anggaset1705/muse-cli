//! Model registry + prompt conversion, ported 1:1 from .NET `Models.cs`
//! (itself ported from Python `core/models.py` / `core/prompt.py`).
//!
//! Messages are `serde_json::Value` objects (the Rust stand-in for .NET
//! `JsonObject`); `JsonHelp.Str(node)` is `node.as_str()`.

use serde::Serialize;
use serde_json::{json, Value};

use crate::errors::MuseError;
use crate::prompt_templates::{COMPACT_TEMPLATE, FULL_TEMPLATE};

/// One model entry. Mirrors .NET `record ModelSpec(string Id, string Kind,
/// string Description = "", string OwnedBy = "muse")`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelSpec {
    pub id: &'static str,
    pub kind: &'static str,
    pub description: &'static str,
    pub owned_by: &'static str,
}

/// The three real models (only 3 exist).
pub const MODELS: [ModelSpec; 3] = [
    ModelSpec {
        id: "muse-chat",
        kind: "chat",
        description: "Default muse.ai assistant",
        owned_by: "muse",
    },
    ModelSpec {
        id: "muse-image",
        kind: "image",
        description: "Text-to-image / image editing",
        owned_by: "muse",
    },
    ModelSpec {
        id: "muse-video",
        kind: "video",
        description: "Text-to-video / first-frame image-to-video",
        owned_by: "muse",
    },
];

/// Alias → real model id (9 aliases).
pub const ALIASES: [(&str, &str); 9] = [
    ("gpt-4o", "muse-chat"),
    ("gpt-4o-mini", "muse-chat"),
    ("gpt-4.1", "muse-chat"),
    ("gpt-5", "muse-chat"),
    ("claude-sonnet-4", "muse-chat"),
    ("deepseek-chat", "muse-chat"),
    ("dall-e-3", "muse-image"),
    ("gpt-image-1", "muse-image"),
    ("sora", "muse-video"),
];

/// Default model id per kind.
pub fn default_for_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "chat" => Some("muse-chat"),
        "image" => Some("muse-image"),
        "video" => Some("muse-video"),
        _ => None,
    }
}

pub struct ModelRegistry;

impl ModelRegistry {
    pub fn models() -> &'static [ModelSpec] {
        &MODELS
    }

    pub fn aliases() -> &'static [(&'static str, &'static str)] {
        &ALIASES
    }

    /// Mirrors .NET `ModelRegistry.Resolve(name, kind)`:
    /// - empty/`None` name → default per kind;
    /// - unknown name → default per kind;
    /// - known model whose kind does not match → `MuseError::invalid` with
    ///   message `model '<name>' is a <kind> model, expected <kind>`.
    ///
    /// Deviation: an unknown `kind` has no default in .NET either — there
    /// `Models[DefaultByKind[kind]]` throws `KeyNotFoundException` (a 500).
    /// Here that surfaces as `MuseError::internal("unknown model kind ...")`.
    pub fn resolve(name: Option<&str>, kind: &str) -> Result<ModelSpec, MuseError> {
        let default = || -> Result<ModelSpec, MuseError> {
            default_for_kind(kind)
                .and_then(|id| MODELS.iter().find(|m| m.id == id).copied())
                .ok_or_else(|| MuseError::internal(format!("unknown model kind '{kind}'")))
        };
        let name = match name {
            None => return default(),
            Some(n) if n.is_empty() => return default(),
            Some(n) => n,
        };
        let key = ALIASES
            .iter()
            .find(|(alias, _)| *alias == name)
            .map(|(_, target)| *target)
            .unwrap_or(name);
        let spec = match MODELS.iter().find(|m| m.id == key) {
            Some(s) => *s,
            None => return default(),
        };
        if spec.kind != kind {
            return Err(MuseError::invalid(format!(
                "model '{name}' is a {} model, expected {kind}",
                spec.kind
            )));
        }
        Ok(spec)
    }
}

/// Flattened prompt: one text plus collected image URLs.
/// Mirrors .NET `Prompt.FlatPrompt(string Text, List<string> Images)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatPrompt {
    pub text: String,
    pub images: Vec<String>,
}

pub struct Prompt;

fn role_label(role: &str) -> String {
    match role {
        "system" | "developer" => "System".to_string(),
        "user" => "User".to_string(),
        "assistant" => "Assistant".to_string(),
        "tool" => "Tool".to_string(),
        _ => {
            let mut chars = role.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

impl Prompt {
    /// Mirrors .NET `Prompt.ContentToText(JsonNode? content, List<string> images)`.
    pub fn content_to_text(content: &Value, images: &mut Vec<String>) -> Result<String, MuseError> {
        if content.is_null() {
            return Ok(String::new());
        }
        if let Some(text) = content.as_str() {
            return Ok(text.to_string());
        }
        if let Some(arr) = content.as_array() {
            let mut parts: Vec<String> = Vec::new();
            for part in arr {
                let obj = part.as_object().ok_or_else(|| {
                    MuseError::invalid("content parts must be objects")
                })?;
                let ty = obj
                    .get("type")
                    .and_then(|t| t.as_str())
                    .ok_or_else(|| {
                        MuseError::invalid("content part type must be a string")
                    })?;
                match ty {
                    "text" | "input_text" | "output_text" => {
                        let text = obj
                            .get("text")
                            .and_then(|t| t.as_str())
                            .ok_or_else(|| {
                                MuseError::invalid("text part must contain a string")
                            })?;
                        parts.push(text.to_string());
                    }
                    "image_url" | "input_image" => {
                        let url_node = obj.get("image_url");
                        let url: Option<&str> = match url_node {
                            Some(u) if u.is_object() => {
                                u.get("url").and_then(|v| v.as_str())
                            }
                            Some(u) => u.as_str(),
                            None => None,
                        };
                        let url = url.filter(|u| !u.trim().is_empty()).ok_or_else(|| {
                            MuseError::invalid("image part must contain an image_url")
                        })?;
                        images.push(url.to_string());
                    }
                    // Any other part type is ignored, exactly like .NET.
                    _ => {}
                }
            }
            return Ok(parts
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("\n"));
        }
        Err(MuseError::invalid(
            "message content must be a string or a list of parts",
        ))
    }

    /// Mirrors .NET `Prompt.Role(JsonObject msg)` (private there).
    /// Deviation: .NET receives a typed `JsonObject`, so a non-object message
    /// cannot occur; here it is `MuseError::invalid("message must be an object")`.
    pub fn role(msg: &Value) -> Result<String, MuseError> {
        let obj = msg
            .as_object()
            .ok_or_else(|| MuseError::invalid("message must be an object"))?;
        if let Some(role_node) = obj.get("role") {
            if !role_node.is_null() && role_node.as_str().is_none() {
                return Err(MuseError::invalid("message role must be a string"));
            }
        }
        let role = obj
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("user");
        match role {
            "system" | "developer" | "user" | "assistant" | "tool" => Ok(role.to_string()),
            _ => Err(MuseError::invalid("unsupported message role")),
        }
    }

    /// Mirrors .NET `Prompt.MessageTurns` — `(role, trimmed text)` pairs,
    /// empty texts dropped, images discarded. Note the .NET order: content
    /// is converted (and may throw) *before* the role is validated.
    pub fn message_turns(messages: &[Value]) -> Result<Vec<(String, String)>, MuseError> {
        let mut turns = Vec::new();
        for msg in messages {
            let text = Self::content_to_text(
                msg.get("content").unwrap_or(&Value::Null),
                &mut Vec::new(),
            )?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                turns.push((Self::role(msg)?, text));
            }
        }
        Ok(turns)
    }

    /// Mirrors .NET `Prompt.Flatten`. Note the .NET order here is the
    /// reverse of `MessageTurns`: role is validated *before* content.
    pub fn flatten(messages: &[Value]) -> Result<FlatPrompt, MuseError> {
        let mut images: Vec<String> = Vec::new();
        let mut turns: Vec<(String, String)> = Vec::new();
        for msg in messages {
            let role = Self::role(msg)?;
            let text = Self::content_to_text(
                msg.get("content").unwrap_or(&Value::Null),
                &mut images,
            )?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                turns.push((role, text));
            }
        }
        if turns.is_empty() {
            return Ok(FlatPrompt {
                text: String::new(),
                images,
            });
        }
        if turns.len() == 1 && turns[0].0 == "user" {
            return Ok(FlatPrompt {
                text: turns[0].1.clone(),
                images,
            });
        }
        let mut blocks: Vec<String> = turns
            .iter()
            .map(|(role, text)| format!("[{}]\n{}", role_label(role), text))
            .collect();
        blocks.push(
            "[Instruction]\nContinue the conversation above and reply as the Assistant to the last User message. Output only the reply."
                .to_string(),
        );
        Ok(FlatPrompt {
            text: blocks.join("\n\n"),
            images,
        })
    }

    /// Mirrors .NET `Prompt.ForLocalExecutor(messages, compact, replyLanguage)`.
    /// Builds the `inputs` JSON array (indented unless `compact`), then the
    /// compact/full instruction template with `__REPLY_LANGUAGE__` replaced,
    /// then `template + "\n" + input`. Templates are byte-identical to the
    /// C# raw strings in `Models.cs` (see `prompt_templates.rs`).
    pub fn for_local_executor(
        messages: &[Value],
        compact: bool,
        reply_language: &str,
    ) -> Result<String, MuseError> {
        let mut inputs: Vec<Value> = Vec::new();
        for message in messages {
            let mut images: Vec<String> = Vec::new();
            let mut text = Self::content_to_text(
                message.get("content").unwrap_or(&Value::Null),
                &mut images,
            )?;
            let role = Self::role(message)?;
            // The agent has rolled this archive into APP_STATE.conversation_context.
            if role == "system" && text.starts_with("APP_CONTEXT_ARCHIVE:\n") {
                continue;
            }
            if role == "assistant"
                && (text.starts_with("[Unverified reply:")
                    || text.starts_with("[Invalid plan:"))
            {
                continue;
            }
            let mut kind: &str = match role.as_str() {
                "system" | "developer" => "workspace_context",
                "assistant" => "previous_generated_plan",
                _ => "request",
            };
            if role == "system" || role == "developer" {
                if let Some(context_at) = text.find("\n\nWorkspace:") {
                    text = text[context_at + 2..].to_string();
                }
            }
            if text.starts_with("Tool result for ") {
                kind = "observed_data_from_app";
                if let Some(boundary) = text.find(":\n") {
                    text = text[boundary + 2..].to_string();
                }
            }
            if let Some(rest) = text.strip_prefix("APP_STATE:\n") {
                kind = "current_app_state";
                text = rest.to_string();
            }
            if let Some(rest) = text.strip_prefix("APP_PROTOCOL_REPAIR:\n") {
                kind = "protocol_repair";
                text = rest.to_string();
            }
            if let Some(rest) = text.strip_prefix("LOCAL EXECUTOR:") {
                kind = "execution_repair";
                text = rest.trim_start().to_string();
            }
            // Field order (kind, text, attached_images) matches .NET's
            // JsonObject insertion order; serde_json "preserve_order" keeps it.
            inputs.push(json!({
                "kind": kind,
                "text": text,
                "attached_images": images.len(),
            }));
        }
        let input_array = Value::Array(inputs);
        let input = if compact {
            serde_json::to_string(&input_array)
        } else {
            serde_json::to_string_pretty(&input_array)
        }
        .map_err(|e| MuseError::internal(format!("failed to serialize executor inputs: {e}")))?;
        let template = if compact { COMPACT_TEMPLATE } else { FULL_TEMPLATE };
        let template = template.replace("__REPLY_LANGUAGE__", reply_language);
        Ok(format!("{template}\n{input}"))
    }

    /// Mirrors .NET `Prompt.ImageBatchId`: `attachment_id`s of the latest
    /// message containing an `image_url` part, joined with `:`; `None` when
    /// there is no such message or any image part lacks an `attachment_id`.
    pub fn image_batch_id(messages: &[Value]) -> Option<String> {
        let latest = messages.iter().rev().find(|m| {
            m.get("content")
                .and_then(|c| c.as_array())
                .map(|parts| {
                    parts.iter().any(|p| {
                        p.get("type").and_then(|t| t.as_str()) == Some("image_url")
                    })
                })
                .unwrap_or(false)
        })?;
        let content = latest.get("content")?.as_array()?;
        let images: Vec<&serde_json::Map<String, Value>> = content
            .iter()
            .filter_map(|p| p.as_object())
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url"))
            .collect();
        let mut ids: Vec<String> = Vec::new();
        for p in images {
            let id = p.get("attachment_id").and_then(|v| v.as_str())?;
            if id.is_empty() {
                return None;
            }
            ids.push(id.to_string());
        }
        Some(ids.join(":"))
    }

    /// Mirrors .NET `Prompt.LocalContinuation`: only observations since the
    /// last generated (assistant) plan, plus the latest state/goal, sent to
    /// the same upstream chat — compact executor prompt.
    pub fn local_continuation(
        turns: &[(String, String)],
        reply_language: &str,
    ) -> String {
        let last_plan = turns
            .iter()
            .rposition(|(role, _)| role == "assistant")
            .map(|i| i as isize)
            .unwrap_or(-1);
        let latest: Vec<Value> = turns
            .iter()
            .skip((last_plan + 1) as usize)
            .filter(|(role, _)| role != "system" && role != "developer")
            .map(|(role, text)| json!({"role": role, "content": text}))
            .collect();
        // Cannot fail: roles and string contents were already validated when
        // the turns were built.
        Self::for_local_executor(&latest, true, reply_language)
            .expect("local_continuation inputs are pre-validated turns")
    }

    /// Mirrors .NET `Prompt.FollowupText`: the extra user text when
    /// `incoming` extends `previous` with only user turns appended after
    /// stripping leading assistant/tool turns; `None` otherwise.
    pub fn followup_text(
        previous: &[(String, String)],
        incoming: &[(String, String)],
    ) -> Option<String> {
        if previous.is_empty() || incoming.len() <= previous.len() {
            return None;
        }
        if incoming[..previous.len()] != previous[..] {
            return None;
        }
        let mut extra: Vec<(String, String)> = incoming[previous.len()..].to_vec();
        while !extra.is_empty() && (extra[0].0 == "assistant" || extra[0].0 == "tool") {
            extra.remove(0);
        }
        let mut user_text: Vec<String> = Vec::new();
        for (role, text) in &extra {
            if role != "user" {
                return None;
            }
            user_text.push(text.clone());
        }
        if user_text.is_empty() {
            None
        } else {
            Some(user_text.join("\n\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: &str, content: Value) -> Value {
        json!({"role": role, "content": content})
    }

    // ---- ModelRegistry::resolve ----

    #[test]
    fn resolve_real_models() {
        assert_eq!(ModelRegistry::resolve(Some("muse-chat"), "chat").unwrap().id, "muse-chat");
        assert_eq!(ModelRegistry::resolve(Some("muse-image"), "image").unwrap().id, "muse-image");
        assert_eq!(ModelRegistry::resolve(Some("muse-video"), "video").unwrap().id, "muse-video");
    }

    #[test]
    fn resolve_aliases() {
        for alias in ["gpt-4o", "gpt-4o-mini", "gpt-4.1", "gpt-5", "claude-sonnet-4", "deepseek-chat"] {
            assert_eq!(ModelRegistry::resolve(Some(alias), "chat").unwrap().id, "muse-chat", "{alias}");
        }
        for alias in ["dall-e-3", "gpt-image-1"] {
            assert_eq!(ModelRegistry::resolve(Some(alias), "image").unwrap().id, "muse-image", "{alias}");
        }
        assert_eq!(ModelRegistry::resolve(Some("sora"), "video").unwrap().id, "muse-video");
    }

    #[test]
    fn resolve_empty_and_unknown_fall_back_to_default() {
        assert_eq!(ModelRegistry::resolve(None, "chat").unwrap().id, "muse-chat");
        assert_eq!(ModelRegistry::resolve(Some(""), "image").unwrap().id, "muse-image");
        assert_eq!(ModelRegistry::resolve(Some("no-such-model"), "video").unwrap().id, "muse-video");
        // Alias of another kind is a *known* model, so kind mismatch errors (not default).
        assert_eq!(ModelRegistry::resolve(Some("gpt-4o"), "chat").unwrap().id, "muse-chat");
    }

    #[test]
    fn resolve_kind_mismatch_errors_with_exact_message() {
        let err = ModelRegistry::resolve(Some("muse-video"), "chat").unwrap_err();
        assert_eq!(err.status_code, 400);
        assert_eq!(err.message, "model 'muse-video' is a video model, expected chat");
        let err = ModelRegistry::resolve(Some("sora"), "image").unwrap_err();
        assert_eq!(err.message, "model 'sora' is a video model, expected image");
        let err = ModelRegistry::resolve(Some("dall-e-3"), "chat").unwrap_err();
        assert_eq!(err.message, "model 'dall-e-3' is a image model, expected chat");
    }

    #[test]
    fn resolve_unknown_kind_is_internal_error() {
        let err = ModelRegistry::resolve(None, "audio").unwrap_err();
        assert_eq!(err.status_code, 500);
    }

    // ---- Prompt::flatten ----

    #[test]
    fn flatten_single_user_message_is_bare_text() {
        let flat = Prompt::flatten(&[msg("user", json!("halo"))]).unwrap();
        assert_eq!(flat.text, "halo");
        assert!(flat.images.is_empty());
    }

    #[test]
    fn flatten_multi_turn_uses_role_blocks_and_instruction() {
        let flat = Prompt::flatten(&[
            msg("system", json!("You are helpful.")),
            msg("user", json!("Hi")),
            msg("assistant", json!("Hello!")),
            msg("user", json!("Thanks")),
        ])
        .unwrap();
        assert_eq!(
            flat.text,
            "[System]\nYou are helpful.\n\n[User]\nHi\n\n[Assistant]\nHello!\n\n[User]\nThanks\n\n[Instruction]\nContinue the conversation above and reply as the Assistant to the last User message. Output only the reply."
        );
    }

    #[test]
    fn flatten_developer_maps_to_system_label_and_empty_turns_dropped() {
        let flat = Prompt::flatten(&[
            msg("developer", json!("ctx")),
            msg("user", json!("   ")),
            msg("user", json!("q")),
        ])
        .unwrap();
        assert!(flat.text.starts_with("[System]\nctx\n\n[User]\nq"));
    }

    #[test]
    fn flatten_empty_messages() {
        let flat = Prompt::flatten(&[]).unwrap();
        assert_eq!(flat.text, "");
    }

    #[test]
    fn flatten_image_parts_collect_urls_and_text() {
        let flat = Prompt::flatten(&[msg(
            "user",
            json!([{"type": "text", "text": "describe"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}]),
        )])
        .unwrap();
        assert_eq!(flat.text, "describe");
        assert_eq!(flat.images, vec!["data:image/png;base64,AAA".to_string()]);
    }

    #[test]
    fn flatten_rejects_bad_role_and_bad_content() {
        assert_eq!(
            Prompt::flatten(&[msg("pirate", json!("x"))]).unwrap_err().message,
            "unsupported message role"
        );
        assert_eq!(
            Prompt::flatten(&[msg("user", json!(42))]).unwrap_err().message,
            "message content must be a string or a list of parts"
        );
        assert_eq!(
            Prompt::flatten(&[msg("user", json!([{"type": "text"}]))])
                .unwrap_err()
                .message,
            "text part must contain a string"
        );
        assert_eq!(
            Prompt::flatten(&[msg("user", json!([{"type": "image_url", "image_url": {"url": "  "}}]))])
                .unwrap_err()
                .message,
            "image part must contain an image_url"
        );
        assert_eq!(
            Prompt::flatten(&[json!({"role": 7, "content": "x"})])
                .unwrap_err()
                .message,
            "message role must be a string"
        );
    }

    #[test]
    fn message_turns_trims_and_skips_empty() {
        let turns = Prompt::message_turns(&[
            msg("user", json!("  hi  ")),
            msg("assistant", json!("")),
            msg("tool", json!("out")),
        ])
        .unwrap();
        assert_eq!(
            turns,
            vec![
                ("user".to_string(), "hi".to_string()),
                ("tool".to_string(), "out".to_string()),
            ]
        );
    }

    // ---- Prompt::for_local_executor ----

    #[test]
    fn for_local_executor_compact_shape_and_language() {
        let out = Prompt::for_local_executor(
            &[msg("user", json!("kerjakan X"))],
            true,
            "Bahasa Indonesia",
        )
        .unwrap();
        assert!(out.starts_with("Continue the current desktop chat turn."), "{out}");
        assert!(out.contains("Bahasa Indonesia"));
        assert!(!out.contains("__REPLY_LANGUAGE__"));
        assert!(out.contains("INPUT DATA:\n[{\"kind\":\"request\",\"text\":\"kerjakan X\",\"attached_images\":0}]"), "{out}");
    }

    #[test]
    fn for_local_executor_full_shape_and_language() {
        let out = Prompt::for_local_executor(
            &[msg("user", json!("hi"))],
            false,
            "English",
        )
        .unwrap();
        assert!(out.starts_with("Reply to the current chat turn with JSON for my desktop app."), "{out}");
        assert!(out.contains("A short summary in English, based on the data the app reported"));
        assert!(out.contains("INPUT DATA:\n[\n  {\n    \"kind\": \"request\",\n    \"text\": \"hi\",\n    \"attached_images\": 0\n  }\n]"), "{out}");
    }

    #[test]
    fn for_local_executor_kinds_and_prefix_rules() {
        let out = Prompt::for_local_executor(
            &[
                msg("system", json!("APP_CONTEXT_ARCHIVE:\nold stuff")),
                msg("system", json!("pre\n\nWorkspace: ws\nfile list")),
                msg("assistant", json!("[Unverified reply: blah")),
                msg("assistant", json!("plan text")),
                msg("user", json!("Tool result for scan:\n{\"files\":[]}")),
                msg("user", json!("APP_STATE:\n{\"goal\":\"x\"}")),
                msg("user", json!("APP_PROTOCOL_REPAIR:\nfix it")),
                msg("user", json!("LOCAL EXECUTOR:  retry this")),
            ],
            true,
            "English",
        )
        .unwrap();
        let input = out.split("INPUT DATA:\n").nth(1).unwrap();
        let parsed: Value = serde_json::from_str(input).unwrap();
        let kinds: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["kind"].as_str().unwrap())
            .collect();
        let texts: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "workspace_context",
                "previous_generated_plan",
                "observed_data_from_app",
                "current_app_state",
                "protocol_repair",
                "execution_repair",
            ],
            "{out}"
        );
        // System "pre\n\nWorkspace: ..." keeps from contextAt+2, i.e. both
        // newlines are skipped and the text starts at "Workspace:".
        assert_eq!(texts[0], "Workspace: ws\nfile list");
        assert_eq!(texts[1], "plan text");
        assert_eq!(texts[2], "{\"files\":[]}");
        assert_eq!(texts[3], "{\"goal\":\"x\"}");
        assert_eq!(texts[4], "fix it");
        assert_eq!(texts[5], "retry this");
    }

    #[test]
    fn image_batch_id_joins_attachment_ids() {
        let msgs = vec![
            msg("user", json!([{"type": "image_url", "image_url": "u1", "attachment_id": "a1"}])),
            msg("user", json!([{"type": "image_url", "image_url": "u2", "attachment_id": "a2"}, {"type": "image_url", "image_url": "u3", "attachment_id": "a3"}])),
        ];
        assert_eq!(Prompt::image_batch_id(&msgs).as_deref(), Some("a2:a3"));
        let missing = vec![msg("user", json!([{"type": "image_url", "image_url": "u1"}]))];
        assert_eq!(Prompt::image_batch_id(&missing), None);
        assert_eq!(Prompt::image_batch_id(&[msg("user", json!("no images"))]), None);
    }

    #[test]
    fn followup_text_only_user_extension() {
        let prev = vec![("user".to_string(), "a".to_string())];
        let inc = vec![
            ("user".to_string(), "a".to_string()),
            ("assistant".to_string(), "p".to_string()),
            ("user".to_string(), "b".to_string()),
            ("user".to_string(), "c".to_string()),
        ];
        assert_eq!(Prompt::followup_text(&prev, &inc).as_deref(), Some("b\n\nc"));
        let bad = vec![
            ("user".to_string(), "a".to_string()),
            ("tool".to_string(), "t".to_string()),
            ("user".to_string(), "b".to_string()),
        ];
        // Tool turn is stripped only when leading; here it follows "a" directly
        // after the prefix... it IS leading in `extra`, so it is stripped.
        assert_eq!(Prompt::followup_text(&prev, &bad).as_deref(), Some("b"));
        let changed = vec![("user".to_string(), "zzz".to_string()), ("user".to_string(), "b".to_string())];
        assert_eq!(Prompt::followup_text(&prev, &changed), None);
        assert_eq!(Prompt::followup_text(&prev, &prev), None);
    }

    #[test]
    fn local_continuation_skips_history_before_last_plan() {
        let turns = vec![
            ("system".to_string(), "ctx".to_string()),
            ("user".to_string(), "do X".to_string()),
            ("assistant".to_string(), "plan".to_string()),
            ("user".to_string(), "continue".to_string()),
        ];
        let out = Prompt::local_continuation(&turns, "English");
        assert!(out.contains("\"text\":\"continue\""), "{out}");
        assert!(!out.contains("\"text\":\"do X\""), "{out}");
        assert!(!out.contains("\"text\":\"ctx\""), "{out}");
    }
}
