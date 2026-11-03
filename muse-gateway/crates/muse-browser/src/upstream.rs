//! Fakta upstream muse.ai + renew sesi HTTP — port `Upstream.cs` (.NET,
//! dari Python `upstream/muse.py`). Semua yang di sini diamati dari web
//! client publik dan bisa berubah.

use std::collections::HashMap;
use std::time::Duration;

use crate::error::MuseError;
use crate::types::SessionInfo;

/// Port `Upstream` (.NET): konstanta + helper cookie + `RenewSession`.
pub struct Upstream;

impl Upstream {
    pub const SITE: &'static str = "https://muse.ai";
    pub const NEW_THREAD_URL: &'static str = "https://muse.ai/thread/new";
    pub const SESSION_API: &'static str = "https://muse.ai/api/session";
    pub const VM_WAKE_API: &'static str = "https://muse.ai/api/hatch/vm/wake";

    /// Cookie sesi wajib (`.NET: SessionCookies`).
    pub const SESSION_COOKIES: &'static [&'static str] =
        &["hatch_sess", "hatch_native_auth_device"];
    /// Cookie sesi opsional (`.NET: OptionalSessionCookies`).
    pub const OPTIONAL_SESSION_COOKIES: &'static [&'static str] = &["hatch_gw", "hatch_vml"];

    /// User-Agent yang dipakai .NET untuk renew HTTP.
    pub const USER_AGENT: &'static str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

    /// `Upstream.CookieHeader` — `k=v; k=v`, cookie kosong dilewat.
    pub fn cookie_header(cookies: &HashMap<String, String>) -> String {
        cookies
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// `Upstream.MissingSessionCookies` — cookie wajib yang kosong/hilang.
    pub fn missing_session_cookies(cookies: &HashMap<String, String>) -> Vec<String> {
        Self::SESSION_COOKIES
            .iter()
            .filter(|c| cookies.get(**c).map(|v| v.is_empty()).unwrap_or(true))
            .map(|c| c.to_string())
            .collect()
    }

    /// Port `Upstream.RenewSession(cookies, confirmSeconds=20, pollSeconds=2)`.
    ///
    /// Alur .NET (persis):
    /// 1. `GET /api/session` dengan cookie → 401/403 = Auth; non-2xx lain =
    ///    error upstream; gabungkan `Set-Cookie` balasan ke jar.
    /// 2. Bila `vm_id` ada dan `vm_state != "DISABLED"`: `POST
    ///    /api/hatch/vm/wake` `{vm_id, retry_count:0, connect_attempt_id}`,
    ///    lalu poll `/api/session` sampai `vm_state == "RUNNING"` atau
    ///    `confirm_seconds` habis.
    /// 3. `ok = (session_status == "assigned")`.
    ///
    /// Perbedaan: `connect_attempt_id` .NET memakai `Guid.NewGuid()`;
    /// di sini UUID v4 dibuat dari 16 byte acak (format `8-4-4-4-12` hex).
    pub async fn renew_session(
        cookies: &HashMap<String, String>,
        confirm_seconds: f64,
        poll_seconds: f64,
    ) -> Result<SessionInfo, MuseError> {
        let mut jar: HashMap<String, String> = cookies.clone();
        let mut expires: HashMap<String, i64> = HashMap::new();

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(Self::USER_AGENT)
            .build()?;

        let body = Self::get_session(&client, &mut jar, &mut expires).await?;
        let obj = body.as_object().cloned().unwrap_or_default();
        let str_field = |k: &str| -> Option<String> {
            obj.get(k)?.as_str().map(|s| {
                s.chars().take(128).collect::<String>()
            })
        };
        let mut meta: HashMap<String, Option<String>> = HashMap::new();
        meta.insert("session_status".to_string(), str_field("status"));
        meta.insert("vm_id".to_string(), str_field("vm_id"));
        meta.insert("vm_state".to_string(), str_field("vm_state"));

        let vm_id = meta.get("vm_id").cloned().flatten();
        let vm_state = meta.get("vm_state").cloned().flatten();
        if let Some(vm_id) = vm_id {
            if vm_state.as_deref() != Some("DISABLED") {
                // POST /api/hatch/vm/wake (kegagalan HTTP dicatat, tidak fatal —
                // sama seperti .NET yang menangkap HttpRequestException).
                let wake_result: Result<reqwest::Response, reqwest::Error> = client
                    .post(Self::VM_WAKE_API)
                    .header("Cookie", Self::cookie_header(&jar))
                    .header("Origin", Self::SITE)
                    .header("Referer", Self::NEW_THREAD_URL)
                    .json(&serde_json::json!({
                        "vm_id": vm_id,
                        "retry_count": 0,
                        "connect_attempt_id": new_uuid_v4(),
                    }))
                    .send()
                    .await;
                match wake_result {
                    Ok(resp) => {
                        Self::merge_cookies(&resp, &mut jar, &mut expires);
                        meta.insert(
                            "vm_wake_http".to_string(),
                            Some(resp.status().as_u16().to_string()),
                        );
                    }
                    Err(e) => {
                        meta.insert(
                            "vm_wake_http".to_string(),
                            Some(format!("{:?}", e).chars().take(64).collect()),
                        );
                    }
                }
                let after = Self::confirm_running(
                    &client,
                    &mut jar,
                    &mut expires,
                    confirm_seconds,
                    poll_seconds,
                )
                .await;
                meta.insert("vm_state_after".to_string(), after.clone());
                meta.insert(
                    "vm_wake_ok".to_string(),
                    after.map(|s| (s == "RUNNING").to_string()),
                );
            }
        }

        let ok = meta
            .get("session_status")
            .cloned()
            .flatten()
            .as_deref()
            == Some("assigned");
        Ok(SessionInfo {
            ok,
            cookies: jar,
            cookie_expires: expires,
            meta,
        })
    }

    /// Satu `GET /api/session` + merge `Set-Cookie` (dipakai `renew_session`
    /// dan `confirm_running` — dipisah sebagai fn agar lifetime `&mut jar`
    /// tidak tersangkut closure).
    async fn get_session(
        client: &reqwest::Client,
        jar: &mut HashMap<String, String>,
        expires: &mut HashMap<String, i64>,
    ) -> Result<serde_json::Value, MuseError> {
        let resp = client
            .get(Self::SESSION_API)
            .header("Cookie", Self::cookie_header(jar))
            .header("Origin", Self::SITE)
            .header("Referer", Self::NEW_THREAD_URL)
            .header("Accept", "application/json")
            .send()
            .await?;
        Self::merge_cookies(&resp, jar, expires);
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(MuseError::upstream_auth(format!(
                "session rejected by upstream (HTTP {status})"
            )));
        }
        if !resp.status().is_success() {
            return Err(MuseError::internal(format!(
                "session endpoint returned HTTP {status}"
            )));
        }
        let text = resp.text().await?;
        let body: serde_json::Value =
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        Ok(body)
    }

    /// Port `Upstream.ConfirmRunning` — poll `/api/session` sampai
    /// `vm_state == "RUNNING"` atau deadline; kegagalan satu poll hanya
    /// menghabiskan poll itu (seperti .NET: `catch { }`).
    async fn confirm_running(
        client: &reqwest::Client,
        jar: &mut HashMap<String, String>,
        expires: &mut HashMap<String, i64>,
        confirm_seconds: f64,
        poll_seconds: f64,
    ) -> Option<String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(confirm_seconds);
        let mut state: Option<String> = None;
        loop {
            match client
                .get(Self::SESSION_API)
                .header("Cookie", Self::cookie_header(jar))
                .header("Origin", Self::SITE)
                .header("Referer", Self::NEW_THREAD_URL)
                .send()
                .await
            {
                Ok(resp) => {
                    Self::merge_cookies(&resp, jar, expires);
                    if resp.status().is_success() {
                        if let Ok(text) = resp.text().await {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                if let Some(vm) =
                                    v.get("vm_state").and_then(|x| x.as_str())
                                {
                                    state = Some(
                                        vm.chars().take(128).collect::<String>(),
                                    );
                                    if state.as_deref() == Some("RUNNING") {
                                        return state;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(_) => { /* satu poll gagal hanya menghabiskan poll itu */ }
            }
            if tokio::time::Instant::now() + Duration::from_secs_f64(poll_seconds) > deadline {
                return state;
            }
            tokio::time::sleep(Duration::from_secs_f64(poll_seconds)).await;
        }
    }

    /// Port `Upstream.MergeCookies` — gabungkan `Set-Cookie` balasan ke jar
    /// + catat `Expires=` (RFC1123) per nama cookie.
    fn merge_cookies(
        resp: &reqwest::Response,
        jar: &mut HashMap<String, String>,
        expires: &mut HashMap<String, i64>,
    ) {
        for header in resp.headers().get_all(reqwest::header::SET_COOKIE).iter() {
            let Ok(header) = header.to_str() else {
                continue;
            };
            let mut parts = header.split(';');
            let Some(first) = parts.next() else {
                continue;
            };
            let Some(eq) = first.find('=') else {
                continue;
            };
            let name = first[..eq].trim().to_string();
            let value = first[eq + 1..].trim().to_string();
            if name.is_empty() {
                continue;
            }
            if !value.is_empty() {
                jar.insert(name.clone(), value);
            }
            for part in parts {
                let p = part.trim();
                let rest = p
                    .strip_prefix("Expires=")
                    .or_else(|| p.strip_prefix("expires="));
                if let Some(rest) = rest {
                    if let Ok(dt) = httpdate::parse_http_date(rest) {
                        if let Ok(dur) = dt.duration_since(std::time::UNIX_EPOCH) {
                            expires.insert(name.clone(), dur.as_secs() as i64);
                        }
                    }
                }
            }
        }
    }
}

/// UUID v4 acak (format `8-4-4-4-12` hex lowercase) — pengganti
/// `Guid.NewGuid().ToString()` .NET untuk `connect_attempt_id`.
/// Dibuat dari `getrandom` via `std` (tanpa dep `uuid`).
pub fn new_uuid_v4() -> String {
    let mut b = [0u8; 16];
    fill_random(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    )
}

fn fill_random(buf: &mut [u8]) {
    // /dev/urandom tersedia di Linux; fallback: campuran waktu + pid.
    // (Tidak memakai dep `rand`/`getrandom` demi build mandiri yang ringan.)
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(buf).is_ok() {
            return;
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut x = (now as u64) ^ ((std::process::id() as u64) << 32) ^ 0x9E3779B97F4A7C15;
    for byte in buf.iter_mut() {
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *byte = (x.wrapping_mul(0x2545F4914F6CDD1D) >> 56) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_header_skips_empty() {
        let mut m = HashMap::new();
        m.insert("hatch_sess".to_string(), "abc".to_string());
        m.insert("hatch_gw".to_string(), "".to_string());
        assert_eq!(Upstream::cookie_header(&m), "hatch_sess=abc");
    }

    #[test]
    fn missing_session_cookies_detected() {
        let mut m = HashMap::new();
        m.insert("hatch_sess".to_string(), "abc".to_string());
        let missing = Upstream::missing_session_cookies(&m);
        assert_eq!(missing, vec!["hatch_native_auth_device".to_string()]);
        let empty = Upstream::missing_session_cookies(&HashMap::new());
        assert_eq!(empty.len(), 2);
    }

    #[test]
    fn uuid_v4_format() {
        let u = new_uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(u.chars().nth(8), Some('-'));
        assert_eq!(u.chars().nth(14), Some('4'));
        assert!(u.chars().nth(19).map(|c| "89ab".contains(c)).unwrap_or(false));
    }
}
