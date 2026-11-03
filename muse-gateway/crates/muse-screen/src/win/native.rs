//! Raw Win32 desktop helpers — port of `NativeDesktop.cs` (.NET).
//!
//! All functions here are thin wrappers over `user32.dll` / `gdi32.dll`
//! via the `windows` crate. They run on the MTA worker thread (see
//! `win/mod.rs`), like the .NET `ComputerWorker`.

use crate::error::{ScreenError, ScreenResult};
use crate::geometry::DesktopWindow;
use std::ffi::c_void;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::ProcessStatus::GetModuleFileNameExW;
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, INPUT, INPUT_0,
    INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOD_NOREPEAT, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    VIRTUAL_KEY,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetForegroundWindow, GetLastActivePopup, GetSystemMetrics,
    GetWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SetCursorPos, SetForegroundWindow, ShowWindow, WindowFromPoint,
    GA_ROOT, GW_OWNER, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SW_RESTORE, WNDENUMPROC,
};
use windows::core::BOOL;

/// Window handle from the integer stored in [`DesktopWindow`].
pub fn hwnd(handle: i64) -> HWND {
    HWND(handle as isize as *mut c_void)
}

/// Set per-monitor-V2 DPI awareness on the calling (worker) thread so all
/// coordinates are physical pixels (`.NET`: `PhysicalCoordinates()`).
pub fn set_thread_dpi_awareness() {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Process id of a window, `0` when it cannot be read.
pub fn window_process_id(handle: HWND) -> u32 {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(handle, Some(&mut pid as *mut u32));
        pid
    }
}

/// Process start time in .NET ticks (100 ns since 0001-01-01 UTC), `None`
/// when the process cannot be opened.
pub fn process_start_ticks(pid: u32) -> Option<i64> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut creation = Default::default();
        let mut exit = Default::default();
        let mut kernel = Default::default();
        let mut user = Default::default();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if ok.is_err() {
            return None;
        }
        // FILETIME is 100 ns since 1601-01-01; .NET ticks are 100 ns since
        // 0001-01-01. Offset between the epochs: 504911232000000000 ticks.
        let ft = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        Some(ft as i64 + 504_911_232_000_000_000)
    }
}

/// Executable file stem of a process (`"notepad"`), `None` when unreadable.
pub fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 512];
        let len = GetModuleFileNameExW(Some(handle), None, &mut buf);
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if len == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        Some(stem.to_string())
    }
}

unsafe extern "system" fn enum_windows_proc(handle: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<DesktopWindow>);
    let self_pid = GetCurrentProcessId();
    if !IsWindowVisible(handle).as_bool() {
        return BOOL::from(true);
    }
    let pid = window_process_id(handle);
    if pid == self_pid {
        return BOOL::from(true); // Never automate Muse's own windows.
    }
    let mut buf = vec![0u16; 512];
    let len = GetWindowTextW(handle, &mut buf);
    if len > 0 {
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if let (Some(started), Some(name)) =
            (process_start_ticks(pid), process_name(pid))
        {
            out.push(DesktopWindow {
                handle: handle.0 as i64,
                process_id: pid as i32,
                title,
                process_started: started,
                application_name: name,
            });
        }
    }
    BOOL::from(true)
}

/// Top-level windows in Z-order (topmost first) — `.NET`: `EnumerateWindows`.
pub fn enumerate_windows() -> Vec<DesktopWindow> {
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(
            WNDENUMPROC::Some(enum_windows_proc),
            LPARAM(&mut out as *mut Vec<DesktopWindow> as isize),
        );
    }
    out
}

/// Window rectangle (left, top, right, bottom), physical pixels.
pub fn window_rect(handle: HWND) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(handle, &mut rect as *mut RECT).is_ok() {
            Some((rect.left, rect.top, rect.right, rect.bottom))
        } else {
            None
        }
    }
}

pub fn foreground_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

pub fn is_window(handle: HWND) -> bool {
    unsafe { IsWindow(Some(handle)).as_bool() }
}

/// Whether `candidate` is the root window itself or one of its owned
/// dialogs/menus (`.NET`: `InScope`). The candidate's top-level ancestor
/// must belong to the same process as the root, and the root must not be
/// one of Muse's own windows.
pub fn in_scope(root: HWND, candidate: HWND) -> bool {
    unsafe {
        if !IsWindow(Some(root)).as_bool() || !IsWindow(Some(candidate)).as_bool() {
            return false;
        }
        let pid = window_process_id(root);
        if pid == GetCurrentProcessId() {
            return false;
        }
        let mut current = GetAncestor(candidate, GA_ROOT);
        for _ in 0..16 {
            if current.0.is_null() {
                break;
            }
            if current == root {
                return true;
            }
            if window_process_id(current) != pid {
                return false;
            }
            current = GetWindow(current, GW_OWNER).unwrap_or_default();
        }
        false
    }
}

