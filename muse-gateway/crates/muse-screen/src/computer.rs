//! `DesktopComputer` — the stateful screen-control session.
//!
//! Port of `DesktopComputer` (`DesktopComputer.cs`, .NET). The .NET class
//! runs UIAutomation on a dedicated MTA worker thread (`ComputerWorker.cs`)
//! and guards every action with:
//!
//! * a target window (authorized explicitly or auto-picked),
//! * an element cache that expires 180 s after the last inspection and is
//!   identity-checked (name / automation id / role) before each use,
//! * a screenshot snapshot that expires 120 s after capture and is
//!   invalidated by any mutating action,
//! * a generation counter that revokes control when the worker times out
//!   or the user disarms.
//!
//! On non-Windows all control methods return
//! [`ScreenError::NotImplemented`]; `status()` (see `status.rs`) still
//! works. The interactive permission dialog of the .NET desktop host
//! (`RequestAuthorization`) is deliberately *not* part of this crate: the
//! host (Tauri app / agent) calls [`DesktopComputer::authorize`] or
//! [`DesktopComputer::try_auto_authorize`] when the user has approved a
//! window.

use crate::actions::{ScreenAction, ScreenArgs, ScreenOutcome};
use crate::error::{ScreenError, ScreenResult};
use crate::geometry::DesktopWindow;
use crate::policy;
use std::sync::atomic::AtomicU64;
#[cfg(not(windows))]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Emergency-stop hotkey id (`.NET`: `EmergencyHotkey = 0x4D55`).
pub const EMERGENCY_HOTKEY: i32 = 0x4D55;

/// Message returned when no computer is available on this host — the same
/// string `ToolExecutor.ComputerAsync` (.NET) returns when `Computer` is
/// null.
#[cfg(not(windows))]
pub const UNAVAILABLE_MESSAGE: &str =
    "Computer tools are unavailable in this host. Use the Muse desktop application.";

/// Shared, synchronously-revokable session state.
///
/// The worker thread (Windows) reads this on every action, so a timeout or
/// an explicit [`DesktopComputer::disarm`] takes effect immediately even
/// while a UIA call is still stuck — the same role as .NET's `_target` +
/// `_generation` fields.
#[derive(Default)]
pub(crate) struct Shared {
    pub target: Mutex<Option<DesktopWindow>>,
    pub generation: AtomicU64,
}

/// The screen-control session.
pub struct DesktopComputer {
    shared: Arc<Shared>,
    #[cfg(windows)]
    worker: crate::win::UiWorker,
}

