//! OpenAI-style error bodies, mirroring .NET `Errors.cs` (`MuseException`).
//! Shape: `{"error": {"message": ..., "type": ..., "code": ...}}`.
//!
//! The behaviour (status / type / code / message) is owned by
//! `muse-core::MuseError` (CONTRACTS.md §11). This module is only the
//! axum adapter: a transparent newtype, because the orphan rule forbids
//! implementing axum's `IntoResponse` directly on the foreign core type.
//! Constructors delegate 1:1 to the core constructors; nothing here
//! defines a second error taxonomy.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Thin axum wrapper around [`muse_core::MuseError`].
#[derive(Debug, Clone)]
pub struct MuseError(pub muse_core::MuseError);

#[allow(dead_code)] // constructors are the public error API; not all are used yet.
impl MuseError {
    pub fn invalid(message: impl Into<String>) -> Self {
        MuseError(muse_core::MuseError::invalid(message))
    }
    pub fn unauthorized() -> Self {
        MuseError(muse_core::MuseError::unauthorized(None))
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        MuseError(muse_core::MuseError::not_found(message))
    }
    pub fn not_implemented(message: impl Into<String>) -> Self {
        MuseError(muse_core::MuseError::not_implemented(message))
    }
    /// 403 `forbidden` — the caller is authenticated but the operation
    /// is refused (e.g. screen control in read-only mode, E6).
    pub fn forbidden(message: impl Into<String>) -> Self {
        MuseError(muse_core::MuseError::new(
            message,
            403,
            "server_error",
            "forbidden",
        ))
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        MuseError(muse_core::MuseError::new(
            message,
            409,
            "server_error",
            "conflict",
        ))
    }
    /// 409 `session_busy` (`.NET AgentEndpoints`: "Agent session is already
    /// running", type `conflict`, code `session_busy`).
    pub fn session_busy() -> Self {
        MuseError(muse_core::MuseError::new(
            "Agent session is already running",
            409,
            "conflict",
            "session_busy",
        ))
    }
    pub fn no_account(message: Option<&str>) -> Self {
        MuseError(muse_core::MuseError::no_account(message))
    }
    pub fn internal(message: impl Into<String>) -> Self {
        MuseError(muse_core::MuseError::internal(message))
    }
    pub fn status(&self) -> StatusCode {
        StatusCode::from_u16(self.0.status_code)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl From<muse_core::MuseError> for MuseError {
    fn from(e: muse_core::MuseError) -> Self {
        MuseError(e)
    }
}

impl From<muse_store::StoreError> for MuseError {
    fn from(e: muse_store::StoreError) -> Self {
        match e {
            muse_store::StoreError::Invalid(msg) => MuseError::invalid(msg),
            muse_store::StoreError::NotFound(msg) => MuseError::not_found(msg),
            muse_store::StoreError::TooLarge(msg) => MuseError::invalid(msg),
            other => MuseError::internal(other.to_string()),
        }
    }
}

impl From<muse_agent::AgentError> for MuseError {
    fn from(e: muse_agent::AgentError) -> Self {
        match e {
            muse_agent::AgentError::Invalid(msg) => MuseError::invalid(msg),
            muse_agent::AgentError::Cancelled => {
                MuseError(muse_core::MuseError::new("cancelled", 499, "server_error", "client_disconnected"))
            }
            muse_agent::AgentError::Io(err) => MuseError::internal(err.to_string()),
            muse_agent::AgentError::Message(msg) => MuseError::internal(msg),
        }
    }
}

impl From<muse_browser::MuseError> for MuseError {
    fn from(e: muse_browser::MuseError) -> Self {
        MuseError(muse_core::MuseError::new(
            e.message,
            e.status,
            e.error_type,
            e.code,
        ))
    }
}

impl From<muse_login::LoginError> for MuseError {
    fn from(e: muse_login::LoginError) -> Self {
        MuseError::internal(e.to_string())
    }
}

impl std::fmt::Display for MuseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.message)
    }
}

impl IntoResponse for MuseError {
    fn into_response(self) -> Response {
        (self.status(), Json(self.0.to_body())).into_response()
    }
}

pub type ApiResult<T> = Result<T, MuseError>;