/// The window that currently receives input for the target: the foreground
/// window when it is in scope, else the root's last active popup, else the
/// root (`.NET`: `ActiveWindow`).
pub fn active_window(root: HWND) -> HWND {
    unsafe {
        let foreground = GetForegroundWindow();
        if in_scope(root, foreground) {
            return foreground;
        }
        let popup = GetLastActivePopup(root);
        if !popup.0.is_null() && IsWindowVisible(popup).as_bool() && in_scope(root, popup) {
            return popup;
        }
        root
    }
}

/// Bring a window to the front (`.NET`: `Focus`).
pub fn focus_window(window: HWND, validate: &mut dyn FnMut() -> ScreenResult<()>) -> ScreenResult<()> {
    validate()?;
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        if GetForegroundWindow() == window {
            return Ok(());
        }
        let _ = SetForegroundWindow(window);
        let start = Instant::now();
        while GetForegroundWindow() != window {
            if start.elapsed() > Duration::from_millis(500) {
                return Err(ScreenError::Blocked(
                    "Windows did not allow bringing the target window to the front. Open that window and try again."
                        .to_string(),
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    validate()
}

/// Reject the action while the user holds a modifier key or mouse button
/// (`.NET`: `CheckModifiers`).
pub fn check_modifiers() -> ScreenResult<()> {
    // Shift, Ctrl, Alt, Win, left/right/middle/X buttons.
    for key in [0x10, 0x11, 0x12, 0x5B, 0x5C, 0x01, 0x02, 0x04, 0x05, 0x06] {
        if unsafe { GetAsyncKeyState(key) } & 0x8000u16 as i16 != 0 {
            return Err(ScreenError::Blocked(
                "Release the modifier keys and mouse buttons before Muse acts.".to_string(),
            ));
        }
    }
    Ok(())
}

/// Send synthetic input, releasing everything if Windows accepts only part
/// of the batch (`.NET`: `Send`).
pub fn send_inputs(inputs: &[INPUT]) -> ScreenResult<()> {
    unsafe {
        let sent = SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        if sent as usize != inputs.len() {
            let mut release: Vec<INPUT> = inputs
                .iter()
                .filter(|i| i.r#type == INPUT_KEYBOARD)
                .map(|i| {
                    let mut ki = i.Anonymous.ki;
                    ki.dwFlags |= KEYEVENTF_KEYUP;
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 { ki },
                    }
                })
                .collect();
            release.push(INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: MOUSEEVENTF_LEFTUP | MOUSEEVENTF_RIGHTUP,
                        ..Default::default()
                    },
                },
            });
            let _ = SendInput(&release, std::mem::size_of::<INPUT>() as i32);
            return Err(ScreenError::Blocked(
                "Windows blocked the action. Muse cannot control a secure window or an app running with higher privileges."
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn mouse_input(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn keyboard_input(vk: u16, scan: u16, flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// The point must belong to the target window and be uncovered
/// (`.NET`: `AtPoint`).
pub fn at_point(
    root: HWND,
    x: i32,
    y: i32,
    validate: &mut dyn FnMut() -> ScreenResult<()>,
) -> ScreenResult<()> {
    validate()?;
    unsafe {
        if !in_scope(root, GetForegroundWindow())
            || !in_scope(root, WindowFromPoint(POINT { x, y }))
        {
            return Err(ScreenError::Blocked(
                "The action point is covered by another window or the window changed. Take a new screenshot before acting."
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Click at a physical point (`.NET`: `Click`).
pub fn click(
    root: HWND,
    x: i32,
    y: i32,
    twice: bool,
    right: bool,
    validate: &mut dyn FnMut() -> ScreenResult<()>,
) -> ScreenResult<()> {
    check_modifiers()?;
    focus_window(active_window(root), &mut *validate)?;
    at_point(root, x, y, &mut *validate)?;
    unsafe {
        if SetCursorPos(x, y).is_err() {
            return Err(ScreenError::Blocked("Could not move the cursor.".to_string()));
        }
    }
    at_point(root, x, y, &mut *validate)?;
    let original = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) };
    let (down, up) = if right {
        (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
    } else {
        (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
    };
    send_inputs(&[mouse_input(down, 0), mouse_input(up, 0)])?;
    if twice {
        at_point(root, x, y, &mut *validate)?;
        let now = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) };
        if now != original {
            return Err(ScreenError::Blocked(
                "The window at the click point changed; the second click was not sent."
                    .to_string(),
            ));
        }
        send_inputs(&[mouse_input(down, 0), mouse_input(up, 0)])?;
    }
    Ok(())
}

/// Type text as Unicode scan codes, in chunks of 32 UTF-16 units
/// (`.NET`: `Type`). The caller validates focus/scope per chunk via
/// `before_chunk`.
pub fn type_text(
    root: HWND,
    text: &str,
    before_chunk: &mut dyn FnMut() -> ScreenResult<()>,
) -> ScreenResult<()> {
    check_modifiers()?;
    focus_window(active_window(root), &mut || Ok(()))?;
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(32) {
        check_modifiers()?;
        before_chunk()?;
        if !in_scope(root, foreground_window()) {
            return Err(ScreenError::Blocked(
                "The window receiving keys changed. Stopped typing.".to_string(),
            ));
        }
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for &unit in chunk {
            inputs.push(keyboard_input(0, unit, KEYEVENTF_UNICODE));
            inputs.push(keyboard_input(
                0,
                unit,
                KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
            ));
        }
        send_inputs(&inputs)?;
    }
    Ok(())
}

/// Press a chord of virtual keys: down in order, up in reverse
/// (`.NET`: `Key`).
pub fn press_keys(root: HWND, keys: &[u16], validate: &mut dyn FnMut() -> ScreenResult<()>) -> ScreenResult<()> {
    check_modifiers()?;
    focus_window(active_window(root), &mut *validate)?;
    validate()?;
    if !in_scope(root, foreground_window()) {
        return Err(ScreenError::Blocked(
            "The window receiving keys changed.".to_string(),
        ));
    }
    let mut inputs: Vec<INPUT> = keys.iter().map(|&k| keyboard_input(k, 0, Default::default())).collect();
    inputs.extend(
        keys.iter()
            .rev()
            .map(|&k| keyboard_input(k, 0, KEYEVENTF_KEYUP)),
    );
    send_inputs(&inputs)?;
    // Let the matching synthetic key-up reach Windows before the next
    // action checks for user-held modifiers.
    let start = Instant::now();
    while !keys
        .iter()
        .all(|&k| unsafe { GetAsyncKeyState(k as i32) } & 0x8000u16 as i16 == 0)
    {
        if start.elapsed() > Duration::from_millis(500) {
            return Err(ScreenError::Blocked(
                "A key is still held down. Release the key and continue.".to_string(),
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

/// Scroll the wheel at a physical point (`.NET`: `Scroll`).
pub fn scroll(
    root: HWND,
    x: i32,
    y: i32,
    direction: &str,
    validate: &mut dyn FnMut() -> ScreenResult<()>,
) -> ScreenResult<()> {
    check_modifiers()?;
    focus_window(active_window(root), &mut *validate)?;
    at_point(root, x, y, &mut *validate)?;
    unsafe {
        if SetCursorPos(x, y).is_err() {
            return Err(ScreenError::Blocked("Could not move the cursor.".to_string()));
        }
    }
    at_point(root, x, y, &mut *validate)?;
    let amount: u32 = if direction == "up" || direction == "right" {
        360
    } else {
        (-360i32) as u32
    };
    let flags = if direction == "left" || direction == "right" {
        MOUSEEVENTF_HWHEEL
    } else {
        MOUSEEVENTF_WHEEL
    };
    send_inputs(&[mouse_input(flags, amount)])
}

/// Drag from one physical point to another in 16 steps (`.NET`: `Drag`).
pub fn drag(
    root: HWND,
    from: (i32, i32),
    to: (i32, i32),
    validate: &mut dyn FnMut() -> ScreenResult<()>,
) -> ScreenResult<()> {
    check_modifiers()?;
    focus_window(active_window(root), &mut *validate)?;
    at_point(root, from.0, from.1, &mut *validate)?;
    at_point(root, to.0, to.1, &mut *validate)?;
    unsafe {
        if SetCursorPos(from.0, from.1).is_err() {
            return Err(ScreenError::Blocked("Could not move the cursor.".to_string()));
        }
    }
    at_point(root, from.0, from.1, &mut *validate)?;
    send_inputs(&[mouse_input(MOUSEEVENTF_LEFTDOWN, 0)])?;
    let result = (|| {
        for step in 1..=16 {
            let x = from.0 + ((to.0 as i64 - from.0 as i64) * step / 16) as i32;
            let y = from.1 + ((to.1 as i64 - from.1 as i64) * step / 16) as i32;
            at_point(root, x, y, &mut *validate)?;
            unsafe {
                if SetCursorPos(x, y).is_err() {
                    return Err(ScreenError::Blocked(
                        "Could not move the cursor.".to_string(),
                    ));
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        at_point(root, to.0, to.1, validate)
    })();
    unsafe {
        let _ = SendInput(
            &[mouse_input(MOUSEEVENTF_LEFTUP, 0)],
            std::mem::size_of::<INPUT>() as i32,
        );
    }
    result
}

/// Whether every sample point of the rectangle belongs to the target window
/// (`.NET`: `IsUncovered` — 6 rows x 8 columns, sampled at cell centres).
pub fn is_uncovered(root: HWND, left: i32, top: i32, width: i32, height: i32) -> bool {
    for row in 0..6 {
        for col in 0..8 {
            // Cell centre: left + (col + 0.5) * width / 8.
            let x = left + ((col * 2 + 1) * width) / 16;
            let y = top + ((row * 2 + 1) * height) / 12;
            if !in_scope(root, unsafe { WindowFromPoint(POINT { x, y }) }) {
                return false;
            }
        }
    }
    true
}

/// Whether a rectangle lies fully on the virtual screen.
pub fn on_virtual_screen(left: i32, top: i32, width: i32, height: i32) -> bool {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        left >= vx && top >= vy && left + width <= vx + vw && top + height <= vy + vh
    }
}

/// Capture a window rectangle from the screen into a BGRA buffer
/// (`.NET`: `Graphics.CopyFromScreen` via `BitBlt`).
///
/// Returns `(bgra_bytes, width, height)` with rows top-down.
pub fn capture_bgra(left: i32, top: i32, width: i32, height: i32) -> ScreenResult<(Vec<u8>, i32, i32)> {
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.0.is_null() {
            return Err(ScreenError::Uia("Could not open the screen device context.".to_string()));
        }
        let result = (|| -> ScreenResult<(Vec<u8>, i32, i32)> {
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            if mem_dc.0.is_null() {
                return Err(ScreenError::Uia("Could not create a memory device context.".to_string()));
            }
            let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
            if bitmap.0.is_null() {
                let _ = DeleteDC(mem_dc);
                return Err(ScreenError::Uia("Could not create a capture bitmap.".to_string()));
            }
            let old = SelectObject(mem_dc, bitmap.into());
            let blt = BitBlt(mem_dc, 0, 0, width, height, Some(screen_dc), left, top, SRCCOPY);
            if blt.is_err() {
                let _ = SelectObject(mem_dc, old);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(mem_dc);
                return Err(ScreenError::Uia("Could not copy the window from the screen.".to_string()));
            }
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // top-down DIB
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0, // BI_RGB
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: Default::default(),
            };
            let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
            let lines = GetDIBits(
                mem_dc,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr() as *mut c_void),
                &mut info,
                DIB_RGB_COLORS,
            );
            let _ = SelectObject(mem_dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(mem_dc);
            if lines != height {
                return Err(ScreenError::Uia("Could not read the captured pixels.".to_string()));
            }
            Ok((pixels, width, height))
        })();
        let _ = windows::Win32::Graphics::Gdi::ReleaseDC(None, screen_dc);
        result
    }
}

/// Register the Esc emergency-stop hotkey on a host window
/// (`.NET`: `RegisterEmergencyStop` — id `0x4D55`, `MOD_NOREPEAT`, `Esc`).
pub fn register_emergency_stop(window_handle: i64) -> bool {
    unsafe {
        RegisterHotKey(
            Some(hwnd(window_handle)),
            crate::computer::EMERGENCY_HOTKEY,
            MOD_NOREPEAT,
            0x1B,
        )
        .is_ok()
    }
}

/// Unregister the Esc emergency-stop hotkey (`.NET`: `UnregisterEmergencyStop`).
pub fn unregister_emergency_stop(window_handle: i64) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd(window_handle)), crate::computer::EMERGENCY_HOTKEY);
    }
}
