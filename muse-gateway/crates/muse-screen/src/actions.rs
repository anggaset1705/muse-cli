//! Tool-call surface: action names, arguments, and outcomes.
//!
//! Wire names come from `ToolCallParser.cs` (`CONTRACTS.md` §8):
//! `ui_tree` → `computer_inspect`, `ui_image` → `computer_screenshot`,
//! `ui_press` → `computer_invoke`, `ui_fill` → `computer_set_value`,
//! `ui_type` → `computer_type`, `ui_key` → `computer_key`,
//! `ui_click` → `computer_click`, `ui_drag` → `computer_drag`,
//! `ui_focus` → `computer_focus`, `ui_scroll` → `computer_scroll`,
//! `ui_assert` → `computer_assert`, `ui_toggle` → `computer_toggle`,
//! `ui_select` → `computer_select`, `ui_expand` → `computer_expand`.
//! `computer_list_windows` is the extra action `DesktopComputer` answers
//! directly (`.NET`: `ExecuteAsync`).

use serde::{Deserialize, Serialize};

/// One computer tool action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenAction {
    /// `computer_inspect` — walk the UIA tree of the target window.
    Inspect,
    /// `computer_screenshot` — capture the target window as a JPEG.
    Screenshot,
    /// `computer_focus` — bring the target window (and optionally an
    /// element) to the front.
    Focus,
    /// `computer_invoke` — invoke (click) an element via InvokePattern.
    Invoke,
    /// `computer_set_value` — set an element's value via ValuePattern.
    SetValue,
    /// `computer_type` — type text into the focused / given element.
    Type,
    /// `computer_key` — press a key chord (see `policy::keys_for_chord`).
    Key,
    /// `computer_toggle` — set a toggle element on/off.
    Toggle,
    /// `computer_select` — select a selection-item element.
    Select,
    /// `computer_expand` — expand / collapse an element.
    Expand,
    /// `computer_scroll` — scroll an element, or scroll at image coords.
    Scroll,
    /// `computer_click` — click at image coordinates.
    Click,
    /// `computer_drag` — drag from one image point to another.
    Drag,
    /// `computer_assert` — assert an element property, with a short wait.
    Assert,
    /// `computer_list_windows` — list the authorized target window.
    ListWindows,
}

impl ScreenAction {
    /// Parse the `computer_*` tool name. Unknown names return `None`
    /// (`.NET` throws "Unknown computer tool" for those).
    pub fn from_tool_name(name: &str) -> Option<Self> {
        Some(match name {
            "computer_inspect" => ScreenAction::Inspect,
            "computer_screenshot" => ScreenAction::Screenshot,
            "computer_focus" => ScreenAction::Focus,
            "computer_invoke" => ScreenAction::Invoke,
            "computer_set_value" => ScreenAction::SetValue,
            "computer_type" => ScreenAction::Type,
            "computer_key" => ScreenAction::Key,
            "computer_toggle" => ScreenAction::Toggle,
            "computer_select" => ScreenAction::Select,
            "computer_expand" => ScreenAction::Expand,
            "computer_scroll" => ScreenAction::Scroll,
            "computer_click" => ScreenAction::Click,
            "computer_drag" => ScreenAction::Drag,
            "computer_assert" => ScreenAction::Assert,
            "computer_list_windows" => ScreenAction::ListWindows,
            _ => return None,
        })
    }

    /// The `computer_*` tool name for this action.
    pub fn tool_name(&self) -> &'static str {
        match self {
            ScreenAction::Inspect => "computer_inspect",
            ScreenAction::Screenshot => "computer_screenshot",
            ScreenAction::Focus => "computer_focus",
            ScreenAction::Invoke => "computer_invoke",
            ScreenAction::SetValue => "computer_set_value",
            ScreenAction::Type => "computer_type",
            ScreenAction::Key => "computer_key",
            ScreenAction::Toggle => "computer_toggle",
            ScreenAction::Select => "computer_select",
            ScreenAction::Expand => "computer_expand",
            ScreenAction::Scroll => "computer_scroll",
            ScreenAction::Click => "computer_click",
            ScreenAction::Drag => "computer_drag",
            ScreenAction::Assert => "computer_assert",
            ScreenAction::ListWindows => "computer_list_windows",
        }
    }
}

