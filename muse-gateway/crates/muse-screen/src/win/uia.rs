//! UIAutomation actions — port of the `Execute` switch in
//! `DesktopComputer.cs` (.NET), plus the element/scope guards.
//!
//! Runs entirely on the MTA worker thread (see `win/mod.rs`). Every COM
//! call is `unsafe` by nature of the `windows` crate bindings; the safety
//! argument is the same as .NET's: one dedicated MTA thread owns all
//! UIAutomation objects, and calls are serialized through the worker queue.

use super::native;
use super::UiState;
use crate::actions::{
    InspectResult, ScreenAction, ScreenArgs, ScreenOutcome, UiBounds, UiNode,
};
use crate::error::{ScreenError, ScreenResult};
use crate::geometry::DesktopWindow;
use crate::policy;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationScrollPattern, IUIAutomationSelectionItemPattern,
    IUIAutomationTogglePattern, IUIAutomationValuePattern, ScrollAmount_LargeDecrement,
    ScrollAmount_LargeIncrement, ScrollAmount_NoAmount, ToggleState_Off, ToggleState_On,
    UIA_CONTROLTYPE_ID, UIA_ExpandCollapsePatternId,
    UIA_InvokePatternId, UIA_ScrollPatternId, UIA_SelectionItemPatternId,
    UIA_TogglePatternId, UIA_ValuePatternId,
};
use windows::core::BSTR;

/// Element identity snapshot, checked before every interactive use
/// (`.NET`: `_identities`).
#[derive(Debug, Clone)]
pub struct ElementIdentity {
    pub name: String,
    pub automation_id: String,
    pub role: String,
}

/// Cached properties of one element, read defensively (any failing
/// property falls back to a default — `.NET` does the same per element
/// inside the inspect loop).
#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub name: String,
    pub automation_id: String,
    pub role: String,
    pub enabled: bool,
    pub offscreen: bool,
    pub is_password: bool,
    pub process_id: i32,
    pub native_window_handle: i64,
    pub has_keyboard_focus: bool,
    /// left, top, right, bottom (physical pixels).
    pub bounds: (i32, i32, i32, i32),
}

fn limit(text: &str) -> String {
    text.chars().take(600).collect()
}

fn bstr_to_string(b: BSTR) -> String {
    // BSTR derefs to [u16].
    String::from_utf16_lossy(&b)
}

/// Control-type programmatic names, matching .NET
/// `ControlType.ProgrammaticName` (`"ControlType.Button"`, …).
pub fn control_type_name(id: UIA_CONTROLTYPE_ID) -> &'static str {
    match id.0 {
        50000 => "ControlType.Button",
        50001 => "ControlType.Calendar",
        50002 => "ControlType.CheckBox",
        50003 => "ControlType.ComboBox",
        50004 => "ControlType.Edit",
        50005 => "ControlType.Hyperlink",
        50006 => "ControlType.Image",
        50007 => "ControlType.ListItem",
        50008 => "ControlType.List",
        50009 => "ControlType.Menu",
        50010 => "ControlType.MenuBar",
        50011 => "ControlType.MenuItem",
        50012 => "ControlType.ProgressBar",
        50013 => "ControlType.RadioButton",
        50014 => "ControlType.ScrollBar",
        50015 => "ControlType.Slider",
        50016 => "ControlType.Spinner",
        50017 => "ControlType.StatusBar",
        50018 => "ControlType.Tab",
        50019 => "ControlType.TabItem",
        50020 => "ControlType.Text",
        50021 => "ControlType.ToolBar",
        50022 => "ControlType.ToolTip",
        50023 => "ControlType.Tree",
        50024 => "ControlType.TreeItem",
        50025 => "ControlType.Custom",
        50026 => "ControlType.Group",
        50027 => "ControlType.Thumb",
        50028 => "ControlType.DataGrid",
        50029 => "ControlType.DataItem",
        50030 => "ControlType.Document",
        50031 => "ControlType.SplitButton",
        50032 => "ControlType.Window",
        50033 => "ControlType.Pane",
        50034 => "ControlType.Header",
        50035 => "ControlType.HeaderItem",
        50036 => "ControlType.Table",
        50037 => "ControlType.TitleBar",
        50038 => "ControlType.Separator",
        _ => "ControlType.Custom",
    }
}

