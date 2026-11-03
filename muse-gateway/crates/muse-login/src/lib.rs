//! Muse sign-in window — Rust port of `Muse2Api.WinForms.LoginForm` (153 lines).
//!
//! Behaviour copied from `LoginForm.cs`:
//!
//! * window 1060x800 (min 800x620), WebView2 with its **own** profile under
//!   `<data_root>\WebView2` — never the user's personal browser;
//! * navigates to `https://muse.ai`, polls every 2 seconds;
//! * verifies the session by injecting JS that calls
//!   `fetch('/api/session', {credentials:'include'})` and posts the result back
//!   via `window.chrome.webview.postMessage({kind:'muse-session-check', nonce,
//!   authenticated})`, with a random nonce per login (messages from any host
//!   other than `muse.ai` or with a wrong nonce are rejected);
//! * once verified, reads cookies through the WebView2 CookieManager —
//!   `hatch_sess` and `hatch_native_auth_device` are required (non-empty),
//!   `hatch_gw` and `hatch_vml` are optional — and POSTs them to
//!   `{gateway_url}/admin/accounts` (`reuse_session: true`, `label`);
//! * if a saved account exists but its session cookies are missing from the
//!   fresh WebView, the saved cookies are restored into the WebView first;
//! * "Other account" button clears all WebView cookies and navigates again;
//!   a Retry button appears on failure; closing the window stops the poll.
//!
//! The Windows implementation lives in [`windows`] (`#[cfg(windows)]`, built on
//! the `webview2-com` crate). On any other platform `show_login` returns a
//! [`LoginError::NotImplemented`] error — an honest stub, never a fake login.
//! All pure logic (cookie validation, session-message parsing, gateway request
//! building/parsing) is platform-independent and unit-tested here.

pub mod cookies;
pub mod gateway;
pub mod login_log;
pub mod refresh;
pub mod session_check;

#[cfg(windows)]
mod windows;

use std::fmt;
use std::path::Path;

/// Muse site the login window navigates to (LoginForm.cs: `MuseSite`).
pub const MUSE_SITE: &str = "https://muse.ai";

/// Window size from LoginForm.cs: `Size = new Size(1060, 800)`.
pub const WINDOW_WIDTH: i32 = 1060;
/// Window size from LoginForm.cs.
pub const WINDOW_HEIGHT: i32 = 800;
/// Window minimum size from LoginForm.cs: `MinimumSize = new Size(800, 620)`.
pub const WINDOW_MIN_WIDTH: i32 = 800;
/// Window minimum size from LoginForm.cs.
pub const WINDOW_MIN_HEIGHT: i32 = 620;
/// Poll interval from LoginForm.cs: `Timer { Interval = 2000 }`.
pub const POLL_INTERVAL_MS: u64 = 2000;

/// A cookie saved in the gateway (`accounts.json`) that can be restored into
/// the login WebView when the fresh WebView has no session cookies
/// (LoginForm.cs: saved-account restore in `InitWebView`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedCookie {
    /// Cookie name, e.g. `hatch_sess`.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Expiry as Unix seconds, if the cookie is not a session cookie.
    /// Expired values are ignored on restore (LoginForm.cs checks
    /// `expiry > now` before setting `Expires`).
    pub expires_unix: Option<i64>,
}

/// Successful login: the gateway stored the account and returned its id
/// (LoginForm.cs: `LastAccountId` / `result?["account"]?["id"]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginResult {
    /// Gateway account id, e.g. `acc_0123456789`.
    pub account_id: String,
}

/// Errors from [`show_login`].
#[derive(Debug)]
pub enum LoginError {
    /// The login window is Windows-only (WebView2). Returned on every other
    /// platform instead of pretending the login worked.
    NotImplemented(String),
    /// The user closed the login window before an account was saved
    /// (LoginForm.cs: form closed → poll stops, no `LastAccountId`).
    Cancelled,
    /// The gateway could not be reached, rejected the request, or returned a
    /// response without an account id.
    Gateway(String),
    /// WebView2 / Win32 setup failed on Windows (e.g. no WebView2 runtime).
    WebView(String),
    /// Bad input to [`show_login`] (empty gateway URL, ...).
    InvalidInput(String),
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoginError::NotImplemented(m) => write!(f, "login not implemented on this platform: {m}"),
            LoginError::Cancelled => write!(f, "login window closed before an account was saved"),
            LoginError::Gateway(m) => write!(f, "gateway error: {m}"),
            LoginError::WebView(m) => write!(f, "WebView2 error: {m}"),
            LoginError::InvalidInput(m) => write!(f, "invalid input: {m}"),
        }
    }
}

impl std::error::Error for LoginError {}

/// Open the Muse sign-in window and save the resulting session to the gateway.
///
/// * `gateway_url` — gateway base URL, e.g. `http://127.0.0.1:18610`
///   (no trailing slash).
/// * `admin_key` — gateway admin key, sent as `Authorization: Bearer <key>`
///   (LoginForm.cs via `GatewayClient.AddAccountWithCookies`).
/// * `label` — account label; blank means "Personal account"
///   (LoginForm.cs: `UiStrings.Get("AccountPersonal")`, English default).
/// * `data_root` — desktop data root (`DesktopDataPaths.Root` in .NET:
///   `%LocalAppData%\Muse2Api.WinForms`, or `MUSE_DESKTOP_PROFILE_ROOT`).
///   The WebView2 profile lives in `<data_root>\WebView2`.
/// * `saved_cookies` — cookies of an already-saved account, restored into the
///   WebView when its session cookies are missing (LoginForm.cs:
///   `_savedAccount`). Pass an empty slice for a fresh login. Ignored when
///   `clear_session` is true.
/// * `clear_session` — LoginForm.cs `clearExistingSession`: when true, the
///   saved cookies are NOT restored and all WebView cookies are deleted
///   before navigating, so the user signs in with a different account
///   ("Other account"). The gateway accounts themselves are untouched.
///
/// Returns the saved gateway account id. On non-Windows platforms returns
/// [`LoginError::NotImplemented`].
///
/// # Differences from CONTRACTS.md §11
///
/// The contract lists `show_login(gateway_url, admin_key, label)`. This crate
/// takes three extra parameters, as instructed by the coordinator task:
/// `data_root` (the WebView2 profile folder must be derived from it, exactly
/// like `DesktopDataPaths.Root` in `LoginForm.InitWebView`),
/// `saved_cookies` (the saved-account restore input), and `clear_session`
/// (the `clearExistingSession` constructor flag of `LoginForm`, now exposed
/// so the Tauri dialog's "Other account" button can open a fresh sign-in
/// window for a second account).
pub fn show_login(
    gateway_url: &str,
    admin_key: &str,
    label: &str,
    data_root: &Path,
    saved_cookies: &[SavedCookie],
    clear_session: bool,
) -> Result<LoginResult, LoginError> {
    if gateway_url.trim().is_empty() {
        return Err(LoginError::InvalidInput(
            "gateway_url must not be empty".to_string(),
        ));
    }
    #[cfg(windows)]
    {
        let label = if label.trim().is_empty() {
            "Personal account"
        } else {
            label
        };
        windows::show_login_impl(gateway_url, admin_key, label, data_root, saved_cookies, clear_session)
    }
    #[cfg(not(windows))]
    {
        let _ = (admin_key, label, data_root, saved_cookies, clear_session);
        Err(LoginError::NotImplemented(
            "the Muse sign-in window needs Windows + the Edge WebView2 runtime; \
             this platform has no WebView2 host. Run the login on the Windows \
             desktop application instead."
                .to_string(),
        ))
    }
}