impl DesktopComputer {
    /// Create a new session (no target window yet).
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        DesktopComputer {
            shared: Arc::clone(&shared),
            #[cfg(windows)]
            worker: crate::win::UiWorker::new(shared),
        }
    }

    /// The currently authorized target window, if any.
    pub fn target(&self) -> Option<DesktopWindow> {
        self.shared.target.lock().ok()?.clone()
    }

    /// Context string for the agent prompt — mirror of .NET `Context`.
    /// English by default; use [`DesktopComputer::context_for`] when the
    /// reply language is known so the tab name matches the app UI
    /// ("Layar" in Bahasa Indonesia, "Screen" otherwise).
    pub fn context(&self) -> String {
        self.context_for("English")
    }

    /// Language-aware [`DesktopComputer::context`]. The chooser tab is
    /// named "Layar" for Bahasa Indonesia and "Screen" for any other
    /// reply language — never the upstream Vietnamese chooser name.
    pub fn context_for(&self, reply_language: &str) -> String {
        match self.target() {
            Some(t) => format!(
                "enabled for selected window and its owned dialogs/menus: {}, process {}. Use computer_inspect for fresh element_id. Screenshot coordinates are IMAGE pixels, not UIA physical bounds; use image_scale to convert. Captures expire after any UI action. Esc stops control.",
                t.title, t.process_id
            ),
            None => {
                let tab = if reply_language == "Bahasa Indonesia" {
                    "Layar"
                } else {
                    "Screen"
                };
                format!(
                    "disabled. The window is auto-selected on the first computer call when possible; otherwise ask the user to select a test window using the {tab} tab in the app. If the app is not open at all yet, open it first with run_command (for example `start <url>`), then call computer_inspect again. Never attempt screen control through shell commands."
                )
            }
        }
    }

    /// Authorize a target window (`.NET`: `Authorize`).
    ///
    /// Validates that the window still exists, belongs to the stated
    /// process, is not one of Muse's own windows, and — when
    /// `process_started` is set — that the process has not restarted.
    pub fn authorize(&self, target: DesktopWindow) -> ScreenResult<()> {
        #[cfg(windows)]
        {
            self.worker.authorize(target)?;
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let _ = target;
            Err(ScreenError::NotImplemented(UNAVAILABLE_MESSAGE.to_string()))
        }
    }

    /// Auto-select a target window without any dialog (`.NET`:
    /// `TryAutoAuthorize`): hint match, then the foreground window (or the
    /// top-level window owning it), then the topmost window in Z-order.
    /// Returns `false` when nothing safe can be picked.
    pub fn try_auto_authorize(&self, preferred_hint: Option<&str>) -> ScreenResult<bool> {
        #[cfg(windows)]
        {
            return self.worker.try_auto_authorize(preferred_hint);
        }
        #[cfg(not(windows))]
        {
            let _ = preferred_hint;
            Err(ScreenError::NotImplemented(UNAVAILABLE_MESSAGE.to_string()))
        }
    }

    /// Revoke control immediately (`.NET`: `Disarm`): clears the target and
    /// the screenshot snapshot, bumps the generation so any in-flight
    /// worker validation fails.
    pub fn disarm(&self) {
        #[cfg(windows)]
        {
            self.worker.disarm();
        }
        #[cfg(not(windows))]
        {
            self.shared.generation.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut t) = self.shared.target.lock() {
                *t = None;
            }
        }
    }

    /// Reset per-turn authorization flags (`.NET`: `BeginTurn`). This crate
    /// has no host permission callback, so this is a no-op kept for API
    /// parity with the .NET session lifecycle.
    pub fn begin_turn(&self) {}

    /// List top-level windows sorted by title (`.NET`: `ListWindows`).
    pub fn list_windows(&self) -> ScreenResult<Vec<DesktopWindow>> {
        #[cfg(windows)]
        {
            return self.worker.list_windows();
        }
        #[cfg(not(windows))]
        {
            Err(ScreenError::NotImplemented(UNAVAILABLE_MESSAGE.to_string()))
        }
    }

    /// Live preview of the target window without focusing it or replacing
    /// the tool snapshot (`.NET`: `PreviewAsync`). Returns `Ok(None)` when
    /// there is no target.
    pub fn preview(&self) -> ScreenResult<Option<String>> {
        #[cfg(windows)]
        {
            if self.target().is_none() {
                return Ok(None);
            }
            return self.worker.preview();
        }
        #[cfg(not(windows))]
        {
            Ok(None)
        }
    }

    /// Execute one computer action (`.NET`: `ExecuteAsync` + `Execute`).
    ///
    /// Argument validation runs on every platform first; on non-Windows
    /// the call then fails with `NotImplemented`.
    ///
    /// On Windows the first call auto-authorizes a target window
    /// (`.NET`: `RequestComputerAuthorization` → `TryAutoAuthorize`),
    /// using `args.preferred_hint` when set — without this the agent
    /// loop could never drive a window the user did not pick first
    /// (e.g. "buka browser dan ketik google.com" after the browser was
    /// just launched).
    pub fn execute(
        &self,
        action: ScreenAction,
        args: ScreenArgs,
    ) -> ScreenResult<ScreenOutcome> {
        policy::validate(&action, &args)?;
        #[cfg(windows)]
        {
            if self.target().is_none() {
                self.try_auto_authorize(args.preferred_hint.as_deref())?;
            }
            if action == ScreenAction::ListWindows {
                let target = self.require_target()?;
                let windows = vec![target];
                let text = serde_json::to_string(&windows).unwrap_or_default();
                return Ok(ScreenOutcome::text(text));
            }
            let _target = self.require_target()?;
            return self.worker.execute(action, args);
        }
        #[cfg(not(windows))]
        {
            let _ = action;
            let _ = args;
            Err(ScreenError::NotImplemented(UNAVAILABLE_MESSAGE.to_string()))
        }
    }

    #[cfg(windows)]
    fn require_target(&self) -> ScreenResult<DesktopWindow> {
        self.target().ok_or_else(|| {
            ScreenError::Unavailable(
                "Screen control is not on and no window could be auto-selected. If the app you want to operate is not open yet, open it first with run_command background:true (for example `start https://shellauction.net` opens the browser at that page), wait a moment, then call computer_inspect again — the newly opened window is auto-selected. Only ask the user to pick a window in the Screen tab when the app is already open but the wrong window was selected."
                    .to_string(),
            )
        })
    }

    /// Register the Esc emergency-stop hotkey on a host window
    /// (`.NET`: `RegisterEmergencyStop`). Returns `false` when registration
    /// failed or on non-Windows.
    pub fn register_emergency_stop(window_handle: i64) -> bool {
        #[cfg(windows)]
        {
            return crate::win::register_emergency_stop(window_handle);
        }
        #[cfg(not(windows))]
        {
            let _ = window_handle;
            false
        }
    }

    /// Unregister the Esc emergency-stop hotkey (`.NET`:
    /// `UnregisterEmergencyStop`).
    pub fn unregister_emergency_stop(window_handle: i64) {
        #[cfg(windows)]
        crate::win::unregister_emergency_stop(window_handle);
        #[cfg(not(windows))]
        let _ = window_handle;
    }
}

