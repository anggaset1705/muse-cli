//! Loopback-only CORS, mirroring .NET `GatewayApplication` policy
//! `"LoopbackOnly"`.
//!
//! Allowed origins: absolute URIs with scheme `http` (case-insensitive)
//! and host `localhost`, `127.0.0.1` or `tauri.localhost` — any port.
//! Anything else gets NO CORS headers at all (the browser blocks the
//! response), exactly like .NET.
//!
//! Implemented as a small hand-rolled axum middleware instead of
//! `tower-http`'s `CorsLayer` on purpose: .NET answers preflights with
//! `204 No Content`, while tower-http answers `200 OK`. Browsers accept
//! both, but the skeleton keeps byte-level parity with the .NET gateway
//! (verified: `OPTIONS` → 204 + CORS headers).

use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

const ALLOWED_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "tauri.localhost"];

/// Mirror of .NET `SetIsOriginAllowed`: absolute http URI whose host is a
/// loopback host. No port check, no credentials mode.
pub fn origin_allowed(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    if uri.scheme_str().map(|s| !s.eq_ignore_ascii_case("http")).unwrap_or(true) {
        return false;
    }
    let Some(host) = uri.host() else {
        return false;
    };
    ALLOWED_HOSTS.contains(&host.to_ascii_lowercase().as_str())
}

fn allow_origin_value(origin: &str) -> HeaderValue {
    HeaderValue::from_str(origin).unwrap_or_else(|_| HeaderValue::from_static("*"))
}

/// axum middleware: short-circuit preflights with 204, and stamp
/// `Access-Control-Allow-Origin` (+ `Vary: Origin`) on allowed responses.
pub async fn loopback_only(req: Request<Body>, next: Next) -> Response {
    let headers = req.headers().clone();
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let Some(origin) = origin.filter(|o| origin_allowed(o)) else {
        // No (or disallowed) Origin: pass through untouched, like .NET.
        return next.run(req).await;
    };

    let is_preflight = req.method() == Method::OPTIONS
        && headers.contains_key(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD);

    if is_preflight {
        // .NET CORS middleware answers preflights with 204.
        let mut res = (StatusCode::NO_CONTENT, ()).into_response();
        let h = res.headers_mut();
        h.insert(
            axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            allow_origin_value(&origin),
        );
        let method = headers
            .get(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("*"));
        h.insert(axum::http::header::ACCESS_CONTROL_ALLOW_METHODS, method);
        let req_headers = headers
            .get(axum::http::header::ACCESS_CONTROL_REQUEST_HEADERS)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("*"));
        h.insert(axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, req_headers);
        h.insert(axum::http::header::VARY, HeaderValue::from_static("Origin"));
        return res;
    }

    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        allow_origin_value(&origin),
    );
    h.append(axum::http::header::VARY, HeaderValue::from_static("Origin"));
    res
}

// Wiring: apply with `axum::middleware::from_fn(cors::loopback_only)` as
// the outermost layer, mirroring .NET's `app.UseCors("LoopbackOnly")`
// placed right after `Build()`.