/// Arguments for a computer action, as a flat struct (all optional; the
/// policy layer in `policy.rs` enforces per-action requirements, mirroring
/// `ComputerActionPolicy.Validate`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenArgs {
    /// Element id from the latest `computer_inspect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    /// Text for `computer_type` / `computer_set_value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Key chord for `computer_key`, e.g. `"CTRL+C"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Image x coordinate (screenshot pixels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    /// Image y coordinate (screenshot pixels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// Drag destination image x coordinate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_x: Option<i32>,
    /// Drag destination image y coordinate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_y: Option<i32>,
    /// Mouse button for `computer_click`: `"left"` (default) or `"right"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    /// Double click for `computer_click`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_click: Option<bool>,
    /// Scroll direction: `"up"` / `"down"` / `"left"` / `"right"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// Desired toggle state: `"on"` / `"off"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Desired expand state for `computer_expand` (`true` = expand).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// Asserted property: `name` / `value` / `toggle` / `enabled` /
    /// `offscreen` / `selected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    /// Expected value for `computer_assert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// Comparison for `computer_assert`: `"equals"` (default) or
    /// `"contains"`.
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_: Option<String>,
    /// How long `computer_assert` may wait (ms, 0–5000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<i32>,
    /// Free-text filter for `computer_inspect` (name / automation id /
    /// role substring, case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Pagination offset for `computer_inspect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
    /// Page size for `computer_inspect` (1–180, default 80).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
    /// Maximum tree depth for `computer_inspect` (1–30, default 12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<i32>,
    /// Screenshot id from the latest `computer_screenshot`, required for
    /// coordinate actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// Preferred window hint for auto-authorization (title / app name
    /// substring).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_hint: Option<String>,
}

/// The result of a computer action — mirror of .NET `ComputerToolResult`.
///
/// `text` is the human/tool-readable result (for inspect and screenshot
/// it is a JSON document, like .NET). `image_data_url` carries the
/// screenshot as `data:image/jpeg;base64,…` when the action produced one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenOutcome {
    /// Result text (JSON for inspect/screenshot, status line otherwise).
    pub text: String,
    /// Screenshot data URL, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_data_url: Option<String>,
}

impl ScreenOutcome {
    /// Text-only outcome.
    pub fn text(text: impl Into<String>) -> Self {
        ScreenOutcome {
            text: text.into(),
            image_data_url: None,
        }
    }
}

/// One inspected UI element — the JSON node `.NET` emits per element in
/// `computer_inspect` (field names kept identical).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    /// Stable id for this inspection (`e<inspection>_<n>`).
    pub element_id: String,
    /// Depth in the tree (root = 0).
    pub depth: i32,
    /// Accessible name (max 600 chars, like .NET `Limit`).
    pub name: String,
    /// Automation id (max 600 chars).
    pub automation_id: String,
    /// Control type programmatic name, e.g. `"ControlType.Button"`.
    pub role: String,
    /// Whether the element is enabled.
    pub enabled: bool,
    /// Whether the element is offscreen.
    pub offscreen: bool,
    /// Current value (ValuePattern, max 600 chars), `""` when unsupported.
    pub value: String,
    /// Bounding rectangle in window physical pixels.
    pub bounds: UiBounds,
    /// Supported UIA pattern programmatic names (max 16).
    pub patterns: Vec<String>,
}

/// Bounding rectangle (window physical pixels) of an inspected element.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UiBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The JSON document `computer_inspect` returns as `text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectResult {
    pub window: String,
    pub active_window: i64,
    pub inspection_id: String,
    pub coordinate_space: String,
    pub elements: Vec<UiNode>,
    pub scanned: i32,
    pub next_offset: Option<i32>,
    pub truncated: bool,
    pub depth_limit_reached: bool,
    pub scan_limit_reached: bool,
    pub hint: String,
}

/// The JSON document `computer_screenshot` returns as `text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub snapshot_id: String,
    pub active_window: i64,
    pub width: i32,
    pub height: i32,
    pub window_width: i32,
    pub window_height: i32,
    pub image_fingerprint: String,
    pub image_scale: ImageScale,
    pub coordinate_space: String,
    pub window: String,
}

/// Screenshot scale factors (image pixels per physical pixel).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImageScale {
    pub x: f64,
    pub y: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_round_trip() {
        for name in [
            "computer_inspect",
            "computer_screenshot",
            "computer_focus",
            "computer_invoke",
            "computer_set_value",
            "computer_type",
            "computer_key",
            "computer_toggle",
            "computer_select",
            "computer_expand",
            "computer_scroll",
            "computer_click",
            "computer_drag",
            "computer_assert",
            "computer_list_windows",
        ] {
            let a = ScreenAction::from_tool_name(name).unwrap();
            assert_eq!(a.tool_name(), name);
        }
        assert!(ScreenAction::from_tool_name("computer_bogus").is_none());
        assert!(ScreenAction::from_tool_name("ui_click").is_none());
    }

    #[test]
    fn args_match_field_is_renamed() {
        let mut args = ScreenArgs::default();
        args.match_ = Some("contains".to_string());
        let v = serde_json::to_value(&args).unwrap();
        assert_eq!(v["match"], "contains");
        let back: ScreenArgs = serde_json::from_value(v).unwrap();
        assert_eq!(back.match_.as_deref(), Some("contains"));
    }
}
