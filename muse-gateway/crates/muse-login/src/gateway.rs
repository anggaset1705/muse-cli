//! Saving the login to the gateway: `POST /admin/accounts`.
//!
//! Source:
//! * `GatewayClient.AddAccountWithCookies` (Client.cs): POST
//!   `{url}/admin/accounts` with `Authorization: Bearer <adminKey>` and body
//!   `{label, cookies: [{name, value, expirationDate?}], reuse_session: true}`.
//! * `Endpoints.cs` `MapPost("/accounts")`: with `reuse_session` the pool
//!   upserts by session (`UpsertSession`); the response is
//!   `{account: <public account>, missing_cookies: [...]}`.
//! * `LoginForm.RefreshStatus`: the account id is read from
//!   `result?["account"]?["id"]`; a missing id throws
//!   `InvalidOperationException("No saved account returned")`.
//! * `GatewayClient.Request` (Client.cs): non-2xx throws
//!   `HTTP <status>: <first 500 chars of body>`; the client sends no proxy
//!   (`UseProxy = false`) and has a 10-minute timeout.

use crate::cookies::{cookie_array_json, CookieEntry};
use crate::LoginError;

/// Build the exact `POST /admin/accounts` body `AddAccountWithCookies` sends.
pub fn build_add_account_body(label: &str, cookies: &[CookieEntry]) -> serde_json::Value {
    serde_json::json!({
        "label": label,
        "cookies": cookie_array_json(cookies),
        "reuse_session": true,
    })
}

/// Extract the saved account id from a `POST /admin/accounts` response.
///
/// Mirrors `LoginForm.RefreshStatus`: `result?["account"]?["id"]`; a missing
/// id is an error ("No saved account returned").
pub fn parse_account_id(body: &serde_json::Value) -> Result<String, LoginError> {
    body.get("account")
        .and_then(|a| a.get("id"))
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .map(|id| id.to_string())
        .ok_or_else(|| LoginError::Gateway("No saved account returned".to_string()))
}

/// POST the login cookies to the gateway and return the saved account id.
///
/// `gateway_url` is the base URL without a trailing slash, e.g.
/// `http://127.0.0.1:18610`. The admin key is sent as
/// `Authorization: Bearer <admin_key>` (Client.cs `Request`). No proxy is
/// used — the gateway is loopback (Client.cs `UseProxy = false`).
pub fn post_cookies_to_gateway(
    gateway_url: &str,
    admin_key: &str,
    label: &str,
    cookies: &[CookieEntry],
) -> Result<String, LoginError> {
    let url = format!("{}/admin/accounts", gateway_url.trim_end_matches('/'));
    let body = build_add_account_body(label, cookies);
    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {admin_key}"))
        .timeout(std::time::Duration::from_secs(30))
        .send_json(&body)
        .map_err(|err| match err {
            ureq::Error::Status(status, resp) => {
                let text = resp
                    .into_string()
                    .unwrap_or_else(|_| String::new());
                let snippet: String = text.chars().take(500).collect();
                LoginError::Gateway(format!("HTTP {status}: {snippet}"))
            }
            other => LoginError::Gateway(other.to_string()),
        })?;
    let text = response
        .into_string()
        .map_err(|err| LoginError::Gateway(err.to_string()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| LoginError::Gateway(format!("gateway returned invalid JSON: {err}")))?;
    parse_account_id(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_matches_dotnet_shape() {
        let cookies = vec![
            CookieEntry { name: "hatch_sess".into(), value: "s".into(), expires_unix: None },
            CookieEntry {
                name: "hatch_native_auth_device".into(),
                value: "d".into(),
                expires_unix: Some(1_800_000_000),
            },
        ];
        let body = build_add_account_body("Personal account", &cookies);
        assert_eq!(body["label"], "Personal account");
        assert_eq!(body["reuse_session"], true);
        assert_eq!(body["cookies"][0]["name"], "hatch_sess");
        assert_eq!(body["cookies"][1]["expirationDate"], 1_800_000_000);
    }

    #[test]
    fn parse_account_id_reads_nested_id() {
        let body = serde_json::json!({
            "account": { "id": "acc_0123456789", "label": "Personal account" },
            "missing_cookies": [],
        });
        assert_eq!(parse_account_id(&body).unwrap(), "acc_0123456789");
    }

    #[test]
    fn parse_account_id_errors_without_id() {
        assert!(parse_account_id(&serde_json::json!({})).is_err());
        assert!(parse_account_id(&serde_json::json!({ "account": {} })).is_err());
        assert!(parse_account_id(&serde_json::json!({ "account": { "id": "" } })).is_err());
    }
}
