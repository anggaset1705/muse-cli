//! Error yang memetakan ke `MuseException` / `UpstreamException` .NET
//! (`src/Muse2Api/Errors.cs`): tiap error membawa HTTP status, `type`,
//! `code`, dan flag `retryable` (dipakai failover `Gateway.cs`).

use std::fmt;

/// Satu error gateway, dengan bentuk body OpenAI-style yang sama dengan .NET
/// (`MuseException.ToBody()`).
#[derive(Debug, Clone)]
pub struct MuseError {
    pub message: String,
    pub status: u16,
    /// `error.type` di body JSON (mis. `server_error`, `upstream_error`).
    pub error_type: String,
    pub code: String,
    /// Apakah `Gateway` boleh mencoba akun lain (`UpstreamException.Retryable`).
    pub retryable: bool,
}

impl MuseError {
    pub fn new(
        message: impl Into<String>,
        status: u16,
        error_type: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            status,
            error_type: error_type.into(),
            code: code.into(),
            retryable: true,
        }
    }

    /// `MuseException(message, 500, "server_error", "internal_error")`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(message, 500, "server_error", "internal_error")
    }

    /// `MuseException.Invalid` — 400.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(message, 400, "invalid_request_error", "invalid_request")
    }

    /// `MuseException.NotImplemented` — 501.
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(message, 501, "not_implemented", "not_implemented")
    }

    /// `MuseException.NoAccount` — 503.
    pub fn no_account(message: impl Into<String>) -> Self {
        Self::new(message, 503, "server_error", "no_account_available")
    }

    /// `UpstreamException.Auth` — 502, `upstream_auth_failed`.
    /// Dipakai bila sesi/cookie ditolak upstream (bukan bukti rate limit).
    pub fn upstream_auth(message: impl Into<String>) -> Self {
        Self::new(message, 502, "upstream_error", "upstream_auth_failed")
    }

    /// `UpstreamException.Quota` — 429, `upstream_quota_exhausted`.
    pub fn upstream_quota(message: impl Into<String>) -> Self {
        Self::new(message, 429, "upstream_error", "upstream_quota_exhausted")
    }

    /// `UpstreamException.Timeout` — 504, `upstream_timeout`.
    pub fn upstream_timeout(message: impl Into<String>) -> Self {
        Self::new(message, 504, "upstream_error", "upstream_timeout")
    }

    /// `UpstreamException.ResponseUnavailable` — 502, tidak retryable:
    /// upstream mungkin sudah menerima request; jangan pernah kirim ulang
    /// otomatis (komentar di `Errors.cs`).
    pub fn upstream_response_unavailable(message: impl Into<String>) -> Self {
        let mut e = Self::new(message, 502, "upstream_error", "upstream_response_unavailable");
        e.retryable = false;
        e
    }

    /// `UpstreamException.Refused` — 502, tidak retryable.
    pub fn upstream_refused(message: impl Into<String>) -> Self {
        let mut e = Self::new(message, 502, "upstream_error", "upstream_refused");
        e.retryable = false;
        e
    }

    /// Error khusus browser: Chromium tidak bisa di-launch.
    /// Pesan mengikuti `BrowserDriver.StartupAsync` (.NET) — bedanya jalur
    /// install Playwright diganti chromiumoxide (tidak ada `playwright.ps1`).
    pub fn browser_not_installed(message: impl Into<String>) -> Self {
        Self::new(message, 500, "server_error", "browser_not_installed")
    }

    /// Body error gaya OpenAI, sama dengan `MuseException.ToBody()`.
    pub fn to_body(&self) -> serde_json::Value {
        serde_json::json!({
            "error": {
                "message": self.message,
                "type": self.error_type,
                "code": self.code,
            }
        })
    }
}

impl fmt::Display for MuseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({} {})", self.message, self.status, self.code)
    }
}

impl std::error::Error for MuseError {}

impl From<chromiumoxide::error::CdpError> for MuseError {
    fn from(e: chromiumoxide::error::CdpError) -> Self {
        MuseError::internal(format!("CDP error: {e}"))
    }
}

impl From<reqwest::Error> for MuseError {
    fn from(e: reqwest::Error) -> Self {
        MuseError::internal(format!("HTTP error: {e}"))
    }
}

impl From<std::io::Error> for MuseError {
    fn from(e: std::io::Error) -> Self {
        MuseError::internal(format!("IO error: {e}"))
    }
}

impl From<serde_json::Error> for MuseError {
    fn from(e: serde_json::Error) -> Self {
        MuseError::internal(format!("JSON error: {e}"))
    }
}
