//! Windows implementation of the Muse sign-in window.
//!
//! Direct port of `Muse2Api.WinForms.LoginForm` (WinForms) to raw Win32 +
//! WebView2 via the `webview2-com` crate (0.39.1). The WinForms layout is
//! reproduced with plain child windows: a heading (title + description),
//! the WebView2 controller in the middle, and a footer with a status label,
//! a Retry button and an "Other account" button. The look matches
//! `DesktopTheme` (dark palette #141414, Segoe UI, owner-drawn CleanButton
//! footer buttons) and the exe carries a Common Controls v6 manifest
//! (see `muse-gateway/muse-gateway.manifest` + `build.rs`).
//!
//! Threading model (mirrors the WebView2 STA requirement and the
//! `webview2-com` sample): everything runs on one STA UI thread. All shared
//! state lives in a single `RefCell<LoginState>` behind a raw pointer stored
//! in `GWLP_USERDATA`; it is only ever dereferenced on the UI thread and is
//! freed on `WM_NCDESTROY`. The poll timer (`WM_TIMER`, 2 s) drives
//! `refresh()`; the WebView2 async helpers (`wait_for_async_operation`)
//! pump the message loop while waiting, so `refresh()` never holds a
//! `RefCell` borrow across a pump — inputs are snapshotted, the borrow is
//! dropped, the async call runs, then outputs are stored back. Re-entrant
//! `refresh()` calls (timer firing during a pump, navigation completing
//! during a pump) are ignored via the `refreshing` flag, exactly like the
//! `_refreshing` guard in `LoginForm.RefreshStatus`.
//!
//! Re-entrancy rule (hardened 3 Nov 2026, after a real crash): `set_status`
//! and `show_retry` take plain HWNDs and are NEVER called while a state
//! borrow is held — `SetWindowTextW` synchronously sends
//! `WM_CTLCOLORSTATIC` to the parent window proc, and the dark-theme
//! handler there used to `cell.borrow()` (`windows.rs:1056`): called
//! while `refresh_inner` held a `borrow_mut()`, that panicked with
//! "RefCell already mutably borrowed" inside a window proc that cannot
//! unwind, aborting the whole login child right after the cookies were
//! complete (login.log, 3 Nov 2026 09:10:30Z). The `wnd_proc` handlers
//! that read the cell (`WM_SIZE`, `WM_CTLCOLORSTATIC`, `WM_DRAWITEM`)
//! use `try_borrow()` with safe fallbacks for the same reason: they can
//! be dispatched re-entrantly from the nested pump in
//! `wait_for_async_operation` (`GetMessage` + `DispatchMessage` while
//! waiting for a WebView2 async op) or synchronously from
//! `SetWindowTextW`. A failed `try_borrow()` means "state busy — paint
//! or lay out with safe defaults this frame"; the next paint/layout
//! picks up the real values.
//!
//! This file is `#[cfg(windows)]` only and is **not** compiled on Linux.
//! It was written against the `webview2-com` 0.39.1 source in the local
//! cargo registry and the `LoginForm.cs` reference; see `LOGIN-VERIFY.md`
//! for the Windows verification checklist — it has NOT been run on Windows
//! yet.

use std::cell::RefCell;
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::{
    core::{w, BOOL, Interface as _, PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        Graphics::{
            Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE},
            Gdi::{
                CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateSolidBrush,
                DEFAULT_CHARSET, DEFAULT_QUALITY, DeleteObject, DrawTextW,
                DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, FillRect,
                HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, PS_SOLID,
                RoundRect, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
            },
        },
        System::{Com::*, LibraryLoader::GetModuleHandleW},
        UI::{
            Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_HOTLIGHT, ODS_SELECTED},
            HiDpi::{GetDpiForWindow, PROCESS_PER_MONITOR_DPI_AWARE, SetProcessDpiAwareness},
            WindowsAndMessaging::*,
        },
    },
};
use webview2_com::{
    CoTaskMemPWSTR, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, ExecuteScriptCompletedHandler,
    GetCookiesCompletedHandler, NavigationCompletedEventHandler,
    WebMessageReceivedEventHandler, take_pwstr,
    Microsoft::Web::WebView2::Win32::*,
};

use crate::cookies::{filter_login_cookies, has_all_required, CookieEntry};
use crate::gateway::post_cookies_to_gateway;
use crate::login_log::{log_stage, short};
use crate::refresh::{decide_refresh, RefreshDecision};
use crate::session_check::{
    new_nonce, parse_session_message, session_check_script, MUSE_HOST,
    SESSION_CHECK_THROTTLE_SECS,
};
use crate::{LoginError, LoginResult, SavedCookie, MUSE_SITE, POLL_INTERVAL_MS,
            WINDOW_HEIGHT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH, WINDOW_WIDTH};

/// English UI strings (LoginForm.cs reads them from `UiStrings`; these are
/// the English defaults in `UiStrings.cs`).
mod strings {
    pub const TITLE: &str = "Connect your Muse account";
    pub const HEADING: &str = "Sign in to your Muse account";
    pub const HEADING_DESC: &str =
        "Sign in right on the Muse page below. The app will save the connection automatically when you finish.";
    pub const STATUS_OPENING: &str = "Opening the sign-in page…";
    pub const WAITING_SIGN_IN: &str =
        "Waiting for you to sign in. You only need to act on the Muse page.";
    /// Perbaikan 1 (3 Nov 2026): the required cookies are already there
    /// but the page is still on a login redirect (Meta/OAuth), not
    /// muse.ai — the sign-in finished, the Muse page has not loaded back
    /// yet. The old code returned silently here and the window sat
    /// forever on WAITING_SIGN_IN.
    pub const LOGIN_WAITING_REDIRECT: &str = "Signed in — waiting for the Muse page…";
    /// Indonesian wording of LOGIN_WAITING_REDIRECT, recorded for a
    /// future locale-aware pass. The window currently shows the English
    /// strings only (like every other status here); the login child
    /// receives no locale, so this is not displayed yet.
    #[allow(dead_code)]
    pub const LOGIN_WAITING_REDIRECT_ID: &str = "Sudah masuk — menunggu halaman Muse…";
    pub const CHECKING_SESSION: &str = "Checking and restoring your saved Muse session…";
    pub const LOGIN_SUCCESS_SAVING: &str = "Signed in. Saving the connection…";
    pub const LOGIN_CONNECTED_DONE: &str = "Connected. You can start working on your project.";
    pub const LOGIN_SAVE_FAILED: &str =
        "Signed in, but the connection could not be saved. Click Retry; you do not need to sign in again.";
    pub const LOGIN_WINDOW_FAILED: &str =
        "Could not open the sign-in window. Check Edge WebView2 on this computer, then click Retry.";
    pub const LOGIN_OTHER_ACCOUNT_STATUS: &str = "Sign in to another account on the Muse page.";
    pub const SESSION_NOT_CONFIRMED: &str =
        "The saved session has not been confirmed by Muse yet. Sign in on the page if asked; the app does not fetch a new code by itself.";
    pub const RETRY: &str = "Retry";
    pub const OTHER_ACCOUNT: &str = "Another account";
}

