//! Bearer auth, mirroring .NET `Endpoints.Bearer` +
//! `Endpoints.RequireApi` / `RequireAdmin` and
//! `AgentEndpoints.RequireAgentApi`.
//!
//! A request is authorized when its token equals (constant-time) the
//! effective admin key, the API key, or a key from `keys.json`
//! (`KeyStore.Verify`, which also stamps `last_used_at` via `Touch`).
//! Admin endpoints accept ONLY the effective admin key.

use std::sync::Mutex;

use axum::http::HeaderMap;

use muse_store::KeyStore;

use crate::errors::{ApiResult, MuseError};

/// Caller identity, mirroring .NET `RequireApi`'s `Identify` return
/// values — used for request logging (`key_id` / `key_name`).
#[derive(Debug, Clone)]
pub struct Caller {
    pub key_id: String,
    pub key_name: String,
}

impl Caller {
    pub fn admin() -> Self {
        Caller {
            key_id: "admin".to_string(),
            key_name: "admin".to_string(),
        }
    }
    pub fn legacy() -> Self {
        Caller {
            key_id: "legacy".to_string(),
            key_name: "legacy".to_string(),
        }
    }
}

/// Extract the bearer token: `Authorization: Bearer <token>`
/// (case-insensitive scheme, like .NET) or the `X-Api-Key` header
/// (`.NET Endpoints.Bearer` reads both).
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if auth.len() >= 7 && auth[..7].eq_ignore_ascii_case("bearer ") {
            let token = auth[7..].trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    headers
        .get("X-Api-Key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Constant-time equality for equal-length inputs; `false` on length
/// mismatch (mirrors `CryptographicOperations.FixedTimeEquals` + the
/// `TimingEquals` guard that returns false for an empty expected value).
pub fn timing_equals(a: &str, b: &str) -> bool {
    if b.is_empty() {
        return false;
    }
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..ab.len() {
        diff |= ab[i] ^ bb[i];
    }
    diff == 0
}

/// `Endpoints.RequireApi`: effective admin key → `admin`, API key →
/// `legacy`, else `KeyStore.Verify` (+ `Touch`); otherwise 401.
pub fn require_api(
    headers: &HeaderMap,
    effective_admin_key: &str,
    api_key: &str,
    keys: &Mutex<KeyStore>,
) -> ApiResult<Caller> {
    let provided = bearer(headers).ok_or_else(MuseError::unauthorized)?;
    if timing_equals(&provided, effective_admin_key) {
        return Ok(Caller::admin());
    }
    if timing_equals(&provided, api_key) {
        return Ok(Caller::legacy());
    }
    let matched = {
        let mut store = keys.lock().map_err(|_| MuseError::internal("key store poisoned"))?;
        match store.verify(&provided) {
            Some(k) => {
                store.touch(&k);
                Some((k.id, k.name))
            }
            None => None,
        }
    };
    match matched {
        Some((id, name)) => Ok(Caller {
            key_id: id,
            key_name: name,
        }),
        None => Err(MuseError::unauthorized()),
    }
}

/// `Endpoints.RequireAdmin`: ONLY the effective admin key; 401 otherwise.
pub fn require_admin(headers: &HeaderMap, effective_admin_key: &str) -> ApiResult<Caller> {
    let provided = bearer(headers).ok_or_else(MuseError::unauthorized)?;
    if timing_equals(&provided, effective_admin_key) {
        Ok(Caller::admin())
    } else {
        Err(MuseError::unauthorized())
    }
}

/// `AgentEndpoints.RequireAgentApi`: identical to `RequireApi` (admin key,
/// API key, or a `keys.json` key); no `key_id` is recorded by .NET here,
/// but the gateway logs the same identity as `RequireApi`.
pub fn require_agent_api(
    headers: &HeaderMap,
    effective_admin_key: &str,
    api_key: &str,
    keys: &Mutex<KeyStore>,
) -> ApiResult<Caller> {
    require_api(headers, effective_admin_key, api_key, keys)
}
