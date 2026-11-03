//! The `fetch('/api/session')` verification injected into the Muse page.
//!
//! Source: `LoginForm.RefreshStatus` (LoginForm.cs:149) injects this script
//! when the required cookies exist but the session is not verified yet, at
//! most once every 15 seconds:
//!
//! ```js
//! (async()=>{const c=new AbortController();const timer=setTimeout(()=>c.abort(),8000);
//! try{const r=await fetch('/api/session',{credentials:'include',cache:'no-store',signal:c.signal});
//! const b=r.ok?await r.json():{};
//! window.chrome.webview.postMessage({kind:'muse-session-check',nonce:<nonce>,
//! authenticated:r.ok&&b.status==='assigned'});}
//! catch{window.chrome.webview.postMessage({kind:'muse-session-check',nonce:<nonce>,
//! authenticated:false});}finally{clearTimeout(timer);}})()
//! ```
//!
//! The page answers through `WebMessageReceived`. `LoginForm` accepts the
//! message only when `message.Source` host is `muse.ai`, `kind` is
//! `muse-session-check` and `nonce` equals the per-login random nonce
//! (`Guid.NewGuid().ToString("N")` — 32 lowercase hex chars).

/// `muse.ai` — the only host a session-check message may come from.
pub const MUSE_HOST: &str = "muse.ai";

/// How long the injected `fetch` may take before it is aborted (LoginForm.cs: 8000 ms).
pub const SESSION_CHECK_TIMEOUT_MS: u64 = 8000;

/// Minimum gap between two injected session checks (LoginForm.cs:
/// `_nextSessionCheck = DateTime.UtcNow.AddSeconds(15)`).
pub const SESSION_CHECK_THROTTLE_SECS: u64 = 15;

/// Generate a fresh verification nonce: 32 lowercase hex chars, the same
/// shape as `Guid.NewGuid().ToString("N")` in LoginForm.cs.
pub fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Build the exact injected script for `nonce`.
///
/// `nonce` is JSON-encoded (quoted) before interpolation, exactly like
/// `System.Text.Json.JsonSerializer.Serialize(_verificationNonce)` in
/// LoginForm.cs — so a nonce can never break out of the JS string literal.
pub fn session_check_script(nonce: &str) -> String {
    let nonce_json = serde_json::to_string(nonce).expect("nonce is a plain string");
    format!(
        "(async()=>{{const c=new AbortController();const timer=setTimeout(()=>c.abort(),8000);\
         try{{const r=await fetch('/api/session',{{credentials:'include',cache:'no-store',signal:c.signal}});\
         const b=r.ok?await r.json():{{}};\
         window.chrome.webview.postMessage({{kind:'muse-session-check',nonce:{nonce_json},\
         authenticated:r.ok&&b.status==='assigned'}});}}\
         catch{{window.chrome.webview.postMessage({{kind:'muse-session-check',nonce:{nonce_json},\
         authenticated:false}});}}finally{{clearTimeout(timer);}}}})()"
    )
}

/// Validate a `WebMessageReceived` payload.
///
/// Returns `Some(authenticated)` only when **all** of these hold, mirroring
/// the guards in the `WebMessageReceived` handler in LoginForm.cs:
///
/// * `source_host == "muse.ai"` (the `Uri.Host` check on `message.Source`);
/// * `kind == "muse-session-check"`;
/// * `nonce == expected_nonce`.
///
/// Anything else returns `None` (message ignored, login state unchanged).
/// A malformed JSON payload also returns `None` — in .NET the handler
/// swallows the exception ("A page message cannot break the login form").
pub fn parse_session_message(
    source_host: &str,
    json: &str,
    expected_nonce: &str,
) -> Option<bool> {
    if source_host != MUSE_HOST {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value.get("kind")?.as_str()? != "muse-session-check" {
        return None;
    }
    if value.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    value.get("authenticated")?.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_32_lowercase_hex() {
        let nonce = new_nonce();
        assert_eq!(nonce.len(), 32);
        assert!(nonce.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(nonce, new_nonce());
    }

    #[test]
    fn script_contains_nonce_quoted_and_fetch() {
        let script = session_check_script("abcdef0123456789abcdef0123456789");
        assert!(script.contains("\"abcdef0123456789abcdef0123456789\""));
        assert!(script.contains("fetch('/api/session'"));
        assert!(script.contains("credentials:'include'"));
        assert!(script.contains("b.status==='assigned'"));
        assert!(script.contains("muse-session-check"));
    }

    #[test]
    fn parse_accepts_matching_message() {
        let nonce = "abcdef0123456789abcdef0123456789";
        let json = serde_json::json!({
            "kind": "muse-session-check",
            "nonce": nonce,
            "authenticated": true,
        })
        .to_string();
        assert_eq!(parse_session_message("muse.ai", &json, nonce), Some(true));
    }

    #[test]
    fn parse_rejects_wrong_host_nonce_kind_and_garbage() {
        let nonce = "abcdef0123456789abcdef0123456789";
        let good = serde_json::json!({
            "kind": "muse-session-check",
            "nonce": nonce,
            "authenticated": true,
        })
        .to_string();
        assert_eq!(parse_session_message("evil.example", &good, nonce), None);
        assert_eq!(parse_session_message("muse.ai", &good, "othernonce"), None);
        let wrong_kind = serde_json::json!({
            "kind": "something-else", "nonce": nonce, "authenticated": true,
        })
        .to_string();
        assert_eq!(parse_session_message("muse.ai", &wrong_kind, nonce), None);
        assert_eq!(parse_session_message("muse.ai", "not json{", nonce), None);
        let unauth = serde_json::json!({
            "kind": "muse-session-check", "nonce": nonce, "authenticated": false,
        })
        .to_string();
        assert_eq!(parse_session_message("muse.ai", &unauth, nonce), Some(false));
    }
}
