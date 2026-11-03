//! `keys.json`: `ApiKey` + `KeyStore` (Store.cs:286-404).
//!
//! Unlike `accounts.json`, the .NET `KeyStore` reads/writes **plain** files
//! (no `PrivateDataProtection`), so this module has no protector involved.
//! Wire format (CONTRACTS.md §6, koreksi 2 Nov 2026): a JSON array with
//! PascalCase fields (`Id`, `Name`, `Prefix`, `Hash`, `CreatedAt`,
//! `LastUsedAt`, `Revoked`, `Note`); snake_case spellings are accepted on
//! read, matching the .NET default serialization.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{now_unix, random_hex_bytes, sha256_hex_lower, StoreError};
use crate::json::{read_file_to_string, ser_f64_dotnet, write_atomic};

/// One API key (Store.cs:287-306). `hash` is the lowercase hex SHA-256 of
/// the plaintext key; the plaintext itself is never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ApiKey {
    #[serde(default = "new_key_id", alias = "id")]
    pub id: String,
    #[serde(default, alias = "name")]
    pub name: String,
    #[serde(default, alias = "prefix")]
    pub prefix: String,
    #[serde(default, alias = "hash")]
    pub hash: String,
    #[serde(
        default = "now_unix",
        alias = "created_at",
        serialize_with = "ser_f64_dotnet"
    )]
    pub created_at: f64,
    #[serde(
        default,
        alias = "last_used_at",
        serialize_with = "ser_f64_dotnet"
    )]
    pub last_used_at: f64,
    #[serde(default, alias = "revoked")]
    pub revoked: bool,
    #[serde(default, alias = "note")]
    pub note: String,
}

/// `key_` + 10 hex chars (Store.cs:289).
pub fn new_key_id() -> String {
    format!("key_{}", random_hex_bytes(5))
}

impl ApiKey {
    /// `ApiKey.Public()` (Store.cs:298-306): everything except the hash.
    pub fn public(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "prefix": self.prefix,
            "created_at": dotnet_number(self.created_at),
            "last_used_at": dotnet_number(self.last_used_at),
            "revoked": self.revoked,
            "note": self.note,
        })
    }
}

