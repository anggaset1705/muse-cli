//! Pure geometry + window selection — no OS calls, fully testable.
//!
//! Port of `DesktopWindow`, `WindowAutoPicker` and `ScreenshotMap` from
//! `DesktopComputer.cs` / `ComputerGeometry.cs` (.NET).

use serde::{Deserialize, Serialize};
use std::fmt;

/// A top-level window, as enumerated by the OS.
///
/// `process_started` is .NET `DateTime.Ticks` (100 ns units since
/// 0001-01-01 UTC) so an authorized target can be re-validated against a
/// process restart. `0` means unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopWindow {
    /// Window handle as an integer (`HWND` value).
    pub handle: i64,
    /// Owning process id.
    pub process_id: i32,
    /// Window title text.
    pub title: String,
    /// Process start time in .NET ticks (`0` = unknown).
    #[serde(default)]
    pub process_started: i64,
    /// Process name (executable file stem), e.g. `"notepad"`.
    #[serde(default)]
    pub application_name: String,
}

impl fmt::Display for DesktopWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.application_name.is_empty() {
            write!(f, "{}", self.title)
        } else {
            write!(f, "{} — {}", self.title, self.application_name)
        }
    }
}

/// Pure, testable auto-selection for screen control.
///
/// Input must be in Z-order (topmost first), exactly as `EnumWindows`
/// returns it. Port of `WindowAutoPicker.Pick` (`DesktopComputer.cs`).
pub struct WindowAutoPicker;

impl WindowAutoPicker {
    const SHELL_HOSTS: &'static [&'static str] = &[
        "SearchHost",
        "StartMenuExperienceHost",
        "ShellExperienceHost",
        "TextInputHost",
        "LockApp",
        "WindowsInputExperience",
        "InputApp",
    ];
    const SHELL_TITLES: &'static [&'static str] = &["Windows Input Experience"];

    /// Pick the window to control: hint match, then the foreground window,
    /// then the topmost window in Z-order. `None` when nothing safe can be
    /// picked.
    pub fn pick(
        windows_in_z_order: &[DesktopWindow],
        foreground_handle: i64,
        self_process_id: i32,
        preferred_hint: Option<&str>,
    ) -> Option<DesktopWindow> {
        let candidates: Vec<&DesktopWindow> = windows_in_z_order
            .iter()
            .filter(|w| {
                w.process_id != self_process_id
                    && !w.title.trim().is_empty()
                    && !Self::SHELL_TITLES
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case(w.title.trim()))
                    && !Self::SHELL_HOSTS
                        .iter()
                        .any(|h| h.eq_ignore_ascii_case(&w.application_name))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        if let Some(hint) = preferred_hint {
            let hint = hint.trim();
            if !hint.is_empty() {
                let hinted = candidates.iter().find(|w| {
                    w.title.to_lowercase().contains(&hint.to_lowercase())
                        || w.application_name
                            .to_lowercase()
                            .contains(&hint.to_lowercase())
                });
                if let Some(h) = hinted {
                    return Some((*h).clone());
                }
            }
        }
        if foreground_handle != 0 {
            if let Some(fg) = candidates.iter().find(|w| w.handle == foreground_handle) {
                return Some((*fg).clone());
            }
        }
        candidates.first().map(|w| (*w).clone())
    }
}

/// Maps screenshot image pixels back to physical screen coordinates.
///
/// Port of `ScreenshotMap` (`ComputerGeometry.cs`): the screenshot is a
/// scaled-down image of the window rectangle, so an image pixel `(x, y)`
/// maps to the centre of the corresponding physical pixel block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotMap {
    /// Left edge of the captured window rectangle (physical pixels).
    pub left: i32,
    /// Top edge of the captured window rectangle (physical pixels).
    pub top: i32,
    /// Width of the captured window rectangle (physical pixels).
    pub width: i32,
    /// Height of the captured window rectangle (physical pixels).
    pub height: i32,
    /// Width of the screenshot image in pixels.
    pub image_width: i32,
    /// Height of the screenshot image in pixels.
    pub image_height: i32,
}

