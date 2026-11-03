//! Window screenshots — port of `DesktopComputer.Capture` (.NET).
//!
//! The window is copied from the screen with GDI `BitBlt`, password fields
//! found through UIAutomation are painted over in dim gray (like .NET),
//! and the result is JPEG-encoded (max width 1600 px, like .NET) and
//! returned as a `data:image/jpeg;base64,…` URL together with the JSON
//! document .NET emits (`snapshot_id`, `image_scale`, …).

use super::native;
use super::uia;
use super::UiState;
use crate::actions::{ImageScale, ScreenOutcome, ScreenshotResult};
use crate::error::{ScreenError, ScreenResult};
use crate::geometry::{DesktopWindow, ScreenshotMap};
use std::io::Cursor;
use std::time::Instant;
use windows::Win32::UI::Accessibility::TreeScope_Descendants;

/// Capture the target window (`.NET`: `Capture`).
///
/// With `preview == true` the window is not focused and no snapshot is
/// stored (live view only); otherwise the capture becomes the current
/// snapshot for coordinate actions.
pub fn capture(
    state: &mut UiState,
    target: &DesktopWindow,
    preview: bool,
) -> ScreenResult<ScreenOutcome> {
    let root = native::hwnd(target.handle);
    let window = native::active_window(root);
    if preview {
        uia::validate(state)?;
        if native::foreground_window() != window {
            return Err(ScreenError::Blocked(
                "The test window is no longer in front; live viewing is paused.".to_string(),
            ));
        }
    } else {
        native::focus_window(window, &mut || uia::validate(state))?;
    }
    let Some((left, top, right, bottom)) = native::window_rect(window) else {
        return Err(ScreenError::InvalidInput(
            "Cannot capture this window.".to_string(),
        ));
    };
    let (width, height) = (right - left, bottom - top);
    if width < 1 || height < 1 {
        return Err(ScreenError::InvalidInput(
            "Cannot capture this window.".to_string(),
        ));
    }
    if !native::on_virtual_screen(left, top, width, height) {
        return Err(ScreenError::InvalidInput(
            "Move the entire target window onto the screen before capturing.".to_string(),
        ));
    }
    if (width as i64) * (height as i64) > 32_000_000 {
        return Err(ScreenError::InvalidInput(
            "The window is too large to capture. Resize it first.".to_string(),
        ));
    }
    if !native::is_uncovered(root, left, top, width, height) {
        return Err(ScreenError::Blocked(
            "The target window is covered. Bring the window to the front to avoid capturing another app's content."
                .to_string(),
        ));
    }
    let (mut bgra, w, h) = native::capture_bgra(left, top, width, height)?;
    mask_password_fields(state, window, left, top, width, height, &mut bgra);
    uia::validate(state)?;
    if native::foreground_window() != window {
        return Err(ScreenError::Blocked(
            "Foreground window changed during capture. The image was discarded.".to_string(),
        ));
    }
    // Scale down to at most 1600 px wide, like .NET.
    let scale = (1600f64 / width as f64).min(1.0);
    let scaled_w = ((width as f64 * scale) as i32).max(1);
    let scaled_h = ((height as f64 * scale) as i32).max(1);
    let rgb = bgra_to_rgb(&bgra);
    let image = image::RgbImage::from_raw(w as u32, h as u32, rgb)
        .ok_or_else(|| ScreenError::Uia("Could not build the screenshot image.".to_string()))?;
    let image = if scaled_w != width || scaled_h != height {
        image::imageops::resize(
            &image,
            scaled_w as u32,
            scaled_h as u32,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        image
    };
    let mut jpeg = Cursor::new(Vec::new());
    {
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85);
        encoder
            .encode(
                image.as_raw(),
                scaled_w as u32,
                scaled_h as u32,
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| ScreenError::Uia(format!("Could not encode the screenshot: {e}")))?;
    }
    let jpeg = jpeg.into_inner();
    uia::validate(state)?;
    if native::window_rect(window) != Some((left, top, right, bottom)) {
        return Err(ScreenError::Blocked(
            "The window changed while capturing; the image was discarded, please take a new screenshot."
                .to_string(),
        ));
    }
    let data_url = format!(
        "data:image/jpeg;base64,{}",
        {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&jpeg)
        }
    );
    if preview {
        return Ok(ScreenOutcome {
            text: "Live view on this machine.".to_string(),
            image_data_url: Some(data_url),
        });
    }
    use sha2::Digest;
    let fingerprint: String = sha2::Sha256::digest(&jpeg)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    state.captured_at = Some(Instant::now());
    state.snapshot = uia::new_short_id();
    state.image_map = Some(ScreenshotMap {
        left,
        top,
        width,
        height,
        image_width: scaled_w,
        image_height: scaled_h,
    });
    state.capture_window = window;
    let result = ScreenshotResult {
        snapshot_id: state.snapshot.clone(),
        active_window: window.0 as i64,
        width: scaled_w,
        height: scaled_h,
        window_width: width,
        window_height: height,
        image_fingerprint: fingerprint,
        image_scale: ImageScale {
            x: scaled_w as f64 / width as f64,
            y: scaled_h as f64 / height as f64,
        },
        coordinate_space: "screenshot image pixels; multiply UIA physical bounds by image_scale for image coordinates"
            .to_string(),
        window: target.title.clone(),
    };
    let text = serde_json::to_string(&result)
        .map_err(|e| ScreenError::Uia(format!("Could not serialize screenshot: {e}")))?;
    Ok(ScreenOutcome {
        text,
        image_data_url: Some(data_url),
    })
}

