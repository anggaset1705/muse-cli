//! Driver boundary — the trait the real browser driver implements.
//!
//! Per CONTRACTS.md §11 the `muse-browser` crate owns `MuseDriver`
//! (`chat(req) -> stream event`) and `MockDriver`. That crate does not
//! exist yet, so this crate defines the small trait itself (as the task
//! requires) and the coordinator wires the real driver at integration
//! time. Nothing here `use`s a crate that does not exist yet.
//!
//! `ChatRequest` carries the already-built planner messages (system prompt
//! + history + the current turn, built by `chat_turn`) together with the
//! flat prompt text the mock driver echoes.

use async_trait::async_trait;

use crate::error::{AgentError, AgentResult, CancellationToken};

/// One planner request to the driver.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Model name, e.g. `muse-chat`.
    pub model: String,
    /// Flattened prompt text (the current user turn) — what the mock
    /// driver echoes back.
    pub prompt: String,
    /// Attached image URLs (at most 4 per turn).
    pub images: Vec<String>,
    /// Language Muse should reply in, e.g. `English` / `Bahasa Indonesia`.
    pub reply_language: String,
    /// Agent session id (conversation hint, so the driver reuses the same
    /// upstream thread).
    pub session_id: Option<String>,
    /// Full planner message list (system prompt + history + user turn),
    /// as JSON objects with `role` / `content`.
    pub messages: Vec<serde_json::Value>,
}

/// Receives streamed reply deltas while the driver chats. The chat turn
/// forwards each delta as an `assistant_delta` SSE event.
pub trait DeltaSink: Send {
    fn on_delta(&mut self, text: &str);
}

/// The Muse driver: talks to muse.ai (via the browser) and streams the
/// planner reply. Implementations live in `muse-browser`.
#[async_trait]
pub trait MuseDriver: Send + Sync {
    /// Run one planner chat. Deltas are pushed to `sink` as they arrive;
    /// the returned string is the complete reply text.
    async fn chat(
        &self,
        req: ChatRequest,
        cancel: &CancellationToken,
        sink: &mut dyn DeltaSink,
    ) -> AgentResult<String>;
}

/// Offline driver, ported from .NET `Drivers.cs` `MockDriver` (itself
/// ported from Python `drivers/mock.py`). Needs no account and returns
/// deterministic output:
///
/// `[mock:{model}] You said: {last 200 chars of prompt}` (+
/// ` (with N image(s))` when images are attached), streamed in 8-char
/// chunks with a 1 ms delay each.
pub struct MockDriver {
    delay_ms: u64,
}

impl MockDriver {
    pub fn new() -> Self {
        Self { delay_ms: 1 }
    }

    pub fn with_delay(delay_ms: u64) -> Self {
        Self { delay_ms }
    }
}

impl Default for MockDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MuseDriver for MockDriver {
    async fn chat(
        &self,
        req: ChatRequest,
        cancel: &CancellationToken,
        sink: &mut dyn DeltaSink,
    ) -> AgentResult<String> {
        let tail_len = req.prompt.chars().count().min(200);
        let tail: String = req
            .prompt
            .chars()
            .skip(req.prompt.chars().count() - tail_len)
            .collect();
        let mut reply = format!("[mock:{}] You said: {}", req.model, tail);
        if !req.images.is_empty() {
            reply += &format!(" (with {} image(s))", req.images.len());
        }
        // Stream in 8-char chunks, like .NET.
        let chars: Vec<char> = reply.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            cancel.throw_if_cancelled()?;
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            cancel.throw_if_cancelled()?;
            let end = (i + 8).min(chars.len());
            let chunk: String = chars[i..end].iter().collect();
            sink.on_delta(&chunk);
            i = end;
        }
        Ok(reply)
    }
}

/// Error raised when a driver call fails; kept distinct so the chat turn
/// can surface it as an `error` SSE event instead of an incomplete turn.
pub fn driver_error(message: impl Into<String>) -> AgentError {
    AgentError::Message(message.into())
}
