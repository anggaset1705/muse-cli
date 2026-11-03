//! Error type for screen control.
//!
//! Messages reuse the English strings from the .NET implementation
//! (`DesktopComputer.cs`, `NativeDesktop.cs`, `ComputerGeometry.cs`) so a
//! caller comparing behaviour against .NET sees the same wording.

use std::fmt;

/// Errors that can come out of screen control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenError {
    /// The operation is not implemented on this host / in this build.
    /// This is the honest answer — never a faked success.
    NotImplemented(String),
    /// Screen control exists in principle but is not usable right now
    /// (no target window, no authorization, …).
    Unavailable(String),
    /// The caller supplied an invalid argument (bad key chord, bad
    /// coordinates, missing `element_id`, out-of-range option, …).
    InvalidInput(String),
    /// The `element_id` was never issued by the current inspection, or
    /// the inspection is older than 180 s (`.NET`: `DesktopComputer.cs`
    /// `Element()`).
    StaleElement,
    /// The element changed (name / automation id / role) since it was
    /// inspected, or it no longer belongs to the allowed window.
    ElementChanged,
    /// The target window closed, its process restarted, or the app changed
    /// since it was authorized.
    WindowClosed,
    /// Windows blocked the action: secure window, app running with higher
    /// privileges, point covered by another window, modifier keys held, …
    Blocked(String),
    /// A UI operation did not finish within the timeout (12 s, like the
    /// .NET `ComputerWorker` operation timeout).
    Timeout,
    /// A UIAutomation / COM call failed (HRESULT text included).
    Uia(String),
}

impl fmt::Display for ScreenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScreenError::NotImplemented(m) => write!(f, "not implemented: {m}"),
            ScreenError::Unavailable(m) => write!(f, "{m}"),
            ScreenError::InvalidInput(m) => write!(f, "{m}"),
            ScreenError::StaleElement => write!(
                f,
                "The element ID is stale or does not belong to this read. Read the UI again."
            ),
            ScreenError::ElementChanged => write!(
                f,
                "The element label or type changed since the last observation. Read the UI again to avoid acting on the wrong element."
            ),
            ScreenError::WindowClosed => write!(
                f,
                "The target window is closed. Please choose the window to test again."
            ),
            ScreenError::Blocked(m) => write!(f, "{m}"),
            ScreenError::Timeout => write!(
                f,
                "The target app or UI Automation is not responding. Control permission was revoked; the operation will not be resent."
            ),
            ScreenError::Uia(m) => write!(f, "UI Automation failed: {m}"),
        }
    }
}

impl std::error::Error for ScreenError {}

/// Convenience alias used across the crate.
pub type ScreenResult<T> = Result<T, ScreenError>;