/// Paint dim-gray rectangles over password fields (`.NET`: `Capture`
/// masks `IsPassword` elements with `Brushes.DimGray`).
fn mask_password_fields(
    state: &UiState,
    window: windows::Win32::Foundation::HWND,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    bgra: &mut [u8],
) {
    let Ok(root) = (unsafe { state.automation.ElementFromHandle(window) }) else {
        return;
    };
    let Ok(condition) = (unsafe { state.automation.CreateTrueCondition() }) else {
        return;
    };
    let Ok(all) = (unsafe { root.FindAll(TreeScope_Descendants, &condition) }) else {
        return;
    };
    let Ok(count) = (unsafe { all.Length() }) else {
        return;
    };
    // DimGray (#696969).
    const GRAY: (u8, u8, u8) = (105, 105, 105);
    for i in 0..count {
        let Ok(el) = (unsafe { all.GetElement(i) }) else {
            continue;
        };
        let is_password = unsafe { el.CurrentIsPassword() }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        if !is_password {
            continue;
        }
        let Ok(rect) = (unsafe { el.CurrentBoundingRectangle() }) else {
            continue;
        };
        let (x0, y0) = (rect.left - left, rect.top - top);
        let (x1, y1) = (rect.right - left, rect.bottom - top);
        let x0 = x0.clamp(0, width);
        let y0 = y0.clamp(0, height);
        let x1 = x1.clamp(0, width);
        let y1 = y1.clamp(0, height);
        for y in y0..y1 {
            for x in x0..x1 {
                let idx = (y as usize * width as usize + x as usize) * 4;
                if idx + 2 < bgra.len() {
                    // BGRA order in the DIB buffer.
                    bgra[idx] = GRAY.2;
                    bgra[idx + 1] = GRAY.1;
                    bgra[idx + 2] = GRAY.0;
                }
            }
        }
    }
}

/// Convert a 32-bit BGRA top-down buffer to packed RGB.
fn bgra_to_rgb(bgra: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
    for px in bgra.chunks_exact(4) {
        rgb.push(px[2]);
        rgb.push(px[1]);
        rgb.push(px[0]);
    }
    rgb
}
