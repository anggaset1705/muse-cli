//! Screen control — Rust port of `Muse2Api.Computer` (.NET).
//!
//! The .NET implementation drives the Windows desktop through UIAutomation
//! (tree inspection, screenshots, focus/click/type/key/drag/scroll and the
//! UIA patterns invoke / value / toggle / select / expand-collapse) plus
//! raw Win32 input (`SendInput`). This crate mirrors that surface:
//!
//! * [`status()`] — the same JSON shape as `GET /v1/agent/screen/status`
//!   in the .NET gateway (`AgentEndpoints.cs`, `ScreenStatus()`).
//! * [`DesktopComputer`] — the stateful computer: target-window selection
//!   (auto-pick), element cache with staleness/identity guards, screenshot
//!   snapshots, and one action at a time on a dedicated MTA worker thread
//!   (mirror of `ComputerWorker.cs`).
//! * [`ScreenAction`] / [`ScreenArgs`] — the `computer_*` tool calls
//!   (`ToolCallParser.cs` wire names `ui_tree`, `ui_image`, … map to these
//!   in `muse-agent`; see `CONTRACTS.md` §8).
//!
//! Windows-only by nature: on any other OS every control method returns
//! [`ScreenError::NotImplemented`] with a clear message, while [`status()`]
//! still runs and reports `available: false` — exactly like the .NET
//! gateway host, which has no computer implementation either.

mod actions;
mod computer;
mod error;
mod geometry;
mod policy;
mod status;
#[cfg(windows)]
mod win;

pub use actions::{
    ImageScale, InspectResult, ScreenAction, ScreenArgs, ScreenOutcome, ScreenshotResult,
    UiBounds, UiNode,
};
pub use computer::{DesktopComputer, EMERGENCY_HOTKEY};
pub use error::{ScreenError, ScreenResult};
pub use geometry::{DesktopWindow, ScreenshotMap, WindowAutoPicker};
pub use policy::{keys_for_chord, mutates};
pub use status::{ScreenStatus, status};
