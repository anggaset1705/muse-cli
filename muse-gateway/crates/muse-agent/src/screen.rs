//! Screen (computer) tools — the trait boundary for UIA control.
//!
//! The real implementation lives in the `muse-screen` crate (Windows-only
//! UIAutomation); this crate only defines the trait plus the honest
//! "unavailable in this host" behaviour that .NET `ToolExecutor.ComputerAsync`
//! shows when no `IComputerTools` is attached. `ui_*` actions are never
//! implemented here — they are delegated to the trait.

use async_trait::async_trait;

use crate::error::{AgentError, AgentResult, CancellationToken};
use crate::parser::ToolCall;

/// Result of one computer tool call: display text plus, for screenshots,
/// an optional image data URL.
#[derive(Debug, Clone)]
pub struct ScreenResult {
    pub text: String,
    pub image_data_url: Option<String>,
}

impl ScreenResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image_data_url: None,
        }
    }
}

/// Screen driver provided by the `muse-screen` crate at integration time.
/// Mirrors .NET `IComputerTools` (`Context` + `ExecuteAsync`).
#[async_trait]
pub trait ScreenDriver: Send + Sync {
    /// Short description of the screen scope, shown in workspace context.
    fn context(&self) -> String;

    /// Language-aware [`ScreenDriver::context`]: the chooser tab is
    /// named "Layar" for Bahasa Indonesia and "Screen" otherwise.
    /// Defaults to `context()`; drivers backed by `muse-screen`
    /// override it.
    fn context_for(&self, _reply_language: &str) -> String {
        self.context()
    }

    /// Execute one `computer_*` tool call.
    async fn execute(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> AgentResult<ScreenResult>;
}

/// The error text .NET returns when no screen driver is attached
/// (`ComputerTools.cs`).
pub const SCREEN_UNAVAILABLE: &str =
    "error: Computer tools are unavailable in this host. Use the Muse desktop application.";

/// A screen driver that always reports unavailable — used when the
/// integrator does not attach a real `muse-screen` driver.
pub struct NoScreen;

#[async_trait]
impl ScreenDriver for NoScreen {
    fn context(&self) -> String {
        "unavailable".to_string()
    }

    async fn execute(
        &self,
        _call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> AgentResult<ScreenResult> {
        Err(AgentError::Message(
            "Computer tools are unavailable in this host. Use the Muse desktop application."
                .to_string(),
        ))
    }
}
