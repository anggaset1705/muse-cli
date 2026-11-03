//! Integration tests for the gateway save path.
//!
//! A tiny fake gateway (plain `std::net::TcpListener`) answers
//! `POST /admin/accounts` so `post_cookies_to_gateway` is tested end to
//! end — request line, `Authorization: Bearer` header, JSON body — without
//! a live gateway or Windows.

use muse_login::cookies::CookieEntry;
use muse_login::gateway::post_cookies_to_gateway;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// Read one HTTP request from `stream`, then answer with `response`.
/// TCP is a stream: headers and body may arrive in separate segments, so
/// this reads until the `Content-Length` body is complete (or the peer
/// closes), instead of trusting a single `read()` to return the whole
/// request. (Integration fix, 2 Nov 2026: a single `read()` returned only
/// the headers when ureq flushed them separately from the body, so the
/// captured body was empty and the test failed deterministically.)
fn serve_once(listener: TcpListener, response: String) -> (String, String) {
    let (mut stream, _) = listener.accept().unwrap();
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let mut content_length: Option<usize> = None;
    let mut header_end: Option<usize> = None;
    loop {
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if header_end.is_none() {
            if let Some(pos) = find_subslice(&raw, b"\r\n\r\n") {
                header_end = Some(pos + 4);
                let head = String::from_utf8_lossy(&raw[..pos]).to_string();
                for line in head.lines().skip(1) {
                    if let Some(v) = line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                    {
                        content_length = v.trim().parse().ok();
                    }
                }
            }
        }
        if let (Some(end), Some(len)) = (header_end, content_length) {
            if raw.len() >= end + len {
                break;
            }
        }
    }
    let request = String::from_utf8_lossy(&raw).to_string();
    // Split head/body on the blank line.
    let body = request
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .trim_end_matches('\0')
        .to_string();
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
    (request, body)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn http_ok(json: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json.len(),
        json
    )
}

#[test]
fn post_cookies_sends_bearer_and_body_and_reads_account_id() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        serve_once(
            listener,
            http_ok(r#"{"account":{"id":"acc_0123456789","label":"Personal account"},"missing_cookies":[]}"#),
        )
    });

    let cookies = vec![
        CookieEntry {
            name: "hatch_sess".into(),
            value: "sess-value".into(),
            expires_unix: None,
        },
        CookieEntry {
            name: "hatch_native_auth_device".into(),
            value: "device-value".into(),
            expires_unix: Some(1_800_000_000),
        },
    ];
    let id = post_cookies_to_gateway(
        &format!("http://127.0.0.1:{port}"),
        "admin-key-123",
        "Personal account",
        &cookies,
    )
    .unwrap();
    assert_eq!(id, "acc_0123456789");

    let (request, body) = handle.join().unwrap();
    assert!(request.starts_with("POST /admin/accounts HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer admin-key-123"));
    let body_json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body_json["label"], "Personal account");
    assert_eq!(body_json["reuse_session"], true);
    assert_eq!(body_json["cookies"][0]["name"], "hatch_sess");
    assert_eq!(body_json["cookies"][0]["value"], "sess-value");
    assert!(body_json["cookies"][0].get("expirationDate").is_none());
    assert_eq!(body_json["cookies"][1]["expirationDate"], 1_800_000_000);
}

#[test]
fn post_cookies_reports_http_error_with_body_snippet() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        serve_once(
            listener,
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"error\":\"no\"}"
                .to_string(),
        )
    });
    let err = post_cookies_to_gateway(
        &format!("http://127.0.0.1:{port}"),
        "wrong-key",
        "Personal account",
        &[CookieEntry {
            name: "hatch_sess".into(),
            value: "s".into(),
            expires_unix: None,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("HTTP 401"), "{err}");
    handle.join().unwrap();
}

#[test]
fn post_cookies_errors_when_gateway_returns_no_account_id() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || serve_once(listener, http_ok(r#"{"account":null}"#)));
    let err = post_cookies_to_gateway(
        &format!("http://127.0.0.1:{port}"),
        "admin-key-123",
        "Personal account",
        &[CookieEntry {
            name: "hatch_sess".into(),
            value: "s".into(),
            expires_unix: None,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("No saved account returned"), "{err}");
    handle.join().unwrap();
}

#[test]
fn show_login_is_not_implemented_on_non_windows() {
    // On this Linux VM the login window cannot run; it must fail loudly
    // instead of pretending to log in.
    let err = muse_login::show_login(
        "http://127.0.0.1:18610",
        "admin-key-123",
        "Personal account",
        std::path::Path::new("/tmp/muse-login-test"),
        &[],
        false,
    )
    .unwrap_err();
    match err {
        muse_login::LoginError::NotImplemented(msg) => {
            assert!(msg.contains("Windows"), "{msg}");
        }
        other => panic!("expected NotImplemented, got {other:?}"),
    }
}

#[test]
fn show_login_rejects_empty_gateway_url() {
    let err = muse_login::show_login(
        "",
        "admin-key-123",
        "Personal account",
        std::path::Path::new("/tmp/muse-login-test"),
        &[],
        false,
    )
    .unwrap_err();
    assert!(matches!(err, muse_login::LoginError::InvalidInput(_)));
}