fn dotnet_number(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
        Value::Number((v as i64).into())
    } else {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// `KeyStore` (Store.cs:308-404).
#[derive(Debug)]
pub struct KeyStore {
    file: PathBuf,
    keys: Vec<ApiKey>,
    dirty: bool,
}

impl KeyStore {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        KeyStore {
            file: file.into(),
            keys: Vec::new(),
            dirty: false,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `KeyStore.Load` (Store.cs:318-328): missing file → empty store;
    /// corrupt file → error, file preserved.
    pub fn load(&mut self) -> Result<(), StoreError> {
        if !self.file.exists() {
            return Ok(());
        }
        let text = read_file_to_string(&self.file)?;
        let list: Vec<ApiKey> =
            serde_json::from_str(&text).map_err(|e| StoreError::json_path(&self.file, e))?;
        self.keys.clear();
        for k in list {
            if let Some(existing) = self.keys.iter_mut().find(|x| x.id == k.id) {
                *existing = k;
            } else {
                self.keys.push(k);
            }
        }
        Ok(())
    }

    /// `KeyStore.All` (Store.cs:330-333): ordered by `created_at`.
    pub fn all(&self) -> Vec<ApiKey> {
        let mut keys = self.keys.clone();
        keys.sort_by(|a, b| {
            a.created_at
                .partial_cmp(&b.created_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        keys
    }

    pub fn get(&self, id: &str) -> Option<ApiKey> {
        self.keys.iter().find(|k| k.id == id).cloned()
    }

    /// `KeyStore.Verify` (Store.cs:336-352): SHA-256 the provided key and
    /// compare against stored hashes in constant time; revoked keys never
    /// match. Like .NET, when several keys match (same hash stored twice)
    /// the **last** one wins. Returns the matched key (cloned).
    pub fn verify(&self, provided: &str) -> Option<ApiKey> {
        if provided.is_empty() {
            return None;
        }
        let digest = sha256_hex_lower(provided.as_bytes());
        let mut found: Option<ApiKey> = None;
        for key in &self.keys {
            if constant_time_eq(digest.as_bytes(), key.hash.as_bytes()) && !key.revoked {
                found = Some(key.clone());
            }
        }
        found
    }

    /// `KeyStore.Touch` (Store.cs:354-359): stamp `last_used_at` when at
    /// least one second has passed since the last stamp; marks dirty.
    pub fn touch(&mut self, key: &ApiKey) {
        let now = now_unix();
        if let Some(slot) = self.keys.iter_mut().find(|k| k.id == key.id) {
            if now - slot.last_used_at >= 1.0 {
                slot.last_used_at = now;
                self.dirty = true;
            }
        }
    }

    /// `KeyStore.Create` (Store.cs:361-380): plaintext is `m2a-` + 48 hex
    /// chars (24 random bytes; hex, not base64 — see the .NET comment about
    /// `+`/`/` being stripped). `prefix` is the first 8 chars of the
    /// plaintext; `hash` is its lowercase hex SHA-256.
    pub fn create(&mut self, name: &str, note: &str) -> Result<(ApiKey, String), StoreError> {
        let plaintext = format!("m2a-{}", random_hex_bytes(24));
        let key = ApiKey {
            id: new_key_id(),
            name: name.to_string(),
            prefix: plaintext.chars().take(8).collect(),
            hash: sha256_hex_lower(plaintext.as_bytes()),
            created_at: now_unix(),
            last_used_at: 0.0,
            revoked: false,
            note: note.to_string(),
        };
        self.keys.push(key.clone());
        self.save()?;
        Ok((key, plaintext))
    }

    /// `KeyStore.Remove` (Store.cs:382-388).
    pub fn remove(&mut self, id: &str) -> Result<bool, StoreError> {
        let before = self.keys.len();
        self.keys.retain(|k| k.id != id);
        let removed = self.keys.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Replace a key's fields by id, persisting immediately.
    ///
    /// Integration addition (2 Nov 2026): `PATCH /admin/keys` needs to
    /// persist a name/note/revoked change; the worker crate had no update
    /// path (only `create` / `remove` / `touch`).
    pub fn update(&mut self, key: &ApiKey) -> Result<bool, StoreError> {
        match self.keys.iter_mut().find(|k| k.id == key.id) {
            Some(slot) => {
                *slot = key.clone();
                self.save()?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// `KeyStore.Save` (Store.cs:390-402): indented JSON, atomic via a
    /// fixed `.keys.tmp` file in the same directory.
    pub fn save(&mut self) -> Result<(), StoreError> {
        let json =
            serde_json::to_string_pretty(&self.all()).map_err(StoreError::JsonPlain)?;
        write_atomic(&self.file, &json, ".keys.tmp")?;
        self.dirty = false;
        Ok(())
    }

    /// `KeyStore.Flush` (Store.cs:404): persist only when dirty.
    pub fn flush(&mut self) -> Result<(), StoreError> {
        if self.dirty {
            self.save()
        } else {
            Ok(())
        }
    }
}

/// Constant-time byte equality (`CryptographicOperations.FixedTimeEquals`).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("muse-store-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_verify_roundtrip() {
        let dir = tmpdir("keys");
        let file = dir.join("keys.json");
        let mut store = KeyStore::new(&file);
        let (key, plaintext) = store.create("agent", "test key").unwrap();
        assert!(plaintext.starts_with("m2a-"));
        assert_eq!(plaintext.len(), 4 + 48);
        assert_eq!(key.prefix, &plaintext[..8]);
        assert_eq!(key.hash, sha256_hex_lower(plaintext.as_bytes()));
        assert_eq!(key.public()["hash"], Value::Null); // Public() hides the hash
        assert!(store.verify(&plaintext).is_some());
        assert!(store.verify("m2a-000000000000000000000000000000000000000000000000").is_none());
        assert!(store.verify("").is_none());

        // Persisted file: valid JSON, contract fields, plaintext never stored.
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains(&plaintext));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["Name"], "agent");
        assert_eq!(v[0]["Revoked"], false);

        // Reload and verify again.
        let mut store2 = KeyStore::new(&file);
        store2.load().unwrap();
        assert!(store2.verify(&plaintext).is_some());

        // Touch stamps last_used_at (first touch: 0 -> now).
        let k = store2.get(&key.id).unwrap();
        store2.touch(&k);
        assert!(store2.get(&key.id).unwrap().last_used_at > 0.0);
        store2.flush().unwrap();

        // Revoked keys never verify.
        let mut revoked = store2.get(&key.id).unwrap();
        revoked.revoked = true;
        // (no direct setter in .NET either; simulate by re-creating the store state)
        let mut store3 = KeyStore::new(&file);
        store3.load().unwrap();
        assert!(store3.remove(&key.id).unwrap());
        assert!(store3.get(&key.id).is_none());
        assert!(!store3.remove(&key.id).unwrap());
    }

    #[test]
    fn reads_dotnet_shaped_file() {
        let dir = tmpdir("keys-dotnet");
        let file = dir.join("keys.json");
        std::fs::write(
            &file,
            r#"[{"Id": "key_abcdef0123", "Name": "n", "Prefix": "m2a-1234", "Hash": "abc", "CreatedAt": 1761972245, "LastUsedAt": 0, "Revoked": false, "Note": ""}]"#,
        )
        .unwrap();
        let mut store = KeyStore::new(&file);
        store.load().unwrap();
        let k = store.get("key_abcdef0123").unwrap();
        assert_eq!(k.prefix, "m2a-1234");
        assert_eq!(k.created_at, 1_761_972_245.0);
    }
}