impl ScreenshotMap {
    /// Convert an image pixel to a physical screen point.
    ///
    /// Mirrors the .NET error: coordinates outside the screenshot are
    /// rejected instead of silently clamped.
    pub fn to_screen(&self, x: i32, y: i32) -> Result<(i32, i32), crate::ScreenError> {
        if self.width <= 0
            || self.height <= 0
            || self.image_width <= 0
            || self.image_height <= 0
            || x < 0
            || y < 0
            || x >= self.image_width
            || y >= self.image_height
        {
            return Err(crate::ScreenError::InvalidInput(
                "The coordinates are outside the screenshot. Capture the window again."
                    .to_string(),
            ));
        }
        let sx = self.left
            + (((x as i64 + 1) * 2 - 1) * self.width as i64 / (2 * self.image_width as i64)) as i32;
        // The expression above is `(x + 0.5) * width / image_width` in
        // integer arithmetic (`.NET`: `(int)((x + 0.5) * Bounds.Width / ImageWidth)`).
        let sy = self.top
            + (((y as i64 + 1) * 2 - 1) * self.height as i64 / (2 * self.image_height as i64))
                as i32;
        Ok((sx, sy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(handle: i64, pid: i32, title: &str, app: &str) -> DesktopWindow {
        DesktopWindow {
            handle,
            process_id: pid,
            title: title.to_string(),
            process_started: 0,
            application_name: app.to_string(),
        }
    }

    #[test]
    fn picker_skips_self_and_shell_hosts() {
        let windows = vec![
            win(1, 100, "Muse", "Muse"),
            win(2, 200, "Start", "StartMenuExperienceHost"),
            win(3, 300, "Notepad", "notepad"),
        ];
        let picked = WindowAutoPicker::pick(&windows, 0, 100, None).unwrap();
        assert_eq!(picked.handle, 3);
    }

    #[test]
    fn picker_prefers_hint_over_foreground() {
        let windows = vec![
            win(1, 100, "Muse", "Muse"),
            win(2, 200, "Calculator", "calculator"),
            win(3, 300, "Notepad", "notepad"),
        ];
        let picked = WindowAutoPicker::pick(&windows, 2, 100, Some("notepad")).unwrap();
        assert_eq!(picked.handle, 3);
    }

    #[test]
    fn picker_prefers_foreground_then_topmost() {
        let windows = vec![
            win(1, 100, "Muse", "Muse"),
            win(2, 200, "Calculator", "calculator"),
            win(3, 300, "Notepad", "notepad"),
        ];
        assert_eq!(
            WindowAutoPicker::pick(&windows, 3, 100, None)
                .unwrap()
                .handle,
            3
        );
        assert_eq!(
            WindowAutoPicker::pick(&windows, 0, 100, None)
                .unwrap()
                .handle,
            2
        );
    }

    #[test]
    fn picker_returns_none_when_nothing_safe() {
        let windows = vec![
            win(1, 100, "Muse", "Muse"),
            win(2, 200, "", "notepad"),
            win(3, 300, "Windows Input Experience", "TextInputHost"),
        ];
        assert!(WindowAutoPicker::pick(&windows, 0, 100, None).is_none());
    }

    #[test]
    fn screenshot_map_centre_pixel() {
        // Window 800x600 at (100, 50), image 400x300 (scale 0.5):
        // image pixel (0,0) -> physical (101, 51); (399,299) -> (899, 649).
        let map = ScreenshotMap {
            left: 100,
            top: 50,
            width: 800,
            height: 600,
            image_width: 400,
            image_height: 300,
        };
        assert_eq!(map.to_screen(0, 0).unwrap(), (101, 51));
        assert_eq!(map.to_screen(399, 299).unwrap(), (899, 649));
        assert_eq!(map.to_screen(200, 150).unwrap(), (501, 351));
    }

    #[test]
    fn screenshot_map_rejects_out_of_range() {
        let map = ScreenshotMap {
            left: 0,
            top: 0,
            width: 800,
            height: 600,
            image_width: 400,
            image_height: 300,
        };
        assert!(map.to_screen(400, 0).is_err());
        assert!(map.to_screen(0, -1).is_err());
        assert!(map.to_screen(0, 300).is_err());
    }
}