/// Layout (LoginForm.cs: rows 75 / 100% / 55, footer padding 18).
const HEADING_HEIGHT: i32 = 75;
const FOOTER_HEIGHT: i32 = 55;
const PADDING: i32 = 18;

/// Dark palette from `DesktopTheme.DarkPalette` (WinForms).
/// `COLORREF` is 0x00BBGGRR.
const COLOR_BG: COLORREF = COLORREF(0x00141414); // RGB(20,20,20) Background
const COLOR_SURFACE: COLORREF = COLORREF(0x00202020); // RGB(32,32,32) Surface
const COLOR_SURFACE_STRONG: COLORREF = COLORREF(0x002C2C2C); // RGB(44,44,44) SurfaceStrong (hover/pressed)
const COLOR_BORDER: COLORREF = COLORREF(0x002E2E2E); // RGB(46,46,46) Border
const COLOR_TEXT: COLORREF = COLORREF(0x00ECECEC); // RGB(236,236,236) Text
const COLOR_MUTED: COLORREF = COLORREF(0x00938E8E); // RGB(142,142,147) Muted
const COLOR_ACCENT: COLORREF = COLORREF(0x00F6823B); // RGB(59,130,246) Accent (focus ring)

const TIMER_ID: usize = 1;
/// Fires once, ~1 s after a successful save, to close the window
/// (see `refresh_inner`).
const TIMER_CLOSE_ID: usize = 2;
const CLOSE_DELAY_MS: u32 = 1000;
const ID_RETRY: isize = 101;
const ID_OTHER_ACCOUNT: isize = 102;
const MSG_SESSION_VERIFIED: u32 = WM_APP + 1;
const MSG_REFRESH: u32 = WM_APP + 2;

/// Everything the window needs. Lives in a `RefCell` behind a raw pointer in
/// `GWLP_USERDATA`; UI-thread only (see module docs).
struct LoginState {
    hwnd: HWND,
    /// `None` until the WebView2 controller is created (retry re-inits).
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    cookie_manager: Option<ICoreWebView2CookieManager>,
    status_hwnd: HWND,
    retry_hwnd: HWND,
    switch_hwnd: HWND,
    title_hwnd: HWND,
    desc_hwnd: HWND,
    gateway_url: String,
    admin_key: String,
    label: String,
    /// Desktop data root (`DesktopDataPaths.Root`); the WebView2 profile
    /// lives in `<data_root>\WebView2`.
    data_root: std::path::PathBuf,
    saved_cookies: Vec<SavedCookie>,
    /// LoginForm.cs `_clearExistingSession`: when true, the saved cookies
    /// are NOT restored and all WebView cookies are deleted before the
    /// first navigation, so the user signs in with a different account
    /// (dialog "Other account"). Gateway accounts are untouched.
    clear_session: bool,
    /// Random per-login nonce (LoginForm.cs `_verificationNonce`).
    nonce: String,
    session_verified: bool,
    /// `None` = check immediately (LoginForm.cs `_nextSessionCheck` starts
    /// at `DateTime.MinValue`).
    next_session_check: Option<Instant>,
    /// Logged once when the required cookies are first seen complete, so
    /// the 2 s poll does not spam the runtime log with one line per poll.
    cookies_seen: bool,
    /// Whether the waiting-redirect status is currently shown. Set on
    /// entry into the redirect branch, cleared when the page is muse.ai
    /// again / cookies go missing / the user switches account — the log
    /// line is written only on entry (false -> true).
    waiting_redirect: bool,
    refreshing: bool,
    loading: bool,
    failed: bool,
    /// Set once the gateway saved the account and the "Connected" status
    /// is showing; the window closes when the close timer fires.
    /// While set, polls and button actions are ignored.
    closing: bool,
    /// Segoe UI 10 (DesktopTheme.Body), sent to the children via WM_SETFONT.
    body_font: HFONT,
    /// Segoe UI 17 bold (LoginForm.cs heading), sent to the title via WM_SETFONT.
    heading_font: HFONT,
    /// Set once the gateway returns the saved account id.
    result: Option<String>,
}

type StateCell = RefCell<LoginState>;

/// Raw pointer to the `StateCell`, stored in `GWLP_USERDATA`.
/// `*mut T` is `Copy`; every use site is on the STA UI thread.
type StatePtr = *mut StateCell;

fn win_err(err: windows::core::Error) -> LoginError {
    LoginError::WebView(err.to_string())
}

fn wv_err(err: webview2_com::Error) -> LoginError {
    LoginError::WebView(format!("{err:?}"))
}

/// Set the footer status text.
///
/// Takes the status label HWND (a `Copy` handle), NOT `&LoginState`:
/// `SetWindowTextW` synchronously sends `WM_CTLCOLORSTATIC` to the
/// parent window proc, whose handler reads the state cell — calling
/// this while a `borrow_mut()` is held panicked on 3 Nov 2026
/// ("RefCell already mutably borrowed", `windows.rs:1056`) inside a
/// window proc that cannot unwind, aborting the login child right
/// after the cookies were complete. Every caller snapshots the HWND
/// first and calls this with no borrow held (see the module docs).
/// The change is logged: the 11:22 crash left no trace after
/// "cookies-complete", so the status trail is the evidence for the
/// next login diagnosis.
fn set_status(status_hwnd: HWND, text: &str) {
    if status_hwnd.is_invalid() {
        return;
    }
    let wide = CoTaskMemPWSTR::from(text);
    unsafe {
        let _ = SetWindowTextW(status_hwnd, *wide.as_ref().as_pcwstr());
    }
    log_stage("status", text);
}

/// Show/hide the Retry button. Same no-borrow rule as `set_status`:
/// takes the button HWND, never `&LoginState`.
fn show_retry(retry_hwnd: HWND, show: bool) {
    if retry_hwnd.is_invalid() {
        return;
    }
    unsafe {
        let _ = ShowWindow(
            retry_hwnd,
            if show { SW_SHOW } else { SW_HIDE },
        );
    }
}

/// Process-lifetime dark background brush (`DesktopTheme.Background`).
/// One GDI object, created once and never deleted (like a class brush);
/// shared by the window class background and `WM_CTLCOLORSTATIC`.
fn theme_brush() -> HBRUSH {
    // HBRUSH wraps a raw pointer (neither Send nor Sync), so the static
    // stores the address as usize instead.
    static BG_BRUSH: OnceLock<usize> = OnceLock::new();
    let addr = *BG_BRUSH.get_or_init(|| unsafe { CreateSolidBrush(COLOR_BG).0 as usize });
    HBRUSH(addr as *mut core::ffi::c_void)
}

/// Segoe UI font at `point` pt for the window's DPI (`DesktopTheme.Body`
/// is Segoe UI 10; the heading is Segoe UI 17 bold).
fn create_theme_font(hwnd: HWND, point: i32, bold: bool) -> HFONT {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi as i32 };
    // Logical height is negative: -MulDiv(point, dpi, 72).
    let height = -point * dpi / 72;
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if bold { 700 } else { 400 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            0, // DEFAULT_PITCH | FF_DONTCARE
            w!("Segoe UI"),
        )
    }
}

