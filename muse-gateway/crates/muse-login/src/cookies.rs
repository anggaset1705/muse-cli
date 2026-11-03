//! Cookie rules for the Muse login flow.
//!
//! Source of truth:
//! * `LoginForm.cs`: `RequiredCookies = { "hatch_sess", "hatch_native_auth_device" }`,
//!   `OptionalCookies = { "hatch_gw", "hatch_vml" }`.
//! * `Upstream.cs`: `SessionCookies` / `OptionalSessionCookies` — same lists.
//! * `Endpoints.cs` `ParseCookies` (cookie-export list form): entries are
//!   `{name, value, expirationDate?}` where `expirationDate` is Unix seconds
//!   (double) and is only present for non-session cookies.

use crate::LoginError;

/// Cookies the session must have, non-empty
/// (LoginForm.cs `RequiredCookies`, Upstream.cs `SessionCookies`).
pub const REQUIRED_COOKIES: &[&str] = &["hatch_sess", "hatch_native_auth_device"];
/// Cookies that are sent when present (LoginForm.cs `OptionalCookies`).
pub const OPTIONAL_COOKIES: &[&str] = &["hatch_gw", "hatch_vml"];

/// One cookie read from the WebView2 CookieManager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    /// Expiry as Unix seconds; `None` for session cookies
    /// (LoginForm.cs only writes `expirationDate` when `!cookie.IsSession`).
    pub expires_unix: Option<i64>,
}

/// Names of required cookies that are missing or empty in `cookies`.
///
/// `cookies` is a list of `(name, value)` pairs. Mirrors
/// `Upstream.MissingSessionCookies` and the check in `LoginForm.RefreshStatus`:
/// `RequiredCookies.Any(name => !cookies.Any(c => c.Name == name && c.Value.Length > 0))`.
pub fn missing_required(cookies: &[(&str, &str)]) -> Vec<&'static str> {
    REQUIRED_COOKIES
        .iter()
        .copied()
        .filter(|required| {
            !cookies
                .iter()
                .any(|(name, value)| *name == *required && !value.is_empty())
        })
        .collect()
}

/// `true` when every required cookie is present with a non-empty value.
pub fn has_all_required(cookies: &[(&str, &str)]) -> bool {
    missing_required(cookies).is_empty()
}

/// Keep only the cookies the gateway cares about (required + optional),
/// dropping everything else the WebView may hold for muse.ai.
/// Mirrors the `Where(c => RequiredCookies.Contains(c.Name) || OptionalCookies.Contains(c.Name))`
/// filter in `LoginForm.RefreshStatus`.
pub fn filter_login_cookies(entries: &[CookieEntry]) -> Vec<CookieEntry> {
    entries
        .iter()
        .filter(|e| {
            REQUIRED_COOKIES.contains(&e.name.as_str())
                || OPTIONAL_COOKIES.contains(&e.name.as_str())
        })
        .cloned()
        .collect()
}

/// Build the `cookies` JSON array for `POST /admin/accounts`, in the exact
/// shape `GatewayClient.AddAccountWithCookies` sends (Client.cs):
/// `[{name, value, expirationDate?}]` — `expirationDate` only for cookies
/// that have an expiry (non-session cookies, Unix seconds).
pub fn cookie_array_json(entries: &[CookieEntry]) -> serde_json::Value {
    let array: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let mut obj = serde_json::json!({ "name": e.name, "value": e.value });
            if let Some(expires) = e.expires_unix {
                obj["expirationDate"] = serde_json::json!(expires);
            }
            obj
        })
        .collect();
    serde_json::Value::Array(array)
}

/// Parse a cookie-export list as `Endpoints.ParseCookies` accepts it
/// (`[{name, value, expirationDate?}]`) back into [`CookieEntry`]s.
/// Used by tests and by the restore path; the gateway itself does the same
/// parse on the `POST /admin/accounts` body.
pub fn parse_cookie_array(value: &serde_json::Value) -> Result<Vec<CookieEntry>, LoginError> {
    let arr = value.as_array().ok_or_else(|| {
        LoginError::InvalidInput("cookies must be an array of {name, value} objects".to_string())
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, entry) in arr.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                LoginError::InvalidInput(format!(
                    "cookies[{i}] must be an object with 'name' and 'value'"
                ))
            })?;
        let value_str = entry
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let expires_unix = entry
            .get("expirationDate")
            .and_then(|v| v.as_f64())
            .map(|d| d as i64);
        out.push(CookieEntry {
            name: name.to_string(),
            value: value_str.to_string(),
            expires_unix,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(entries: &[CookieEntry]) -> Vec<(&str, &str)> {
        entries
            .iter()
            .map(|e| (e.name.as_str(), e.value.as_str()))
            .collect()
    }

    #[test]
    fn missing_required_reports_empty_and_absent() {
        let entries = vec![
            CookieEntry { name: "hatch_sess".into(), value: "".into(), expires_unix: None },
            CookieEntry { name: "other".into(), value: "x".into(), expires_unix: None },
        ];
        assert_eq!(
            missing_required(&pairs(&entries)),
            vec!["hatch_sess", "hatch_native_auth_device"]
        );
        assert!(!has_all_required(&pairs(&entries)));
    }

    #[test]
    fn has_all_required_accepts_both_non_empty() {
        let entries = vec![
            CookieEntry { name: "hatch_sess".into(), value: "s".into(), expires_unix: None },
            CookieEntry {
                name: "hatch_native_auth_device".into(),
                value: "d".into(),
                expires_unix: Some(1_800_000_000),
            },
        ];
        assert!(has_all_required(&pairs(&entries)));
        assert!(missing_required(&pairs(&entries)).is_empty());
    }

    #[test]
    fn filter_keeps_only_login_cookies() {
        let entries = vec![
            CookieEntry { name: "hatch_sess".into(), value: "s".into(), expires_unix: None },
            CookieEntry { name: "hatch_gw".into(), value: "g".into(), expires_unix: None },
            CookieEntry { name: "_ga".into(), value: "x".into(), expires_unix: None },
        ];
        let kept = filter_login_cookies(&entries);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|e| e.name != "_ga"));
    }

    #[test]
    fn cookie_array_json_omits_expiration_for_session_cookies() {
        let entries = vec![
            CookieEntry { name: "hatch_sess".into(), value: "s".into(), expires_unix: None },
            CookieEntry {
                name: "hatch_native_auth_device".into(),
                value: "d".into(),
                expires_unix: Some(1_800_000_000),
            },
        ];
        let json = cookie_array_json(&entries);
        assert_eq!(json[0]["name"], "hatch_sess");
        assert!(json[0].get("expirationDate").is_none());
        assert_eq!(json[1]["expirationDate"], 1_800_000_000);
    }

    #[test]
    fn parse_cookie_array_round_trips() {
        let json = serde_json::json!([
            { "name": "hatch_sess", "value": "s" },
            { "name": "hatch_gw", "value": "g", "expirationDate": 1_800_000_000.5 }
        ]);
        let parsed = parse_cookie_array(&json).unwrap();
        assert_eq!(parsed[0].expires_unix, None);
        assert_eq!(parsed[1].expires_unix, Some(1_800_000_000));
    }

    #[test]
    fn parse_cookie_array_rejects_bad_entry() {
        let json = serde_json::json!([{ "value": "no-name" }]);
        assert!(parse_cookie_array(&json).is_err());
    }
}
