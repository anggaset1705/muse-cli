//! Error type, mirroring .NET `Errors.cs` (`MuseException` / `UpstreamException`).
//!
//! Every error maps to an HTTP status and an OpenAI-style error body:
//! `{"error": {"message": ..., "type": ..., "code": ...}}`.

use serde_json::json;

/// Error type for the Muse backend. Mirrors .NET `MuseException`; the
/// `retryable` flag mirrors `UpstreamException.Retryable` (only meaningful
/// for upstream errors; `false` for plain `MuseException`s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuseError {
    pub message: String,
    pub status_code: u16,
    pub error_type: String,
    pub code: String,
    pub retryable: bool,
}

impl MuseError {
    pub fn new(
        message: impl Into<String>,
        status_code: u16,
        error_type: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        MuseError {
            message: message.into(),
            status_code,
            error_type: error_type.into(),
            code: code.into(),
            retryable: false,
        }
    }

    fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// 500 `server_error` / `internal_error`.
    pub fn internal(message: impl Into<String>) -> Self {
        MuseError::new(message, 500, "server_error", "internal_error")
    }

    /// 400 `invalid_request_error` / `invalid_request` (`MuseException.Invalid`).
    pub fn invalid(message: impl Into<String>) -> Self {
        MuseError::new(message, 400, "invalid_request_error", "invalid_request")
    }

    /// 401 `authentication_error` / `invalid_api_key` (`MuseException.Unauthorized`).
    pub fn unauthorized(message: Option<&str>) -> Self {
        MuseError::new(
            message.unwrap_or("invalid or missing API key"),
            401,
            "authentication_error",
            "invalid_api_key",
        )
    }

    /// 404 `invalid_request_error` / `not_found` (`MuseException.NotFound`).
    pub fn not_found(message: impl Into<String>) -> Self {
        MuseError::new(message, 404, "invalid_request_error", "not_found")
    }

    /// 501 `not_implemented` / `not_implemented` (`MuseException.NotImplemented`).
    pub fn not_implemented(message: impl Into<String>) -> Self {
        MuseError::new(message, 501, "not_implemented", "not_implemented")
    }

    /// 503 `server_error` / `no_account_available` (`MuseException.NoAccount`).
    pub fn no_account(message: Option<&str>) -> Self {
        MuseError::new(
            message.unwrap_or("no usable account in pool"),
            503,
            "server_error",
            "no_account_available",
        )
    }

    /// Base upstream error: 502 `upstream_error` / `upstream_error`, retryable.
    pub fn upstream(message: impl Into<String>) -> Self {
        MuseError::new(message, 502, "upstream_error", "upstream_error")
            .with_retryable(true)
    }

    /// 502 `upstream_error` / `upstream_auth_failed` (`UpstreamException.Auth`).
    pub fn upstream_auth(message: impl Into<String>) -> Self {
        MuseError::new(message, 502, "upstream_error", "upstream_auth_failed")
            .with_retryable(true)
    }

    /// 429 `upstream_error` / `upstream_quota_exhausted` (`UpstreamException.Quota`).
    pub fn upstream_quota(message: impl Into<String>) -> Self {
        MuseError::new(message, 429, "upstream_error", "upstream_quota_exhausted")
            .with_retryable(true)
    }

    /// 504 `upstream_error` / `upstream_timeout` (`UpstreamException.Timeout`).
    pub fn upstream_timeout(message: impl Into<String>) -> Self {
        MuseError::new(message, 504, "upstream_error", "upstream_timeout")
            .with_retryable(true)
    }

    /// 502 `upstream_error` / `upstream_response_unavailable`, NOT retryable
    /// (`UpstreamException.ResponseUnavailable` — a DOM/readback problem is not
    /// evidence of rate limiting; the upstream may already have accepted the
    /// request, so never auto-resubmit).
    pub fn upstream_response_unavailable(message: impl Into<String>) -> Self {
        MuseError::new(
            message,
            502,
            "upstream_error",
            "upstream_response_unavailable",
        )
        .with_retryable(false)
    }

    /// 502 `upstream_error` / `upstream_refused`, NOT retryable
    /// (`UpstreamException.Refused`).
    pub fn upstream_refused(message: impl Into<String>) -> Self {
        MuseError::new(message, 502, "upstream_error", "upstream_refused")
            .with_retryable(false)
    }

    /// OpenAI-style error body, exactly like .NET `MuseException.ToBody()`.
    pub fn to_body(&self) -> serde_json::Value {
        json!({
            "error": {
                "message": self.message,
                "type": self.error_type,
                "code": self.code,
            }
        })
    }
}

impl std::fmt::Display for MuseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for MuseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_maps_to_400() {
        let e = MuseError::invalid("model 'x' is a video model, expected chat");
        assert_eq!(e.status_code, 400);
        assert_eq!(e.error_type, "invalid_request_error");
        assert_eq!(e.code, "invalid_request");
        assert_eq!(
            e.to_body(),
            json!({"error": {"message": "model 'x' is a video model, expected chat", "type": "invalid_request_error", "code": "invalid_request"}})
        );
    }

    #[test]
    fn unauthorized_default_message() {
        let e = MuseError::unauthorized(None);
        assert_eq!(e.status_code, 401);
        assert_eq!(e.message, "invalid or missing API key");
        assert_eq!(e.code, "invalid_api_key");
    }

    #[test]
    fn not_found_not_implemented_no_account() {
        assert_eq!(MuseError::not_found("nope").status_code, 404);
        let n = MuseError::not_implemented("Screen control is unavailable in the gateway host.");
        assert_eq!((n.status_code, n.error_type.as_str(), n.code.as_str()), (501, "not_implemented", "not_implemented"));
        let a = MuseError::no_account(None);
        assert_eq!((a.status_code, a.message.as_str(), a.code.as_str()), (503, "no usable account in pool", "no_account_available"));
    }

    #[test]
    fn upstream_variants_and_retryable() {
        assert_eq!(MuseError::upstream("boom").status_code, 502);
        assert!(MuseError::upstream("boom").retryable);
        assert_eq!(MuseError::upstream_auth("bad").code, "upstream_auth_failed");
        let q = MuseError::upstream_quota("quota");
        assert_eq!((q.status_code, q.code.as_str()), (429, "upstream_quota_exhausted"));
        assert!(q.retryable);
        assert_eq!(MuseError::upstream_timeout("slow").status_code, 504);
        let r = MuseError::upstream_response_unavailable("no dom");
        assert_eq!(r.code, "upstream_response_unavailable");
        assert!(!r.retryable);
        assert!(!MuseError::upstream_refused("no").retryable);
        // Plain MuseException is not retryable.
        assert!(!MuseError::invalid("x").retryable);
    }
}