/// Send WM_SETFONT (Segoe UI) to every child control (heading 17 bold,
/// description / status / buttons 10).
fn apply_fonts(state: &LoginState) {
    for (hwnd, font) in [
        (state.title_hwnd, state.heading_font),
        (state.desc_hwnd, state.body_font),
        (state.status_hwnd, state.body_font),
        (state.retry_hwnd, state.body_font),
        (state.switch_hwnd, state.body_font),
    ] {
        if hwnd.is_invalid() || font.0.is_null() {
            continue;
        }
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
}

/// Paint an owner-drawn footer button (`WM_DRAWITEM`), mirroring
/// `CleanButton` in DesktopTheme.cs: rounded corners (diameter 18),
/// Segoe UI 10 text centred, no border or fill while a Quiet button
/// is idle.
///
/// * Retry is a plain `CleanButton` (Quiet = false): Surface fill +
///   Border outline at rest, SurfaceStrong fill on hover/press.
/// * "Another account" is Quiet: transparent at rest (parent background),
///   SurfaceStrong fill on hover/press, never a border.
/// Disabled text is Muted; a focused button gets an Accent outline.
fn draw_button(dis: &DRAWITEMSTRUCT, body_font: HFONT) {
    let quiet = dis.CtlID == ID_OTHER_ACCOUNT as u32;
    let state = dis.itemState.0;
    let hot = state & ODS_HOTLIGHT.0 != 0;
    let pressed = state & ODS_SELECTED.0 != 0;
    let disabled = state & ODS_DISABLED.0 != 0;
    let focused = state & ODS_FOCUS.0 != 0;
    let hdc: HDC = dis.hDC;
    let rc = dis.rcItem;
    unsafe {
        let _ = FillRect(hdc, &rc, theme_brush());
        let active = hot || pressed;
        if !quiet || active {
            let fill = CreateSolidBrush(if active {
                COLOR_SURFACE_STRONG
            } else {
                COLOR_SURFACE
            });
            // Quiet hover: outline in the fill colour (no visible border).
            // Plain button: Border grey, Accent while focused.
            let border_color = if quiet {
                if active {
                    COLOR_SURFACE_STRONG
                } else {
                    COLOR_SURFACE
                }
            } else if focused {
                COLOR_ACCENT
            } else {
                COLOR_BORDER
            };
            let pen = CreatePen(PS_SOLID, 1, border_color);
            let old_brush = SelectObject(hdc, HGDIOBJ(fill.0));
            let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
            let _ = RoundRect(
                hdc,
                rc.left + 1,
                rc.top + 1,
                rc.right - 1,
                rc.bottom - 1,
                18,
                18,
            );
            let _ = SelectObject(hdc, old_brush);
            let _ = SelectObject(hdc, old_pen);
            let _ = DeleteObject(HGDIOBJ(fill.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
        let text = if quiet {
            strings::OTHER_ACCOUNT
        } else {
            strings::RETRY
        };
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let old_font = SelectObject(hdc, HGDIOBJ(body_font.0));
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, if disabled { COLOR_MUTED } else { COLOR_TEXT });
        let mut text_rc = rc;
        let _ = DrawTextW(
            hdc,
            &mut wide,
            &mut text_rc,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
        let _ = SelectObject(hdc, old_font);
    }
}

/// Host part of an absolute URL (`https://muse.ai/x` -> `muse.ai`),
/// mirroring `Uri.Host` in LoginForm.cs.
fn url_host(url: &str) -> Option<&str> {
    let after_scheme = url.split("://").nth(1)?;
    Some(after_scheme.split(['/', '?', '#']).next()?)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Read all cookies for `MUSE_SITE` via the CookieManager
/// (LoginForm.cs: `manager.GetCookiesAsync(MuseSite)`).
fn get_cookies(
    manager: &ICoreWebView2CookieManager,
) -> Result<Vec<CookieEntry>, LoginError> {
    let out: std::rc::Rc<RefCell<Option<Vec<CookieEntry>>>> =
        std::rc::Rc::new(RefCell::new(None));
    let out2 = out.clone();
    let uri = CoTaskMemPWSTR::from(MUSE_SITE);
    let manager = manager.clone();
    GetCookiesCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            manager
                .GetCookies(*uri.as_ref().as_pcwstr(), &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, cookies| {
            error_code?;
            let cookies = cookies
                .ok_or_else(|| windows::core::Error::from(E_POINTER))?;
            let mut count = 0u32;
            unsafe { cookies.Count(&mut count)? };
            let mut entries = Vec::with_capacity(count as usize);
            for i in 0..count {
                let cookie = unsafe { cookies.GetValueAtIndex(i)? };
                let mut name = PWSTR::null();
                let mut value = PWSTR::null();
                let mut expires = 0f64;
                let mut is_session = BOOL(0);
                unsafe {
                    cookie.Name(&mut name)?;
                    cookie.Value(&mut value)?;
                    cookie.Expires(&mut expires)?;
                    cookie.IsSession(&mut is_session)?;
                }
                entries.push(CookieEntry {
                    name: take_pwstr(name),
                    value: take_pwstr(value),
                    // WebView2 Expires is Unix seconds (f64); LoginForm.cs
                    // converts cookie.Expires (UTC DateTime) the same way.
                    expires_unix: if is_session.0 != 0 {
                        None
                    } else {
                        Some(expires as i64)
                    },
                });
            }
            *out2.borrow_mut() = Some(entries);
            Ok(())
        }),
    )
    .map_err(wv_err)?;
    let taken = out.borrow_mut().take();
    taken.ok_or_else(|| LoginError::WebView("GetCookies returned no cookie list".to_string()))
}

/// Run `script` in the page (LoginForm.cs: `ExecuteScriptAsync`).
/// The result is delivered back via `postMessage`, not via the return value.
fn execute_script(webview: &ICoreWebView2, script: &str) -> Result<(), LoginError> {
    let js = CoTaskMemPWSTR::from(script);
    let webview = webview.clone();
    ExecuteScriptCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            webview
                .ExecuteScript(*js.as_ref().as_pcwstr(), &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(|error_code, _result| error_code),
    )
    .map_err(wv_err)
}

/// Create the WebView2 environment with its own profile folder and the
/// controller hosted in `parent` (mirrors `InitWebView` in LoginForm.cs:
/// `CoreWebView2Environment.CreateAsync(null, <Root>\WebView2)`).
fn create_environment(
    user_data_folder: &str,
) -> Result<ICoreWebView2Environment, LoginError> {
    let (tx, rx) = mpsc::channel();
    let folder = CoTaskMemPWSTR::from(user_data_folder);
    let empty = CoTaskMemPWSTR::from("");
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                *empty.as_ref().as_pcwstr(),
                *folder.as_ref().as_pcwstr(),
                None::<&ICoreWebView2EnvironmentOptions>,
                &handler,
            )
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("send environment over mpsc channel");
            Ok(())
        }),
    )
    .map_err(wv_err)?;
    rx.recv()
        .map_err(|_| LoginError::WebView("environment creation produced no result".to_string()))?
        .map_err(win_err)
}

