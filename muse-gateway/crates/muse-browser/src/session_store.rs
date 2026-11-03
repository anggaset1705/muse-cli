//! Snapshot state browser per akun — port `BrowserSessionStore.cs` (.NET).
//!
//! File: `<data>/browser-sessions/<SHA256-hex(account.id)>.json`, berisi
//! JSON `{authenticationHash, storageState}`. `storageState` adalah string
//! JSON format Playwright (`{cookies:[...], origins:[...]}`).
//!
//! Perbedaan vs .NET yang disengaja:
//! - .NET membungkus file dengan `PrivateDataProtection` (DPAPI di Windows,
//!   plaintext di non-Windows — lihat `PrivateDataProtection.cs`). Di crate
//!   ini file ditulis/dibaca **plaintext** di semua platform: di Linux itu
//!   persis perilaku .NET; di Windows, file yang ditulis .NET (DPAPI,
//!   prefix `muse-dpapi-v1:`) TIDAK bisa dibaca di sini — `load()` akan
//!   mengembalikan `None` (cache rusak → pulih dari cookie akun, sesuai
//!   komentar .NET: "This is a cache"). Dekripsi DPAPI adalah tanggung jawab
//!   `muse-core` (kontrak §11); saat tersedia, store ini memakainya.
//! - `load()` memvalidasi hash autentikasi + bentuk JSON (array `cookies`
//!   dan `origins`), sama seperti .NET. File rusak → `None`, file
//!   dipertahankan (tidak dihapus).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::types::Account;
use crate::upstream::Upstream;

pub struct BrowserSessionStore {
    data_dir: PathBuf,
}

impl BrowserSessionStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    /// Nama file snapshot untuk akun (`.NET: PathFor`).
    pub fn path_for(&self, account: &Account) -> PathBuf {
        self.data_dir
            .join("browser-sessions")
            .join(format!("{}.json", sha256_hex(account.id.as_bytes())))
    }

    /// Port `BrowserSessionStore.AuthenticationHash`:
    /// `SHA256(join("\n", SessionCookies.map(n => n + "=" + cookies[n])))`,
    /// hex UPPERCASE (`.NET: Convert.ToHexString`).
    pub fn authentication_hash(account: &Account) -> String {
        let joined = Upstream::SESSION_COOKIES
            .iter()
            .map(|name| {
                format!(
                    "{name}={}",
                    account.cookies.get(*name).map(|s| s.as_str()).unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sha256_hex(joined.as_bytes()).to_uppercase()
    }

    /// Port `Load`: `None` bila file tidak ada, hash tidak cocok, bentuk
    /// JSON tidak valid, atau file terenkripsi DPAPI (Windows .NET).
    pub fn load(&self, account: &Account) -> Option<String> {
        let file = self.path_for(account);
        let raw = std::fs::read_to_string(&file).ok()?;
        // File DPAPI .NET (Windows) tidak bisa dibaca di sini — perlakukan
        // sebagai cache rusak (pulih dari cookie akun).
        if raw.trim_start().starts_with("muse-dpapi-v1:") {
            return None;
        }
        let saved: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let hash = saved.get("authenticationHash")?.as_str()?;
        if hash != Self::authentication_hash(account) {
            return None;
        }
        let state = saved.get("storageState")?.as_str()?;
        let parsed: serde_json::Value = serde_json::from_str(state).ok()?;
        let obj = parsed.as_object()?;
        if !matches!(obj.get("cookies"), Some(serde_json::Value::Array(_))) {
            return None;
        }
        if !matches!(obj.get("origins"), Some(serde_json::Value::Array(_))) {
            return None;
        }
        Some(state.to_string())
    }

    /// Port `SaveAsync`: validasi `storageState` adalah objek JSON, lalu
    /// tulis atomik (file tmp + rename, seperti `PrivateDataProtection.WriteFile`).
    pub fn save(&self, account: &Account, storage_state: &str) -> Result<(), String> {
        let parsed: serde_json::Value =
            serde_json::from_str(storage_state).map_err(|e| format!("Invalid browser state: {e}"))?;
        if !parsed.is_object() {
            return Err("Invalid browser state".to_string());
        }
        let file = self.path_for(account);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let saved = serde_json::json!({
            "authenticationHash": Self::authentication_hash(account),
            "storageState": storage_state,
        });
        let text = serde_json::to_string(&saved).map_err(|e| e.to_string())?;
        // Tulis atomik: tmp unik lalu rename (gaya .NET WriteFile).
        let tmp = file.with_extension(format!(
            "tmp-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        match std::fs::rename(&tmp, &file) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e.to_string())
            }
        }
    }

    /// Direktori data (untuk test).
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// SHA-256 hex lowercase.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let out = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn account_with_cookies() -> Account {
        let mut a = Account::new("acc_0123456789");
        a.cookies.insert("hatch_sess".to_string(), "sess-1".to_string());
        a.cookies
            .insert("hatch_native_auth_device".to_string(), "dev-1".to_string());
        a
    }

    #[test]
    fn authentication_hash_matches_dotnet_shape() {
        // SHA256("hatch_sess=sess-1\nhatch_native_auth_device=dev-1"), hex upper.
        let h = BrowserSessionStore::authentication_hash(&account_with_cookies());
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, h.to_uppercase());
        // Cookie berubah -> hash berubah.
        let mut b = account_with_cookies();
        b.cookies.insert("hatch_sess".to_string(), "sess-2".to_string());
        assert_ne!(h, BrowserSessionStore::authentication_hash(&b));
    }

    #[test]
    fn path_uses_sha256_of_id() {
        let store = BrowserSessionStore::new("/tmp/x");
        let p = store.path_for(&account_with_cookies());
        let expect = sha256_hex(b"acc_0123456789");
        assert_eq!(
            p,
            PathBuf::from(format!("/tmp/x/browser-sessions/{expect}.json"))
        );
    }

    #[test]
    fn save_load_roundtrip_and_hash_guard() {
        let dir = std::env::temp_dir().join(format!(
            "muse-browser-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = BrowserSessionStore::new(&dir);
        let a = account_with_cookies();
        let state = r#"{"cookies":[{"name":"hatch_sess","value":"sess-1"}],"origins":[]}"#;
        store.save(&a, state).expect("save");
        assert_eq!(store.load(&a).as_deref(), Some(state));
        // Hash tidak cocok (cookie akun berubah) -> None.
        let mut b = account_with_cookies();
        b.cookies.insert("hatch_sess".to_string(), "other".to_string());
        assert_eq!(store.load(&b), None);
        // Bentuk storage state tidak valid -> None.
        let bad = r#"{"cookies":[],"origins":[]}"#;
        // (bad punya cookies+origins array, jadi valid; uji yang rusak:)
        let broken = r#"{"nope":true}"#;
        store.save(&a, broken).expect("save broken shape");
        assert_eq!(store.load(&a), None);
        let _ = bad;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_is_none() {
        let store = BrowserSessionStore::new("/tmp/definitely-not-here-muse");
        assert_eq!(store.load(&account_with_cookies()), None);
    }

    #[test]
    fn load_dpapi_file_is_none_not_error() {
        let dir = std::env::temp_dir().join(format!(
            "muse-browser-dpapi-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = BrowserSessionStore::new(&dir);
        let a = account_with_cookies();
        let file = store.path_for(&a);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "muse-dpapi-v1:AAAA-encrypted-by-dotnet-on-windows",
        )
        .unwrap();
        assert_eq!(store.load(&a), None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = HashMap::<String, String>::new();
    }
}