impl Default for DesktopComputer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computer() -> DesktopComputer {
        DesktopComputer::new()
    }

    #[test]
    fn context_reports_disabled_without_target() {
        assert!(computer().context().starts_with("disabled."));
        assert!(computer().target().is_none());
    }

    #[test]
    fn context_for_names_the_tab_in_the_reply_language() {
        // The old upstream Vietnamese chooser name, spelled with
        // escapes so the literal never appears in source.
        const FORBIDDEN: &str = "M\u{e0}n h\u{ec}nh";
        let id = computer().context_for("Bahasa Indonesia");
        assert!(id.contains("Layar tab in the app"), "{id}");
        assert!(!id.contains(FORBIDDEN), "{id}");
        let en = computer().context_for("English");
        assert!(en.contains("Screen tab in the app"), "{en}");
        assert!(!en.contains(FORBIDDEN), "{en}");
        // The language-less default stays English.
        assert!(computer().context().contains("Screen tab in the app"));
    }

    #[test]
    fn disarm_is_safe_without_target() {
        let c = computer();
        c.disarm();
        assert!(c.target().is_none());
    }

    #[test]
    fn preview_without_target_is_none() {
        assert_eq!(computer().preview().unwrap(), None);
    }

    /// Every action on non-Windows must fail with a clear NotImplemented —
    /// never a faked success.
    #[test]
    #[cfg(not(windows))]
    fn all_actions_not_implemented_off_windows() {
        let c = computer();
        let w = DesktopWindow {
            handle: 1,
            process_id: 1,
            title: "x".to_string(),
            process_started: 0,
            application_name: "x".to_string(),
        };
        for (action, args) in [
            (ScreenAction::Inspect, ScreenArgs::default()),
            (ScreenAction::Screenshot, ScreenArgs::default()),
            (ScreenAction::Focus, ScreenArgs::default()),
            (ScreenAction::ListWindows, ScreenArgs::default()),
        ] {
            let err = c.execute(action, args).unwrap_err();
            assert!(
                matches!(err, ScreenError::NotImplemented(_)),
                "{action:?} should be NotImplemented, got {err}"
            );
        }
        assert!(matches!(
            c.authorize(w),
            Err(ScreenError::NotImplemented(_))
        ));
        assert!(matches!(
            c.try_auto_authorize(None),
            Err(ScreenError::NotImplemented(_))
        ));
        assert!(matches!(
            c.list_windows(),
            Err(ScreenError::NotImplemented(_))
        ));
        assert!(!DesktopComputer::register_emergency_stop(1));
    }

    #[test]
    #[cfg(not(windows))]
    fn validation_runs_before_not_implemented() {
        // A bad key chord must be reported as InvalidInput, not masked by
        // the platform stub.
        let c = computer();
        let mut args = ScreenArgs::default();
        args.key = Some("ALT+TAB".to_string());
        assert!(matches!(
            c.execute(ScreenAction::Key, args),
            Err(ScreenError::InvalidInput(_))
        ));
    }
}