fn create_controller(
    environment: &ICoreWebView2Environment,
    parent: HWND,
) -> Result<ICoreWebView2Controller, LoginError> {
    let (tx, rx) = mpsc::channel();
    let environment = environment.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            environment
                .CreateCoreWebView2Controller(parent, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("send controller over mpsc channel");
            Ok(())
        }),
    )
    .map_err(wv_err)?;
    rx.recv()
        .map_err(|_| LoginError::WebView("controller creation produced no result".to_string()))?
        .map_err(win_err)
}

/// The WebView2 rectangle inside the window (heading on top, footer below).
fn webview_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    RECT {
        left: 0,
        top: HEADING_HEIGHT,
        right: rect.right - rect.left,
        bottom: rect.bottom - rect.top - FOOTER_HEIGHT,
    }
}

fn layout_children(state: &LoginState) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(state.hwnd, &mut rect);
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let footer_y = height - FOOTER_HEIGHT;

    if let Some(controller) = &state.controller {
        let bounds = RECT {
            left: 0,
            top: HEADING_HEIGHT,
            right: width,
            bottom: footer_y,
        };
        unsafe {
            let _ = controller.SetBounds(bounds);
        }
    }
    if !state.title_hwnd.is_invalid() {
        unsafe {
            let _ = MoveWindow(state.title_hwnd, PADDING, 6, width - 2 * PADDING, 30, true);
        }
    }
    if !state.desc_hwnd.is_invalid() {
        unsafe {
            let _ = MoveWindow(state.desc_hwnd, PADDING, 38, width - 2 * PADDING, 26, true);
        }
    }
    // Footer: status label fills, Retry (100) + Other account (150) right.
    if !state.status_hwnd.is_invalid() {
        unsafe {
            let _ = MoveWindow(
                state.status_hwnd,
                PADDING,
                footer_y + 16,
                width - PADDING - 100 - 10 - 150 - PADDING,
                24,
                true,
            );
        }
    }
    if !state.retry_hwnd.is_invalid() {
        unsafe {
            let _ = MoveWindow(state.retry_hwnd, width - PADDING - 150 - 10 - 100, footer_y + 12, 100, 30, true);
        }
    }
    if !state.switch_hwnd.is_invalid() {
        unsafe {
            let _ = MoveWindow(state.switch_hwnd, width - PADDING - 150, footer_y + 12, 150, 30, true);
        }
    }
}

/// Restore saved cookies into the WebView when its session cookies are
/// missing (LoginForm.cs `InitWebView` saved-account branch).
fn restore_saved_cookies(
    manager: &ICoreWebView2CookieManager,
    saved: &[SavedCookie],
) -> Result<(), LoginError> {
    if saved.is_empty() {
        return Ok(());
    }
    let existing = get_cookies(manager)?;
    let pairs: Vec<(&str, &str)> = existing
        .iter()
        .map(|e| (e.name.as_str(), e.value.as_str()))
        .collect();
    if has_all_required(&pairs) {
        return Ok(());
    }
    let now = now_unix();
    for cookie_in in saved.iter().filter(|c| {
        crate::cookies::REQUIRED_COOKIES.contains(&c.name.as_str())
            || crate::cookies::OPTIONAL_COOKIES.contains(&c.name.as_str())
    }) {
        let name = CoTaskMemPWSTR::from(cookie_in.name.as_str());
        let value = CoTaskMemPWSTR::from(cookie_in.value.as_str());
        let domain = CoTaskMemPWSTR::from(".muse.ai");
        let path = CoTaskMemPWSTR::from("/");
        // LoginForm.cs: manager.CreateCookie(name, value, ".muse.ai", "/");
        // cookie.IsSecure = true; Expires set only when in the future.
        let cookie = unsafe {
            manager.CreateCookie(
                *name.as_ref().as_pcwstr(),
                *value.as_ref().as_pcwstr(),
                *domain.as_ref().as_pcwstr(),
                *path.as_ref().as_pcwstr(),
            )
        }
        .map_err(win_err)?;
        unsafe {
            cookie.SetIsSecure(true).map_err(win_err)?;
            if let Some(expires) = cookie_in.expires_unix {
                if expires > now {
                    cookie.SetExpires(expires as f64).map_err(win_err)?;
                }
            }
            manager.AddOrUpdateCookie(&cookie).map_err(win_err)?;
        }
    }
    Ok(())
}