/// UIA pattern ids probed for the `patterns` field of an inspected node
/// (`.NET`: `GetSupportedPatterns` — raw UIA has no such call, so .NET's
/// wrapper probes the known patterns the same way).
const PATTERNS: &[(i32, &str)] = &[
    (10000, "InvokePattern"),
    (10001, "SelectionPattern"),
    (10002, "ValuePattern"),
    (10003, "RangeValuePattern"),
    (10004, "ScrollPattern"),
    (10005, "ExpandCollapsePattern"),
    (10006, "GridPattern"),
    (10007, "GridItemPattern"),
    (10008, "MultipleViewPattern"),
    (10009, "WindowPattern"),
    (10010, "SelectionItemPattern"),
    (10011, "DockPattern"),
    (10012, "TablePattern"),
    (10013, "TableItemPattern"),
    (10014, "TextPattern"),
    (10015, "TogglePattern"),
    (10016, "TransformPattern"),
    (10017, "ScrollItemPattern"),
    (10018, "LegacyIAccessiblePattern"),
    (10019, "ItemContainerPattern"),
    (10020, "VirtualizedItemPattern"),
    (10023, "SynchronizedInputPattern"),
    (10024, "ObjectModelPattern"),
    (10025, "AnnotationPattern"),
    (10026, "StylesPattern"),
    (10027, "SpreadsheetPattern"),
    (10028, "TextPattern2"),
    (10029, "TextChildPattern"),
    (10030, "DragPattern"),
    (10031, "DropTargetPattern"),
    (10032, "TextEditPattern"),
    (10033, "CustomNavigationPattern"),
];

fn element_info(element: &IUIAutomationElement) -> ElementInfo {
    unsafe {
        let rect: RECT = element.CurrentBoundingRectangle().unwrap_or_default();
        ElementInfo {
            name: element
                .CurrentName()
                .map(bstr_to_string)
                .unwrap_or_default(),
            automation_id: element
                .CurrentAutomationId()
                .map(bstr_to_string)
                .unwrap_or_default(),
            role: element
                .CurrentControlType()
                .map(control_type_name)
                .unwrap_or("ControlType.Custom")
                .to_string(),
            enabled: element.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(false),
            offscreen: element
                .CurrentIsOffscreen()
                .map(|b| b.as_bool())
                .unwrap_or(false),
            is_password: element
                .CurrentIsPassword()
                .map(|b| b.as_bool())
                .unwrap_or(false),
            process_id: element.CurrentProcessId().unwrap_or(0),
            native_window_handle: element
                .CurrentNativeWindowHandle()
                .map(|h| h.0 as i64)
                .unwrap_or(0),
            has_keyboard_focus: element
                .CurrentHasKeyboardFocus()
                .map(|b| b.as_bool())
                .unwrap_or(false),
            bounds: (rect.left, rect.top, rect.right, rect.bottom),
        }
    }
}

