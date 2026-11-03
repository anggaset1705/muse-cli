//! `muse-agent` — full Rust port of the .NET `Muse2Api.Agent` assembly.
//!
//! * [`parser`] — `ToolCallParser`: JSON action plans from Muse's replies.
//! * [`executor`] — `ToolExecutor`: workspace-jailed file/shell/process
//!   tools, stale-read guard, change journal + revert.
//! * [`session`] — `AgentSession` / `AgentSessionManager` (+ preferences).
//! * [`chat_turn`] — the cancellable chat/tool loop (`AgentLoop`) and the
//!   SSE event shapes from CONTRACTS.md §7.
//! * [`driver`] — the `MuseDriver` trait (+ `MockDriver`) that the
//!   `muse-browser` crate implements for real.
//! * [`screen`] — the `ScreenDriver` trait that the `muse-screen` crate
//!   implements (Windows UIA); `ui_*` actions are never implemented here.
//! * [`approval`] — Ask/Turbo approval + `TurboApprovalPolicy`.
//! * [`intent`] — `LocalTaskIntent` / `WorkflowPolicy` completion checks.
//! * [`goal`] — `GoalRun` + `GoalVerification`.
//! * [`context`] — conversation context fitting (simplified archive).
//! * [`system_prompt`] — the planner system prompt (`AgentClient`).
//! * [`tool_outcome`] — result classification (`ToolOutcome`).
//! * [`text_diff`] — single-hunk review diffs (`TextDiff`).
//! * [`workspace`] — workspace jailing + enumeration (`WorkspaceFiles`).
//! * [`shell`] / [`processes`] — shell resolution + process execution.
//! * [`error`] — `AgentError` + `CancellationToken`.

pub mod approval;
pub mod chat_turn;
pub mod context;
pub mod driver;
pub mod error;
pub mod executor;
pub mod goal;
pub mod intent;
pub mod parser;
pub mod processes;
pub mod screen;
pub mod session;
pub mod shell;
pub mod system_prompt;
pub mod text_diff;
pub mod tool_outcome;
pub mod workspace;

pub use error::{AgentError, AgentResult, CancellationToken};
