//! `ScreenStatus` — the JSON shape of `GET /v1/agent/screen/status`.
//!
//! Source: `AgentEndpoints.cs` `ScreenStatus()`:
//!
//! ```json
//! {
//!   "available": false,
//!   "computer": "unavailable",
//!   "platform": "windows" | "linux" | "other",
//!   "reason": "Computer tools are unavailable in this host. Use the Muse desktop application."
//! }
//! ```
//!
//! The .NET gateway always reports `available: false` because the gateway
//! host has no computer implementation. This crate reports `available:
//! true` only on Windows *and* only when the UIAutomation runtime
//! (`UIAutomationCore.dll`) can actually be loaded; on Windows the
//! `computer` field then reads `"uia"` (the implementation this crate
//! ships) instead of `"unavailable"`. Field names and types are identical
//! in all cases.

use serde::{Deserialize, Serialize};

/// Status of screen control on this host, wire-compatible with .NET.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenStatus {
    /// Whether computer tools can run on this host.
    pub available: bool,
    /// Which computer implementation is behind `available`
    /// (`"unavailable"` when none, `"uia"` on Windows with UIAutomation).
    pub computer: String,
    /// `"windows"`, `"linux"`, or `"other"` — same rule as .NET
    /// (`OperatingSystem.IsWindows()` / `IsLinux()`).
    pub platform: String,
    /// Human-readable reason for the current state.
    pub reason: String,
}

/// Platform string, following the .NET rule.
#[cfg(not(windows))]
pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

#[cfg(not(windows))]
const UNAVAILABLE_REASON: &str =
    "Computer tools are unavailable in this host. Use the Muse desktop application.";

/// Build the status for this host.
pub fn status() -> ScreenStatus {
    #[cfg(windows)]
    {
        if crate::win::uia_runtime_available() {
            return ScreenStatus {
                available: true,
                computer: "uia".to_string(),
                platform: "windows".to_string(),
                reason: "UIAutomation is available on this Windows host.".to_string(),
            };
        }
        return ScreenStatus {
            available: false,
            computer: "unavailable".to_string(),
            platform: "windows".to_string(),
            reason: "UIAutomation runtime (UIAutomationCore.dll) could not be loaded on this Windows host."
                .to_string(),
        };
    }
    #[cfg(not(windows))]
    {
        ScreenStatus {
            available: false,
            computer: "unavailable".to_string(),
            platform: platform_name().to_string(),
            reason: UNAVAILABLE_REASON.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The non-Windows status must serialize to exactly the JSON the .NET
    /// gateway returns for the same host (field names, values, order is
    /// irrelevant to JSON but the value must match).
    #[test]
    #[cfg(not(windows))]
    fn status_matches_dotnet_gateway_json() {
        let s = status();
        let v = serde_json::to_value(&s).unwrap();
        let expected = serde_json::json!({
            "available": false,
            "computer": "unavailable",
            "platform": platform_name(),
            "reason": "Computer tools are unavailable in this host. Use the Muse desktop application.",
        });
        assert_eq!(v, expected);
    }

    #[test]
    fn status_round_trips() {
        let s = status();
        let json = serde_json::to_string(&s).unwrap();
        let back: ScreenStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