fn supported_patterns(element: &IUIAutomationElement) -> Vec<String> {
    let mut out = Vec::new();
    for &(id, name) in PATTERNS {
        if out.len() >= 16 {
            break;
        }
        unsafe {
            if element
                .GetCurrentPattern(windows::Win32::UI::Accessibility::UIA_PATTERN_ID(id))
                .is_ok()
            {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn element_value(element: &IUIAutomationElement) -> String {
    unsafe {
        match element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
            Ok(p) => p.CurrentValue().map(bstr_to_string).unwrap_or_default(),
            Err(_) => String::new(),
        }
    }
}

/// 12-hex-digit id for inspections / snapshots (`.NET`: `Guid "N"[..12]`).
/// Uniqueness comes from a process-wide counter seeded with the clock.
pub fn new_short_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst) ^ seed;
    format!("{:012x}", n & 0xFFFF_FFFF_FFFF)
}

pub(crate) fn lock_target(state: &UiState) -> Option<DesktopWindow> {
    state
        .shared
        .target
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

/// Re-validate the session before / during an action (`.NET`: `Validate`).
///
/// Fails when control was revoked (generation changed), the target is gone,
/// its process restarted, or the app changed.
pub fn validate(state: &mut UiState) -> ScreenResult<()> {
    if state.generation != state.shared.generation.load(Ordering::SeqCst) {
        // Control was revoked: drop everything tied to the old session.
        state.elements.clear();
        state.identities.clear();
        state.snapshot.clear();
        state.image_map = None;
        return Err(ScreenError::Unavailable(
            "Screen control was stopped.".to_string(),
        ));
    }
    let target = lock_target(state).ok_or_else(|| {
        ScreenError::Unavailable("Screen control was stopped.".to_string())
    })?;
    let handle = native::hwnd(target.handle);
    if !native::is_window(handle)
        || native::window_process_id(handle) != target.process_id as u32
    {
        return Err(ScreenError::WindowClosed);
    }
    if target.process_started != 0 {
        match native::process_start_ticks(target.process_id as u32) {
            Some(started) if started == target.process_started => {}
            _ => {
                return Err(ScreenError::WindowClosed);
            }
        }
    }
    Ok(())
}

/// Guard for one element (`.NET`: `CheckElement`).
fn check_element(
    state: &UiState,
    element: &IUIAutomationElement,
    interactive: bool,
) -> ScreenResult<()> {
    let target = lock_target(state).ok_or_else(|| {
        ScreenError::Unavailable("Screen control was stopped.".to_string())
    })?;
    let window = native::hwnd(target.handle);
    let self_pid = std::process::id() as i32;
    let info = element_info(element);
    if info.process_id == self_pid
        || info.is_password
        || interactive && (!info.enabled || info.offscreen)
    {
        return Err(ScreenError::Blocked(
            "The element is hidden, locked, a password field, or belongs to Muse itself. Read the UI again."
                .to_string(),
        ));
    }
    // Chromium/Electron UIA descendants may live in renderer processes;
    // check actual ancestry instead of requiring one PID.
    let mut current = Some(element.clone());
    for _ in 0..64 {
        let Some(el) = current else { break };
        let info = element_info(&el);
        if info.is_password {
            return Err(ScreenError::Blocked(
                "Password fields cannot be operated.".to_string(),
            ));
        }
        if info.native_window_handle != 0 {
            let handle = native::hwnd(info.native_window_handle);
            if native::in_scope(window, handle) {
                return Ok(());
            }
            return Err(ScreenError::Blocked(
                "The element points to a window outside the allowed scope.".to_string(),
            ));
        }
        current = unsafe {
            state
                .automation
                .RawViewWalker()
                .and_then(|w| w.GetParentElement(&el))
                .ok()
        };
    }
    Err(ScreenError::Blocked(
        "The element no longer belongs to the allowed window. Read the UI again."
            .to_string(),
    ))
}

/// Resolve an `element_id` from the current inspection (`.NET`: `Element`).
fn resolve_element(
    state: &mut UiState,
    element_id: &str,
    interactive: bool,
) -> ScreenResult<IUIAutomationElement> {
    let fresh = state
        .inspected_at
        .map(|t| t.elapsed() < Duration::from_secs(180))
        .unwrap_or(false);
    let element = if fresh {
        state.elements.get(element_id).cloned()
    } else {
        None
    };
    let Some(element) = element else {
        return Err(ScreenError::StaleElement);
    };
    check_element(state, &element, interactive)?;
    if interactive {
        if let Some(identity) = state.identities.get(element_id) {
            let info = element_info(&element);
            if info.name != identity.name
                || info.automation_id != identity.automation_id
                || info.role != identity.role
            {
                return Err(ScreenError::ElementChanged);
            }
        }
    }
    Ok(element)
}

/// The currently focused element, in scope (`.NET`: `Focused`).
fn focused_element(state: &UiState) -> ScreenResult<IUIAutomationElement> {
    let target = lock_target(state).ok_or_else(|| {
        ScreenError::Unavailable("Screen control was stopped.".to_string())
    })?;
    let window = native::hwnd(target.handle);
    if !native::in_scope(window, native::foreground_window()) {
        return Err(ScreenError::Blocked(
            "The window receiving keys is outside the allowed scope.".to_string(),
        ));
    }
    let focused = unsafe { state.automation.GetFocusedElement() }
        .map_err(|e| ScreenError::Uia(format!("Could not determine which input has focus: {e}")))?;
    check_element(state, &focused, true)?;
    Ok(focused)
}

/// Convert screenshot image coordinates to a physical point
/// (`.NET`: `Point` local function in `Execute`).
fn snapshot_point(state: &UiState, args: &ScreenArgs, which: &str) -> ScreenResult<(i32, i32)> {
    // `which` is "from" (x/y) or "to" (to_x/to_y).
    let (x, y) = if which == "to" {
        (args.to_x, args.to_y)
    } else {
        (args.x, args.y)
    };
    let (x, y) = (x.unwrap_or(-1), y.unwrap_or(-1));
    let map = state.image_map.as_ref().ok_or_else(|| {
        ScreenError::InvalidInput(
            "The screenshot is stale or an action happened after it was taken. Take a new screenshot and use the new snapshot_id."
                .to_string(),
        )
    })?;
    let snapshot_ok = args.snapshot_id.as_deref() == Some(state.snapshot.as_str())
        && state
            .captured_at
            .map(|t| t.elapsed() < Duration::from_secs(120))
            .unwrap_or(false);
    if !snapshot_ok {
        return Err(ScreenError::InvalidInput(
            "The screenshot is stale or an action happened after it was taken. Take a new screenshot and use the new snapshot_id."
                .to_string(),
        ));
    }
    let target = lock_target(state).ok_or_else(|| {
        ScreenError::Unavailable("Screen control was stopped.".to_string())
    })?;
    let window = native::hwnd(target.handle);
    let bounds_ok = native::in_scope(window, state.capture_window)
        && native::window_rect(state.capture_window).map(|(l, t, r, b)| {
            (l, t, r - l, b - t) == (map.left, map.top, map.width, map.height)
        }) == Some(true);
    if !bounds_ok {
        return Err(ScreenError::InvalidInput(
            "The screenshotted window moved, resized, or closed. Take a new screenshot."
                .to_string(),
        ));
    }
    map.to_screen(x, y)
}

/// The element at a physical point must belong to the target
/// (`.NET`: `CheckPoint`).
fn check_point(state: &UiState, point: (i32, i32)) -> ScreenResult<()> {
    let element = unsafe {
        state
            .automation
            .ElementFromPoint(POINT { x: point.0, y: point.1 })
    }
    .map_err(|e| ScreenError::Uia(format!("Could not read the element at the point: {e}")))?;
    check_element(state, &element, true)
}

const ACTION_DISPATCHED: &str =
    "action_dispatched=true; inspect/capture and assert the resulting UI before reporting a successful test";

/// Dispatch one action on the worker thread (`.NET`: `Execute`).
pub fn execute_action(
    state: &mut UiState,
    action: ScreenAction,
    args: ScreenArgs,
) -> ScreenResult<ScreenOutcome> {
    validate(state)?;
    let target = lock_target(state).ok_or_else(|| {
        ScreenError::Unavailable("Screen control was stopped.".to_string())
    })?;
    let window = native::hwnd(target.handle);
    let mutation = policy::mutates(&action);
    let old_snapshot = state.snapshot.clone();
    let result = dispatch(state, action, &args, &target, window);
    // A failed/partial mutation must also invalidate the old image.
    if mutation && state.snapshot == old_snapshot {
        state.snapshot.clear();
        state.image_map = None;
    }
    match result {
        // Inspect / screenshot / assert return their own text immediately
        // (.NET returns inside the switch); other actions run the final
        // validation and report `action_dispatched`.
        Ok(outcome) if matches!(action, ScreenAction::Inspect | ScreenAction::Screenshot | ScreenAction::Assert) => {
            Ok(outcome)
        }
        Ok(_) => {
            validate(state)?;
            Ok(ScreenOutcome::text(ACTION_DISPATCHED))
        }
        Err(e) => Err(e),
    }
}

fn dispatch(
    state: &mut UiState,
    action: ScreenAction,
    args: &ScreenArgs,
    target: &DesktopWindow,
    window: HWND,
) -> ScreenResult<ScreenOutcome> {
    let element_id = args.element_id.clone().unwrap_or_default();
    match action {
        ScreenAction::Inspect => inspect(state, args, target),
        ScreenAction::Screenshot => super::shot::capture(state, target, false),
        ScreenAction::ListWindows => unreachable!("handled by DesktopComputer::execute"),
        ScreenAction::Focus => {
            native::focus_window(native::active_window(window), &mut || validate(state))?;
            if !element_id.is_empty() {
                let el = resolve_element(state, &element_id, true)?;
                unsafe { el.SetFocus() }
                    .map_err(|e| ScreenError::Uia(format!("Could not focus the element: {e}")))?;
            }
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Invoke => {
            let el = resolve_element(state, &element_id, true)?;
            let pattern = unsafe {
                el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            }
            .map_err(|_| {
                ScreenError::InvalidInput(
                    "The element does not support direct clicking. Read the child region or use a new image to click by coordinates."
                        .to_string(),
                )
            })?;
            validate(state)?;
            unsafe { pattern.Invoke() }
                .map_err(|e| ScreenError::Uia(format!("Invoke failed: {e}")))?;
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::SetValue => {
            let el = resolve_element(state, &element_id, true)?;
            let pattern = unsafe {
                el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            }
            .map_err(|_| {
                ScreenError::InvalidInput(
                    "The element does not support setting content. Click the input field, then use computer_type."
                        .to_string(),
                )
            })?;
            let read_only = unsafe { pattern.CurrentIsReadOnly() }
                .map(|b| b.as_bool())
                .unwrap_or(false);
            if read_only {
                return Err(ScreenError::InvalidInput(
                    "The element does not support setting content. Click the input field, then use computer_type."
                        .to_string(),
                ));
            }
            let text = args.text.clone().unwrap_or_default();
            if text.len() > 16000 {
                return Err(ScreenError::InvalidInput("Value is too long.".to_string()));
            }
            validate(state)?;
            let value = BSTR::from(&text);
            unsafe { pattern.SetValue(&value) }
                .map_err(|e| ScreenError::Uia(format!("SetValue failed: {e}")))?;
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Type => {
            native::focus_window(native::active_window(window), &mut || validate(state))?;
            let input = if !element_id.is_empty() {
                resolve_element(state, &element_id, true)?
            } else {
                focused_element(state)?
            };
            if !element_id.is_empty() {
                unsafe { input.SetFocus() }
                    .map_err(|e| ScreenError::Uia(format!("Could not focus the element: {e}")))?;
            }
            validate(state)?;
            if !element_info(&input).has_keyboard_focus {
                return Err(ScreenError::Blocked(
                    "The requested field does not have focus. Stopped to avoid typing into the wrong place."
                        .to_string(),
                ));
            }
            let text = args.text.clone().unwrap_or_default();
            let input_for_guard = input.clone();
            let mut guard = || {
                validate(state)?;
                let current = focused_element(state)?;
                let same = unsafe {
                    state
                        .automation
                        .CompareElements(&current, &input_for_guard)
                }
                .map(|b| b.as_bool())
                .unwrap_or(false);
                if !same {
                    return Err(ScreenError::Blocked(
                        "The field receiving keys changed. Stopped typing to avoid mistakes."
                            .to_string(),
                    ));
                }
                Ok(())
            };
            native::type_text(window, &text, &mut guard)?;
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Key => {
            native::focus_window(native::active_window(window), &mut || validate(state))?;
            let _ = focused_element(state)?;
            let keys = policy::keys_for_chord(args.key.as_deref().unwrap_or(""))?;
            native::press_keys(window, &keys, &mut || validate(state))?;
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Toggle => {
            let el = resolve_element(state, &element_id, true)?;
            let pattern = unsafe {
                el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            }
            .map_err(|_| ScreenError::InvalidInput("The element cannot be toggled.".to_string()))?;
            let desired = match args.state.as_deref().unwrap_or("").to_lowercase().as_str() {
                "on" => ToggleState_On,
                "off" => ToggleState_Off,
                _ => {
                    return Err(ScreenError::InvalidInput(
                        "state must be on or off".to_string(),
                    ));
                }
            };
            for _ in 0..3 {
                let current = unsafe { pattern.CurrentToggleState() }
                    .map_err(|e| ScreenError::Uia(format!("Could not read toggle state: {e}")))?;
                if current == desired {
                    break;
                }
                validate(state)?;
                unsafe { pattern.Toggle() }
                    .map_err(|e| ScreenError::Uia(format!("Toggle failed: {e}")))?;
            }
            let current = unsafe { pattern.CurrentToggleState() }
                .map_err(|e| ScreenError::Uia(format!("Could not read toggle state: {e}")))?;
            if current != desired {
                return Err(ScreenError::InvalidInput(
                    "The requested toggle state was not reached.".to_string(),
                ));
            }
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Select => {
            let el = resolve_element(state, &element_id, true)?;
            let pattern = unsafe {
                el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
            }
            .map_err(|_| {
                ScreenError::InvalidInput("The element is not a selectable item.".to_string())
            })?;
            validate(state)?;
            unsafe { pattern.Select() }
                .map_err(|e| ScreenError::Uia(format!("Select failed: {e}")))?;
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Expand => {
            let el = resolve_element(state, &element_id, true)?;
            let pattern = unsafe {
                el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
            }
            .map_err(|_| {
                ScreenError::InvalidInput("The element cannot expand/collapse.".to_string())
            })?;
            validate(state)?;
            if args.expanded == Some(false) {
                unsafe { pattern.Collapse() }
                    .map_err(|e| ScreenError::Uia(format!("Collapse failed: {e}")))?;
            } else {
                unsafe { pattern.Expand() }
                    .map_err(|e| ScreenError::Uia(format!("Expand failed: {e}")))?;
            }
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Scroll => {
            if !element_id.is_empty() {
                let el = resolve_element(state, &element_id, true)?;
                let pattern = unsafe {
                    el.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
                }
                .map_err(|_| {
                    ScreenError::InvalidInput(
                        "The element does not support ScrollPattern. Use a new image and the coordinates of the area to scroll."
                            .to_string(),
                    )
                })?;
                validate(state)?;
                let direction = args.direction.clone().unwrap_or_default();
                let horizontal = match direction.as_str() {
                    "left" => ScrollAmount_LargeDecrement,
                    "right" => ScrollAmount_LargeIncrement,
                    _ => ScrollAmount_NoAmount,
                };
                let vertical = match direction.as_str() {
                    "down" => ScrollAmount_LargeIncrement,
                    "up" => ScrollAmount_LargeDecrement,
                    _ => ScrollAmount_NoAmount,
                };
                unsafe { pattern.Scroll(horizontal, vertical) }
                    .map_err(|e| ScreenError::Uia(format!("Scroll failed: {e}")))?;
            } else {
                let point = snapshot_point(state, args, "from")?;
                native::focus_window(native::active_window(window), &mut || validate(state))?;
                check_point(state, point)?;
                native::scroll(
                    window,
                    point.0,
                    point.1,
                    args.direction.as_deref().unwrap_or(""),
                    &mut || validate(state),
                )?;
            }
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Click => {
            let point = snapshot_point(state, args, "from")?;
            native::focus_window(native::active_window(window), &mut || validate(state))?;
            check_point(state, point)?;
            native::click(
                window,
                point.0,
                point.1,
                args.double_click == Some(true),
                args.button.as_deref() == Some("right"),
                &mut || validate(state),
            )?;
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Drag => {
            let from = snapshot_point(state, args, "from")?;
            let to = snapshot_point(state, args, "to")?;
            native::focus_window(native::active_window(window), &mut || validate(state))?;
            check_point(state, from)?;
            check_point(state, to)?;
            native::drag(window, from, to, &mut || validate(state))?;
            Ok(ScreenOutcome::text(""))
        }
        ScreenAction::Assert => assert_element(state, args),
    }
}

/// Walk the UIA tree of the target window (`.NET`: `computer_inspect`).
fn inspect(
    state: &mut UiState,
    args: &ScreenArgs,
    target: &DesktopWindow,
) -> ScreenResult<ScreenOutcome> {
    let window = native::hwnd(target.handle);
    let active = native::active_window(window);
    let (left, top, _right, _bottom) = native::window_rect(active).ok_or_else(|| {
        ScreenError::InvalidInput("Could not read the window position.".to_string())
    })?;
    let element_id = args.element_id.clone().unwrap_or_default();
    let root = if !element_id.is_empty() {
        resolve_element(state, &element_id, false)?
    } else {
        unsafe { state.automation.ElementFromHandle(active) }
            .map_err(|e| ScreenError::Uia(format!("Could not read the window: {e}")))?
    };
    state.elements.clear();
    state.identities.clear();
    let inspection = new_short_id();
    let query = args.query.clone().unwrap_or_default();
    let offset = args.offset.unwrap_or(0);
    let limit_n = args.limit.unwrap_or(80);
    let max_depth = args.max_depth.unwrap_or(12);
    let mut pending: VecDeque<(IUIAutomationElement, i32)> = VecDeque::new();
    pending.push_back((root, 0));
    let mut nodes: Vec<UiNode> = Vec::new();
    let timer = Instant::now();
    let mut scanned = 0i32;
    let mut matched = 0i32;
    let mut characters = 0usize;
    let mut more = false;
    let mut depth_limited = false;
    let mut child_limited = false;
    let self_pid = std::process::id() as i32;
    while !pending.is_empty() && scanned < 1500 && timer.elapsed().as_secs_f64() < 8.0 {
        scanned += 1;
        validate(state)?;
        let Some((element, depth)) = pending.pop_front() else {
            break;
        };
        let info = element_info(&element);
        if info.is_password || info.process_id == self_pid {
            continue;
        }
        if query.is_empty()
            || format!("{} {} {}", info.name, info.automation_id, info.role)
                .to_lowercase()
                .contains(&query.to_lowercase())
        {
            if matched >= offset {
                if nodes.len() as i32 >= limit_n {
                    more = true;
                    break;
                }
                let id = format!("e{}_{}", inspection, nodes.len());
                let (bx, by, br, bb) = info.bounds;
                let (bx, by, bw, bh) = if br <= bx || bb <= by {
                    (0.0, 0.0, 0.0, 0.0)
                } else {
                    (
                        (bx - left) as f64,
                        (by - top) as f64,
                        (br - bx) as f64,
                        (bb - by) as f64,
                    )
                };
                let node = UiNode {
                    element_id: id.clone(),
                    depth,
                    name: limit(&info.name),
                    automation_id: limit(&info.automation_id),
                    role: info.role.clone(),
                    enabled: info.enabled,
                    offscreen: info.offscreen,
                    value: limit(&element_value(&element)),
                    bounds: UiBounds {
                        x: bx,
                        y: by,
                        width: bw,
                        height: bh,
                    },
                    patterns: supported_patterns(&element),
                };
                let cost = serde_json::to_string(&node).map(|s| s.len()).unwrap_or(0);
                if characters + cost > 40000 && !nodes.is_empty() {
                    more = true;
                    break;
                }
                characters += cost;
                nodes.push(node);
                state.elements.insert(id.clone(), element.clone());
                state.identities.insert(
                    id,
                    ElementIdentity {
                        name: info.name,
                        automation_id: info.automation_id,
                        role: info.role,
                    },
                );
                matched += 1;
            } else {
                matched += 1;
            }
        }
        if depth >= max_depth {
            depth_limited = true;
            continue;
        }
        walk_children(state, &element, depth, &mut pending, &mut child_limited);
    }
    state.inspected_at = Some(Instant::now());
    let truncated = more || !pending.is_empty() || depth_limited || child_limited;
    let page_count = nodes.len() as i32;
    let result = InspectResult {
        window: target.title.clone(),
        active_window: active.0 as i64,
        inspection_id: inspection,
        coordinate_space: "window physical pixels; NOT scaled screenshot pixels. Prefer element_id. Read image_scale from ui_image before using physical bounds for image coordinates."
            .to_string(),
        elements: nodes,
        scanned,
        next_offset: if (more || !pending.is_empty()) && page_count > 0 {
            Some(offset + page_count)
        } else {
            None
        },
        truncated,
        depth_limit_reached: depth_limited,
        scan_limit_reached: scanned >= 1500 || child_limited,
        hint: "query searches name/automation_id/role; offset/limit paginate matches. Inspect a container element_id to narrow the tree. New inspection replaces old ids."
            .to_string(),
    };
    let text = serde_json::to_string(&result)
        .map_err(|e| ScreenError::Uia(format!("Could not serialize inspection: {e}")))?;
    Ok(ScreenOutcome::text(text))
}

/// Enqueue a node's ControlView children (max 500, queue max 1500).
fn walk_children(
    state: &UiState,
    element: &IUIAutomationElement,
    depth: i32,
    pending: &mut VecDeque<(IUIAutomationElement, i32)>,
    child_limited: &mut bool,
) {
    let Ok(walker) = (unsafe { state.automation.ControlViewWalker() }) else {
        return;
    };
    let mut child = unsafe { walker.GetFirstChildElement(element) }.ok();
    let mut count = 0;
    while let Some(c) = child {
        if count >= 500 || pending.len() >= 1500 {
            // Siblings remain beyond the cap.
            *child_limited = true;
            break;
        }
        count += 1;
        let next = unsafe { walker.GetNextSiblingElement(&c) }.ok();
        pending.push_back((c, depth + 1));
        child = next;
    }
}

/// Assert an element property, polling up to `wait_ms` (`.NET`:
/// `computer_assert`).
fn assert_element(state: &mut UiState, args: &ScreenArgs) -> ScreenResult<ScreenOutcome> {
    let element_id = args.element_id.clone().unwrap_or_default();
    let check = resolve_element(state, &element_id, false)?;
    let property = args.property.clone().unwrap_or_default();
    let observe = |_state: &UiState| -> ScreenResult<String> {
        match property.as_str() {
            "name" => Ok(element_info(&check).name),
            "value" => {
                let pattern = unsafe {
                    check.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                }
                .map_err(|_| {
                    ScreenError::InvalidInput(
                        "Supported assertions: name, value, toggle, enabled, offscreen, selected"
                            .to_string(),
                    )
                })?;
                Ok(unsafe { pattern.CurrentValue() }
                    .map(bstr_to_string)
                    .unwrap_or_default())
            }
            "toggle" => {
                let pattern = unsafe {
                    check.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                }
                .map_err(|_| {
                    ScreenError::InvalidInput(
                        "Supported assertions: name, value, toggle, enabled, offscreen, selected"
                            .to_string(),
                    )
                })?;
                let s = unsafe { pattern.CurrentToggleState() }
                    .map_err(|e| ScreenError::Uia(format!("Could not read toggle state: {e}")))?;
                Ok(if s == ToggleState_On {
                    "On".to_string()
                } else if s == ToggleState_Off {
                    "Off".to_string()
                } else {
                    "Indeterminate".to_string()
                })
            }
            "enabled" => Ok(if element_info(&check).enabled {
                "True".to_string()
            } else {
                "False".to_string()
            }),
            "offscreen" => Ok(if element_info(&check).offscreen {
                "True".to_string()
            } else {
                "False".to_string()
            }),
            "selected" => {
                let pattern = unsafe {
                    check.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                }
                .map_err(|_| {
                    ScreenError::InvalidInput(
                        "Supported assertions: name, value, toggle, enabled, offscreen, selected"
                            .to_string(),
                    )
                })?;
                Ok(if unsafe { pattern.CurrentIsSelected() }
                    .map(|b| b.as_bool())
                    .unwrap_or(false)
                {
                    "True".to_string()
                } else {
                    "False".to_string()
                })
            }
            _ => Err(ScreenError::InvalidInput(
                "Supported assertions: name, value, toggle, enabled, offscreen, selected"
                    .to_string(),
            )),
        }
    };
    let expected = args.expected.clone().unwrap_or_default();
    let case_insensitive = matches!(property.as_str(), "enabled" | "offscreen" | "selected" | "toggle");
    let wait_ms = args.wait_ms.unwrap_or(1000).clamp(0, 5000);
    let until = Instant::now() + Duration::from_millis(wait_ms as u64);
    let (observed, passed) = loop {
        validate(state)?;
        let observed = observe(state)?;
        let eq = if case_insensitive {
            observed.eq_ignore_ascii_case(&expected)
        } else {
            observed == expected
        };
        let contains = if case_insensitive {
            observed.to_lowercase().contains(&expected.to_lowercase())
        } else {
            observed.contains(&expected)
        };
        let passed = if args.match_.as_deref() == Some("contains") {
            contains
        } else {
            eq
        };
        if passed || Instant::now() >= until {
            break (observed, passed);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let observed_json = serde_json::to_string(&limit(&observed)).unwrap_or_default();
    Ok(ScreenOutcome::text(format!(
        "{}; observed={}",
        if passed {
            "assertion_passed=true"
        } else {
            "error: assertion_failed"
        },
        observed_json
    )))
}
