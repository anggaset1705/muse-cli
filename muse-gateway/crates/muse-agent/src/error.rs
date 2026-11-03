//! Error type + a small cloneable cancellation token (the Rust analogue of
//! .NET `CancellationToken` / `CancellationTokenSource`).

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Errors raised by the agent crate. Tool-execution failures are NOT raised
/// as errors — like .NET `ToolExecutor.ExecuteAsync` they are returned as
/// `"error: ..."` result strings; `AgentError` is for control-flow and
/// programming errors (cancellation, invalid input, I/O).
#[derive(Debug)]
pub enum AgentError {
    /// The operation was cancelled (user pressed cancel / request aborted).
    Cancelled,
    /// Invalid input (bad path, bad arguments, unknown session, ...).
    Invalid(String),
    /// An I/O error.
    Io(std::io::Error),
    /// Any other failure, with a plain message.
    Message(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Cancelled => write!(f, "cancelled"),
            AgentError::Invalid(msg) => write!(f, "{msg}"),
            AgentError::Io(err) => write!(f, "{err}"),
            AgentError::Message(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AgentError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AgentError {
    fn from(err: std::io::Error) -> Self {
        AgentError::Io(err)
    }
}

impl From<serde_json::Error> for AgentError {
    fn from(err: serde_json::Error) -> Self {
        AgentError::Invalid(err.to_string())
    }
}

impl From<tokio::sync::oneshot::error::RecvError> for AgentError {
    fn from(_: tokio::sync::oneshot::error::RecvError) -> Self {
        AgentError::Cancelled
    }
}

pub type AgentResult<T> = Result<T, AgentError>;

/// Cloneable cancellation token, mirroring .NET `CancellationTokenSource`.
/// `cancel()` wakes every waiter; `cancelled()` resolves once cancelled.
#[derive(Clone, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token that is already cancelled.
    pub fn cancelled_token() -> Self {
        let token = Self::new();
        token.cancel();
        token
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub fn throw_if_cancelled(&self) -> AgentResult<()> {
        if self.is_cancelled() {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Future that completes when the token is cancelled. Returns
    /// immediately if already cancelled.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        // `Notify::notified()` can miss a notification that fires between
        // the check above and the await; re-check after enabling the waiter.
        let fut = self.notify.notified();
        tokio::pin!(fut);
        // Enable the waiter first, then re-check.
        fut.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        fut.await;
    }
}

/// Small JSON-object helpers, mirroring the `Str`/`Bool`/`Int` helpers in
/// `Tools.cs` / `ToolEditing.cs`.
pub mod json {
    use serde_json::{Map, Value};

    pub fn str_arg(args: &Map<String, Value>, name: &str, fallback: &str) -> String {
        match args.get(name) {
            Some(Value::String(s)) => s.clone(),
            _ => fallback.to_string(),
        }
    }

    pub fn opt_str_arg(args: &Map<String, Value>, name: &str) -> Option<String> {
        match args.get(name) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn bool_arg(args: &Map<String, Value>, name: &str, fallback: bool) -> bool {
        match args.get(name) {
            Some(Value::Bool(b)) => *b,
            _ => fallback,
        }
    }

    pub fn int_arg(args: &Map<String, Value>, name: &str, fallback: i64) -> i64 {
        match args.get(name) {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(fallback),
            _ => fallback,
        }
    }

    pub fn truncate(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            s.to_string()
        } else {
            let head: String = s.chars().take(max).collect();
            format!("{head}\n... [truncated, {} chars total]", s.chars().count())
        }
    }
}