/// Delete every cookie in the WebView profile and VERIFY the deletion by
/// reading the cookies back before the caller navigates.
///
/// Cacat A (3 Nov 2026): `ICoreWebView2CookieManager::DeleteAllCookies`
/// in this WebView2 SDK version is fire-and-forget — it takes no
/// completion handler and returns at once. The old code navigated
/// immediately after the call, so the first navigation could still carry
/// the old session cookies and muse.ai opened already signed in as the
/// OLD account ("seperti cache tersimpan"); the gateway then merged the
/// save into the old account instead of adding a new one. Now: poll
/// `GetCookies` (each call pumps the message loop) until the required
/// session cookies are gone, up to 5 s, and the caller logs the counts
/// as `session-cleared` BEFORE any navigation — the next add-account
/// attempt can be checked from `login.log` alone.
/// Returns (cookies seen before, cookies left after).
fn clear_all_cookies(
    manager: &ICoreWebView2CookieManager,
) -> Result<(usize, usize), LoginError> {
    let before = get_cookies(manager)?.len();
    if before == 0 {
        return Ok((0, 0));
    }
    unsafe {
        manager.DeleteAllCookies().map_err(win_err)?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = get_cookies(manager)?;
        let required_left = remaining
            .iter()
            .filter(|e| crate::cookies::REQUIRED_COOKIES.contains(&e.name.as_str()))
            .count();
        if required_left == 0 {
            return Ok((before, remaining.len()));
        }
        if Instant::now() >= deadline {
            return Ok((before, remaining.len()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Initialise WebView2 for the window (LoginForm.cs `InitWebView`).
/// On failure the window stays open with Retry visible, like .NET.
fn init_webview(ptr: StatePtr) -> Result<(), LoginError> {
    let cell = unsafe { &*ptr };
    // The data root is kept in `LoginState::data_root`.
    let (hwnd, data_root, saved, clear_session): (
        HWND,
        std::path::PathBuf,
        Vec<SavedCookie>,
        bool,
    ) = {
        let s = cell.borrow();
        (
            s.hwnd,
            s.data_root.clone(),
            s.saved_cookies.clone(),
            s.clear_session,
        )
    };
    let user_data_folder = data_root.join("WebView2");
    let user_data_folder = user_data_folder.to_string_lossy().into_owned();

    let environment = create_environment(&user_data_folder)?;
    let controller = create_controller(&environment, hwnd)?;
    let webview = unsafe { controller.CoreWebView2() }.map_err(win_err)?;
    unsafe {
        controller
            .SetBounds(webview_rect(hwnd))
            .map_err(win_err)?;
        controller.SetIsVisible(true).map_err(win_err)?;
    }
    // Dark WebView background (DesktopTheme.Background) so the page area
    // is not white while muse.ai loads. Best effort: ICoreWebView2Controller2
    // needs a WebView2 Runtime new enough to expose it; if the cast fails
    // the default (white) background stays and login still works.
    if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
        let _ = unsafe {
            controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                A: 255,
                R: 20,
                G: 20,
                B: 20,
            })
        };
    }
    // CookieManager lives on ICoreWebView2_2 (WebView2 SDK >= 1.0.705).
    let webview2: ICoreWebView2_2 = webview.cast().map_err(win_err)?;
    let manager = unsafe { webview2.CookieManager() }.map_err(win_err)?;

    // WebMessageReceived: accept only muse.ai + kind + nonce (LoginForm.cs).
    let message_ptr = ptr;
    let message_handler = WebMessageReceivedEventHandler::create(Box::new(
        move |_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let mut source = PWSTR::null();
            if unsafe { args.Source(&mut source) }.is_err() {
                return Ok(());
            }
            let source_str = take_pwstr(source);
            let host = url_host(&source_str).unwrap_or("");
            let mut json = PWSTR::null();
            if unsafe { args.WebMessageAsJson(&mut json) }.is_err() {
                return Ok(());
            }
            let json_str = take_pwstr(json);
            let cell = unsafe { &*message_ptr };
            let (nonce, hwnd) = {
                let s = cell.borrow();
                (s.nonce.clone(), s.hwnd)
            };
            if let Some(authenticated) = parse_session_message(host, &json_str, &nonce) {
                log_stage("session-message", &format!("authenticated={authenticated}"));
                let status_hwnd = cell.borrow().status_hwnd;
                let show_not_confirmed = {
                    let mut s = cell.borrow_mut();
                    s.session_verified = authenticated;
                    if authenticated {
                        unsafe {
                            let _ = PostMessageW(
                                Some(hwnd),
                                MSG_SESSION_VERIFIED,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }
                        false
                    } else {
                        // A "not confirmed" status replaces any earlier
                        // waiting-redirect status; clear the flag so a later
                        // redirect poll shows it again instead of assuming
                        // it is still on screen.
                        s.waiting_redirect = false;
                        true
                    }
                };
                // set_status runs with no borrow held (see its docs:
                // SetWindowTextW synchronously re-enters the window proc).
                if show_not_confirmed {
                    set_status(status_hwnd, strings::SESSION_NOT_CONFIRMED);
                }
            }
            Ok(())
        },
    ));
    let mut token: i64 = 0;
    unsafe {
        webview
            .add_WebMessageReceived(&message_handler, &mut token)
            .map_err(win_err)?;
    }

    // NavigationCompleted -> re-run the status check (LoginForm.cs).
    let nav_ptr = ptr;
    let nav_handler = NavigationCompletedEventHandler::create(Box::new(
        move |_sender, _args| {
            let cell = unsafe { &*nav_ptr };
            let hwnd = cell.borrow().hwnd;
            unsafe {
                let _ = PostMessageW(Some(hwnd), MSG_REFRESH, WPARAM(0), LPARAM(0));
            }
            Ok(())
        },
    ));
    let mut nav_token: i64 = 0;
    unsafe {
        webview
            .add_NavigationCompleted(&nav_handler, &mut nav_token)
            .map_err(win_err)?;
    }

    {
        let mut s = cell.borrow_mut();
        s.controller = Some(controller);
        s.webview = Some(webview);
        s.cookie_manager = Some(manager);
    }

    // Restore saved cookies before navigating (LoginForm.cs order) — unless
    // this window was opened for a different account: then drop the saved
    // session entirely and start from a blank login page (LoginForm.cs
    // `_clearExistingSession`). The gateway accounts are untouched either
    // way; only the WebView profile cookies are affected.
    // Cacat A: the clear is VERIFIED by reading the cookies back
    // (`clear_all_cookies`) and logged as `session-cleared` BEFORE the
    // first navigation — a fire-and-forget DeleteAllCookies followed by
    // an immediate Navigate is what let the old session ride along.
    // A verification failure must not brick the window: the deletion was
    // still requested, so log it and navigate anyway.
    // Cacat B (3 Nov 2026): the order is exclusive — clear runs
    // INSTEAD of restore, never before it — and both branches log
    // which one ran, so `login.log` alone proves whether a restore
    // could have undone the clear. A restore AFTER a clear would be a
    // bug; there is no such path (the two branches are `if`/`else` and
    // `switch_account` only clears).
    let manager = cell.borrow().cookie_manager.clone().unwrap();
    if clear_session {
        match clear_all_cookies(&manager) {
            Ok((deleted, remaining)) => {
                if remaining == 0 {
                    log_stage(
                        "session-cleared",
                        &format!("cookies=0 (deleted {deleted})"),
                    );
                } else {
                    log_stage(
                        "session-cleared",
                        &format!("cookies={remaining} left (deleted {deleted})"),
                    );
                    log_stage(
                        "session-clear-incomplete",
                        &format!("{remaining} cookie(s) still present after clear"),
                    );
                }
                log_stage(
                    "restore-skipped",
                    "clear-session: saved cookies NOT restored, navigating blank",
                );
            }
            Err(e) => {
                log_stage(
                    "session-clear-check-failed",
                    &format!("clear requested, verification failed: {}", short(&e.to_string())),
                );
                log_stage(
                    "restore-skipped",
                    "clear-session: saved cookies NOT restored (clear unverified)",
                );
            }
        }
    } else {
        restore_saved_cookies(&manager, &saved)?;
        log_stage(
            "restore-saved",
            &format!(
                "saved_cookies={} offered to WebView (restored only if session cookies were missing)",
                saved.len()
            ),
        );
    }

    let url = CoTaskMemPWSTR::from(MUSE_SITE);
    let (webview, status_hwnd) = {
        let s = cell.borrow();
        (s.webview.clone().unwrap(), s.status_hwnd)
    };
    unsafe {
        webview
            .Navigate(*url.as_ref().as_pcwstr())
            .map_err(win_err)?;
    }
    set_status(status_hwnd, strings::WAITING_SIGN_IN);
    unsafe {
        SetTimer(Some(hwnd), TIMER_ID, POLL_INTERVAL_MS as u32, None);
    }
    log_stage("webview-init", "ok, navigating to muse.ai");
    Ok(())
}

/// One poll step (LoginForm.cs `RefreshStatus`).
///
/// Never holds a state borrow across a pumping call; re-entrant calls are
/// dropped while `refreshing` is set.
fn refresh(ptr: StatePtr) {
    let cell = unsafe { &*ptr };
    {
        let mut s = cell.borrow_mut();
        if s.refreshing || s.loading || s.failed || s.closing || s.webview.is_none() {
            return;
        }
        s.refreshing = true;
    }
    let result = refresh_inner(ptr);
    {
        let mut s = cell.borrow_mut();
        s.refreshing = false;
        if let Err(err) = result {
            // LoginForm.cs catch: mark failed, show Retry. (The window
            // stays usable; Retry re-runs the check without re-login.)
            s.failed = true;
            s.loading = false;
            log_stage("refresh-failed", &short(&err.to_string()));
            let _ = err;
        } else {
            return;
        }
    }
    // set_status / show_retry run with no borrow held (see their docs:
    // SetWindowTextW synchronously re-enters the window proc).
    let (status_hwnd, retry_hwnd) = {
        let s = cell.borrow();
        (s.status_hwnd, s.retry_hwnd)
    };
    set_status(status_hwnd, strings::LOGIN_SAVE_FAILED);
    show_retry(retry_hwnd, true);
}

fn refresh_inner(ptr: StatePtr) -> Result<(), LoginError> {
    let cell = unsafe { &*ptr };
    // Snapshot inputs, then drop the borrow before any pumping call.
    // status_hwnd is snapshotted here too: every set_status call below
    // uses this local, never a borrow held across SetWindowTextW.
    let (webview, manager, nonce, verified, next_check, hwnd, gateway_url, admin_key, label, status_hwnd) = {
        let s = cell.borrow();
        (
            s.webview.clone().unwrap(),
            s.cookie_manager.clone().unwrap(),
            s.nonce.clone(),
            s.session_verified,
            s.next_session_check,
            s.hwnd,
            s.gateway_url.clone(),
            s.admin_key.clone(),
            s.label.clone(),
            s.status_hwnd,
        )
    };

    let cookies = get_cookies(&manager)?;
    let pairs: Vec<(&str, &str)> = cookies
        .iter()
        .map(|e| (e.name.as_str(), e.value.as_str()))
        .collect();
    if !has_all_required(&pairs) {
        // User has not signed in yet (or the session expired).
        {
            let mut s = cell.borrow_mut();
            s.waiting_redirect = false;
        }
        return Ok(());
    }

    // Log once when the required cookies first show up complete (counts
    // only — never cookie values).
    {
        let mut s = cell.borrow_mut();
        if !s.cookies_seen {
            s.cookies_seen = true;
            log_stage(
                "cookies-complete",
                &format!(
                    "required={} present, total={}",
                    crate::cookies::REQUIRED_COOKIES.len(),
                    cookies.len()
                ),
            );
        }
    }

    if !verified {
        // Throttle: at most one injected check per 15 s (LoginForm.cs).
        // Unchanged: a throttled poll returns without touching the
        // status — it already shows the last state.
        let now = Instant::now();
        if let Some(next) = next_check {
            if now < next {
                return Ok(());
            }
        }
        // Only check while the page is actually muse.ai.
        let mut source = PWSTR::null();
        unsafe {
            webview.Source(&mut source).map_err(win_err)?;
        }
        let source_str = take_pwstr(source);
        // Inputs are fixed here — cookies complete, session unverified,
        // poll not throttled (all three were checked above) — only the
        // host varies, so the decision is the branch under test in
        // `refresh.rs`.
        let host_is_muse = url_host(&source_str) == Some(MUSE_HOST);
        match decide_refresh(true, false, false, host_is_muse) {
            // Perbaikan 1 (3 Nov 2026): cookies complete but the page is
            // on a login redirect (Meta/OAuth) — the old code returned
            // silently here and the window sat forever on "Waiting for
            // you to sign in". Show the waiting-redirect status instead
            // and inject nothing; the security gate is untouched (the
            // check is still only injected on muse.ai) and the next poll
            // re-decides, so when the host is muse.ai again the check
            // runs as before. The status is set (and logged) only on
            // entry, so the 2 s poll does not spam the log; it never
            // overwrites a success/failure status — this branch only
            // runs while !verified, and refresh() already skips polls
            // while loading/failed/closing.
            RefreshDecision::WaitingRedirect => {
                let show = {
                    let mut s = cell.borrow_mut();
                    if !s.waiting_redirect {
                        s.waiting_redirect = true;
                        true
                    } else {
                        false
                    }
                };
                if show {
                    set_status(status_hwnd, strings::LOGIN_WAITING_REDIRECT);
                    log_stage(
                        "waiting-redirect",
                        &format!("host={}", url_host(&source_str).unwrap_or("")),
                    );
                }
                return Ok(());
            }
            RefreshDecision::InjectCheck => {
                {
                    let mut s = cell.borrow_mut();
                    s.waiting_redirect = false;
                    s.next_session_check =
                        Some(now + Duration::from_secs(SESSION_CHECK_THROTTLE_SECS));
                }
                set_status(status_hwnd, strings::CHECKING_SESSION);
            }
            // With (complete, unverified, unthrottled) inputs only the
            // two arms above are reachable.
            _ => unreachable!("decide_refresh with complete+unverified+unthrottled"),
        }
        execute_script(&webview, &session_check_script(&nonce))?;
        log_stage("session-check-injected", "fetch /api/session, throttle 15s");
        return Ok(());
    }

    // Session verified: save the cookies to the gateway.
    {
        let mut s = cell.borrow_mut();
        s.loading = true;
    }
    set_status(status_hwnd, strings::LOGIN_SUCCESS_SAVING);
    let entries = filter_login_cookies(&cookies);
    let account_id = match post_cookies_to_gateway(&gateway_url, &admin_key, &label, &entries)
    {
        Ok(id) => {
            // The account id itself is printed by cmd_login
            // ("login complete: account …"); here only the outcome and
            // the cookie count are logged — no values.
            log_stage("gateway-post", &format!("ok, cookies={}", entries.len()));
            id
        }
        Err(err) => {
            // The error already carries the HTTP status
            // ("HTTP <status>: <snippet>"); capped to one short line.
            log_stage("gateway-post", &format!("failed: {}", short(&err.to_string())));
            let mut s = cell.borrow_mut();
            s.loading = false;
            return Err(err);
        }
    };
    {
        let mut s = cell.borrow_mut();
        s.loading = false;
        s.result = Some(account_id.clone());
        s.closing = true;
    }
    set_status(status_hwnd, strings::LOGIN_CONNECTED_DONE);
    // Hand the id out before the window is destroyed (WM_NCDESTROY frees
    // the state box before the message loop exits). The window stays open
    // ~1 s on "Connected" so the user sees the success instead of the
    // window vanishing at once (twice mistaken for a crash); the close
    // timer (WM_TIMER / TIMER_CLOSE_ID) destroys it.
    store_login_result(account_id);
    unsafe {
        let _ = SetTimer(Some(hwnd), TIMER_CLOSE_ID, CLOSE_DELAY_MS, None);
    }
    Ok(())
}

/// "Other account": clear all WebView cookies and navigate again
/// (LoginForm.cs `_switch.Click`).
fn switch_account(ptr: StatePtr) {
    let cell = unsafe { &*ptr };
    let (webview, manager, retry_hwnd) = {
        let mut s = cell.borrow_mut();
        if s.webview.is_none() || s.loading || s.refreshing || s.closing {
            return;
        }
        s.session_verified = false;
        s.next_session_check = None;
        s.failed = false;
        s.cookies_seen = false;
        s.waiting_redirect = false;
        (
            s.webview.clone().unwrap(),
            s.cookie_manager.clone().unwrap(),
            s.retry_hwnd,
        )
    };
    show_retry(retry_hwnd, false);
    // Cacat A: same verified clear as `init_webview` — never navigate on
    // an unverified fire-and-forget delete.
    match clear_all_cookies(&manager) {
        Ok((deleted, remaining)) => {
            log_stage(
                "switch-account",
                &format!(
                    "cookies cleared, deleted={deleted} remaining={remaining}, navigating to muse.ai"
                ),
            );
        }
        Err(e) => {
            log_stage(
                "switch-account-clear-failed",
                &format!("clear requested, verification failed: {}", short(&e.to_string())),
            );
            log_stage("switch-account", "navigating to muse.ai");
        }
    }
    unsafe {
        let url = CoTaskMemPWSTR::from(MUSE_SITE);
        let _ = webview.Navigate(*url.as_ref().as_pcwstr());
    }
    let status_hwnd = cell.borrow().status_hwnd;
    set_status(status_hwnd, strings::LOGIN_OTHER_ACCOUNT_STATUS);
}

/// Retry button (LoginForm.cs `_retry.Click`).
fn retry(ptr: StatePtr) {
    let cell = unsafe { &*ptr };
    let (has_webview, retry_hwnd) = {
        let mut s = cell.borrow_mut();
        if s.closing {
            return;
        }
        s.failed = false;
        (s.webview.is_some(), s.retry_hwnd)
    };
    show_retry(retry_hwnd, false);
    if has_webview {
        refresh(ptr);
    } else if let Err(err) = init_webview(ptr) {
        let (status_hwnd, retry_hwnd) = {
            let s = cell.borrow();
            (s.status_hwnd, s.retry_hwnd)
        };
        set_status(status_hwnd, strings::LOGIN_WINDOW_FAILED);
        show_retry(retry_hwnd, true);
        let _ = err;
    }
}

unsafe fn state_cell(hwnd: HWND) -> Option<&'static StateCell> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if raw == 0 {
        None
    } else {
        Some(unsafe { &*(raw as *const StateCell) })
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    match msg {
        WM_GETMINMAXINFO => {
            let mmi = unsafe { &mut *(l_param.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = WINDOW_MIN_WIDTH;
            mmi.ptMinTrackSize.y = WINDOW_MIN_HEIGHT;
            LRESULT(0)
        }
        WM_SIZE => {
            if let Some(cell) = unsafe { state_cell(hwnd) } {
                // try_borrow, not borrow(): layout can be requested
                // while a mutable borrow is held (see the module docs);
                // skipping one layout pass is harmless — the next
                // resize or explicit layout lays the children out.
                if let Ok(s) = cell.try_borrow() {
                    layout_children(&s);
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            // Heading / description / status labels: dark background,
            // Text for the heading, Muted for description + status
            // (DesktopTheme.Label: heading 17 bold Text, desc/status 10 Muted).
            // NEVER borrow() here: this handler runs synchronously
            // inside SetWindowTextW (sent by set_status), which can be
            // called while a mutable borrow is held elsewhere on this
            // thread — a plain borrow() panicked here on 3 Nov 2026
            // ("RefCell already mutably borrowed", windows.rs:1056)
            // inside a window proc that cannot unwind, aborting the
            // login child. try_borrow() falls back to the muted colour
            // for one paint; the next paint picks up the real value.
            if let Some(cell) = unsafe { state_cell(hwnd) } {
                let hdc = HDC(w_param.0 as *mut core::ffi::c_void);
                let ctl = HWND(l_param.0 as *mut core::ffi::c_void);
                let color = match cell.try_borrow() {
                    Ok(s) => {
                        if ctl == s.title_hwnd {
                            COLOR_TEXT
                        } else {
                            COLOR_MUTED
                        }
                    }
                    Err(_) => COLOR_MUTED,
                };
                unsafe {
                    let _ = SetTextColor(hdc, color);
                    let _ = SetBkMode(hdc, TRANSPARENT);
                }
                return LRESULT(theme_brush().0 as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) }
        }
        WM_DRAWITEM => {
            // Owner-drawn footer buttons (BS_OWNERDRAW): paint like CleanButton.
            let dis = unsafe { &*(l_param.0 as *const DRAWITEMSTRUCT) };
            if let Some(cell) = unsafe { state_cell(hwnd) } {
                // Same re-entrancy rule as WM_CTLCOLORSTATIC: paint can
                // be dispatched while a mutable borrow is held, so never
                // borrow() here. On a failed try_borrow, pass a null
                // font — GDI keeps the DC font, the button still paints.
                let font = match cell.try_borrow() {
                    Ok(s) => s.body_font,
                    Err(_) => HFONT(std::ptr::null_mut()),
                };
                draw_button(dis, font);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if w_param.0 == TIMER_ID {
                if let Some(cell) = unsafe { state_cell(hwnd) } {
                    let ptr = cell as *const StateCell as StatePtr;
                    refresh(ptr);
                }
            } else if w_param.0 == TIMER_CLOSE_ID {
                // "Connected" was visible for ~1 s; close the window now.
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_CLOSE_ID);
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        MSG_SESSION_VERIFIED | MSG_REFRESH => {
            if let Some(cell) = unsafe { state_cell(hwnd) } {
                let ptr = cell as *const StateCell as StatePtr;
                refresh(ptr);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            // LOWORD(wParam) = control id, HIWORD(wParam) = notification.
            let id = (w_param.0 & 0xffff) as isize;
            let code = ((w_param.0 >> 16) & 0xffff) as u32;
            if code == BN_CLICKED {
                if let Some(cell) = unsafe { state_cell(hwnd) } {
                    let ptr = cell as *const StateCell as StatePtr;
                    if id == ID_RETRY {
                        retry(ptr);
                    } else if id == ID_OTHER_ACCOUNT {
                        switch_account(ptr);
                    }
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ID);
                let _ = KillTimer(Some(hwnd), TIMER_CLOSE_ID);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // Free the state box; this is the last message for the window.
            let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
            if raw != 0 {
                unsafe {
                    // Children are gone by now, so the theme fonts can be deleted.
                    let cell = &*(raw as *const StateCell);
                    let (body_font, heading_font) = {
                        let s = cell.borrow();
                        (s.body_font, s.heading_font)
                    };
                    if !body_font.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(body_font.0));
                    }
                    if !heading_font.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(heading_font.0));
                    }
                    let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(raw as *mut StateCell));
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) },
    }
}

fn create_child(
    hwnd: HWND,
    hinstance: HINSTANCE,
    class: PCWSTR,
    text: &str,
    id: Option<isize>,
    style: WINDOW_STYLE,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<HWND, LoginError> {
    let wide = CoTaskMemPWSTR::from(text);
    let menu = id.map(|id| HMENU(id as *mut core::ffi::c_void));
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            *wide.as_ref().as_pcwstr(),
            style,
            x,
            y,
            width,
            height,
            Some(hwnd),
            menu,
            Some(hinstance),
            None,
        )
    }
    .map_err(win_err)
}

/// Windows entry point for [`crate::show_login`].
pub fn show_login_impl(
    gateway_url: &str,
    admin_key: &str,
    label: &str,
    data_root: &std::path::Path,
    saved_cookies: &[SavedCookie],
    clear_session: bool,
) -> Result<LoginResult, LoginError> {
    // WebView2 requires an STA thread. If the caller already initialised
    // COM as MTA this fails (RPC_E_CHANGED_MODE) — the login window cannot
    // run on that thread.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() {
        return Err(win_err(hr.into()));
    }
    let we_initialized = hr == S_OK;
    let result = show_login_inner(gateway_url, admin_key, label, data_root, saved_cookies, clear_session);
    if we_initialized {
        unsafe {
            CoUninitialize();
        }
    }
    result
}

fn show_login_inner(
    gateway_url: &str,
    admin_key: &str,
    label: &str,
    data_root: &std::path::Path,
    saved_cookies: &[SavedCookie],
    clear_session: bool,
) -> Result<LoginResult, LoginError> {
    // Best effort: without this the window may render at the wrong scale on
    // high-DPI displays (the webview2-com sample does the same).
    unsafe {
        let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
    }

    let hinstance = unsafe {
        GetModuleHandleW(None)
            .map(|h| HINSTANCE(h.0))
            .map_err(win_err)?
    };
    let class_name = w!("MuseLoginWindow");
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).map_err(win_err)? },
        // Dark background (DesktopTheme.Background), not the white system colour.
        hbrBackground: theme_brush(),
        lpszClassName: class_name,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        return Err(LoginError::WebView(
            "RegisterClassW failed for the login window".to_string(),
        ));
    }

    let title = CoTaskMemPWSTR::from(strings::TITLE);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            *title.as_ref().as_pcwstr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            None,
            None,
            Some(hinstance),
            None,
        )
    }
    .map_err(win_err)?;

    // Dark title bar (WinForms renders dark; a raw Win32 window defaults
    // to a light title bar). Best effort: the attribute exists on
    // Windows 10 1809+; older systems ignore the call.
    unsafe {
        let dark = BOOL(1);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const BOOL as *const core::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
    }

    // Heading (title + description) and footer controls.
    let title_hwnd = create_child(
        hwnd,
        hinstance,
        w!("STATIC"),
        strings::HEADING,
        None,
        WS_CHILD | WS_VISIBLE,
        PADDING,
        6,
        WINDOW_WIDTH - 2 * PADDING,
        30,
    )?;
    let desc_hwnd = create_child(
        hwnd,
        hinstance,
        w!("STATIC"),
        strings::HEADING_DESC,
        None,
        WS_CHILD | WS_VISIBLE,
        PADDING,
        38,
        WINDOW_WIDTH - 2 * PADDING,
        26,
    )?;
    let status_hwnd = create_child(
        hwnd,
        hinstance,
        w!("STATIC"),
        strings::STATUS_OPENING,
        None,
        WS_CHILD | WS_VISIBLE,
        PADDING,
        WINDOW_HEIGHT - FOOTER_HEIGHT + 16,
        WINDOW_WIDTH - 2 * PADDING,
        24,
    )?;
    // Owner-drawn buttons (BS_OWNERDRAW, painted in WM_DRAWITEM like
    // CleanButton); Retry is a plain button, "Another account" is Quiet.
    let retry_hwnd = create_child(
        hwnd,
        hinstance,
        w!("BUTTON"),
        strings::RETRY,
        Some(ID_RETRY),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        WINDOW_WIDTH - PADDING - 150 - 10 - 100,
        WINDOW_HEIGHT - FOOTER_HEIGHT + 12,
        100,
        30,
    )?;
    let switch_hwnd = create_child(
        hwnd,
        hinstance,
        w!("BUTTON"),
        strings::OTHER_ACCOUNT,
        Some(ID_OTHER_ACCOUNT),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        WINDOW_WIDTH - PADDING - 150,
        WINDOW_HEIGHT - FOOTER_HEIGHT + 12,
        150,
        30,
    )?;
    unsafe {
        let _ = ShowWindow(retry_hwnd, SW_HIDE);
    }

    // Segoe UI fonts for this window's DPI (DesktopTheme.Body / heading).
    let body_font = create_theme_font(hwnd, 10, false);
    let heading_font = create_theme_font(hwnd, 17, true);

    let state = Box::new(RefCell::new(LoginState {
        hwnd,
        controller: None,
        webview: None,
        cookie_manager: None,
        status_hwnd,
        retry_hwnd,
        switch_hwnd,
        title_hwnd,
        desc_hwnd,
        gateway_url: gateway_url.to_string(),
        admin_key: admin_key.to_string(),
        label: label.to_string(),
        saved_cookies: saved_cookies.to_vec(),
        clear_session,
        nonce: new_nonce(),
        session_verified: false,
        next_session_check: None,
        cookies_seen: false,
        waiting_redirect: false,
        refreshing: false,
        loading: false,
        failed: false,
        closing: false,
        body_font,
        heading_font,
        result: None,
        data_root: data_root.to_path_buf(),
    }));
    let ptr = Box::into_raw(state);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
    }
    apply_fonts(&unsafe { &*ptr }.borrow());

    // Show the dark window FIRST (like WinForms: the form is visible while
    // the WebView loads), then start WebView2. Previously init ran before
    // ShowWindow, so the user saw nothing until the page was ready.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }

    // First WebView2 init attempt (LoginForm.cs: Load -> InitWebView).
    // Failure leaves the window open with Retry visible.
    if let Err(err) = init_webview(ptr) {
        let cell = unsafe { &*ptr };
        let (status_hwnd, retry_hwnd) = {
            let s = cell.borrow();
            (s.status_hwnd, s.retry_hwnd)
        };
        set_status(status_hwnd, strings::LOGIN_WINDOW_FAILED);
        show_retry(retry_hwnd, true);
        log_stage("webview-init-failed", &short(&err.to_string()));
        let _ = err;
    }

    layout_children(&unsafe { &*ptr }.borrow());

    // Message loop (LoginForm.cs: the form is modal; closing stops the poll).
    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 == 0 {
            break; // WM_QUIT
        }
        if ret.0 == -1 {
            break; // GetMessage error: treat as quit.
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // WM_NCDESTROY already freed the state box, so the saved account id
    // was handed out beforehand via the static slot (see below).
    let account_id = take_login_result();
    match account_id {
        Some(id) => Ok(LoginResult { account_id: id }),
        None => Err(LoginError::Cancelled),
    }
}

// ---------------------------------------------------------------------------
// Result hand-off.
//
// `WM_NCDESTROY` frees the `LoginState` box before the message loop exits,
// so the saved account id cannot be read from the state afterwards. The
// login window is modal and only one runs at a time, so a single static
// slot (guarded by a Mutex) carries the id out. This mirrors
// `LoginForm.LastAccountId`, read by the caller after the dialog closes.
// ---------------------------------------------------------------------------

static LOGIN_RESULT_SLOT: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

fn store_login_result(id: String) {
    if let Ok(mut slot) = LOGIN_RESULT_SLOT.lock() {
        *slot = Some(id);
    }
}

fn take_login_result() -> Option<String> {
    LOGIN_RESULT_SLOT.lock().ok()?.take()
}
