//! `accounts.json`: `Account` + `AccountPool` (Store.cs:1-284).
//!
//! Wire format (CONTRACTS.md §6, koreksi 2 Nov 2026): a JSON array of
//! accounts with PascalCase fields and `Status` as a NUMBER
//! (0 = Active, 1 = Cooling, 2 = Invalid); `id` is `acc_` + 10 hex chars.
//! Snake_case spellings and string statuses are still accepted on read.
//!
//! `AccountPool.Persist` (Store.cs:110-119) serializes with default
//! `JsonSerializerOptions`, i.e. PascalCase property names and the enum as
//! a number (`"Id"`, `"Status": 0`). This crate writes that form and
//! *reads* both: every field has a snake_case serde alias and
//! `AccountStatus` accepts `"active"`/`"Active"` and `0` alike, so files
//! written by older Rust (snake_case) still load. See `json.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::error::{now_unix, random_hex_bytes, sha256_hex_upper, StoreError};
use crate::json::{read_file_to_string, ser_f64_dotnet, write_atomic};
use crate::protect::{PlaintextProtector, Protector};
use crate::quota::{classify as classify_quota, QuotaPhase};

/// Session cookies whose presence defines an authenticated account
/// (`Upstream.SessionCookies`, Upstream.cs:13).
pub const SESSION_COOKIES: [&str; 2] = ["hatch_sess", "hatch_native_auth_device"];

/// `AccountStatus` (Store.cs:9). Serializes as the lowercase string
/// (`"active"`); deserializes from the string (any case) or the .NET
/// numeric value (0 = Active, 1 = Cooling, 2 = Invalid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountStatus {
    Active,
    Cooling,
    Invalid,
}

impl Default for AccountStatus {
    fn default() -> Self {
        AccountStatus::Active
    }
}

impl<'de> Deserialize<'de> for AccountStatus {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = AccountStatus;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("`active`/`cooling`/`invalid` or 0/1/2")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<AccountStatus, E> {
                match v.to_ascii_lowercase().as_str() {
                    "active" => Ok(AccountStatus::Active),
                    "cooling" => Ok(AccountStatus::Cooling),
                    "invalid" => Ok(AccountStatus::Invalid),
                    other => Err(E::custom(format!("unknown account status {other:?}"))),
                }
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<AccountStatus, E> {
                match v {
                    0 => Ok(AccountStatus::Active),
                    1 => Ok(AccountStatus::Cooling),
                    2 => Ok(AccountStatus::Invalid),
                    other => Err(E::custom(format!("unknown account status {other}"))),
                }
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<AccountStatus, E> {
                self.visit_i64(v as i64)
            }
        }
        d.deserialize_any(V)
    }
}

impl Serialize for AccountStatus {
    /// .NET writes the enum as a NUMBER (`"Status": 0`); see CONTRACTS.md §6.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(match self {
            AccountStatus::Active => 0,
            AccountStatus::Cooling => 1,
            AccountStatus::Invalid => 2,
        })
    }
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountStatus::Active => "active",
            AccountStatus::Cooling => "cooling",
            AccountStatus::Invalid => "invalid",
        }
    }
}

/// One Muse account (Store.cs:11-70).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Account {
    #[serde(default = "new_account_id", alias = "id")]
    pub id: String,
    #[serde(default, alias = "label")]
    pub label: String,
    #[serde(default = "default_true", alias = "enabled")]
    pub enabled: bool,
    #[serde(default, alias = "status")]
    pub status: AccountStatus,
    #[serde(default, alias = "cookies")]
    pub cookies: Map<String, Value>,
    #[serde(default, alias = "cookie_expires")]
    pub cookie_expires: Map<String, Value>,
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
    #[serde(
        default,
        alias = "cooldown_until",
        serialize_with = "ser_f64_dotnet"
    )]
    pub cooldown_until: f64,
    #[serde(default, alias = "last_error")]
    pub last_error: Option<String>,
    #[serde(default, alias = "success_count")]
    pub success_count: i64,
    #[serde(default, alias = "fail_count")]
    pub fail_count: i64,
    #[serde(default, alias = "meta")]
    pub meta: Map<String, Value>,
}

fn default_true() -> bool {
    true
}

/// `acc_` + 10 hex chars (Store.cs:13: `"acc_" + Guid.NewGuid().ToString("N")[..10]`).
pub fn new_account_id() -> String {
    format!("acc_{}", random_hex_bytes(5))
}

impl Default for Account {
    fn default() -> Self {
        Account {
            id: new_account_id(),
            label: String::new(),
            enabled: true,
            status: AccountStatus::Active,
            cookies: Map::new(),
            cookie_expires: Map::new(),
            created_at: now_unix(),
            last_used_at: 0.0,
            cooldown_until: 0.0,
            last_error: None,
            success_count: 0,
            fail_count: 0,
            meta: Map::new(),
        }
    }
}

impl Account {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Account.IsAvailable` (Store.cs:39-44).
    pub fn is_available(&self, now: f64) -> bool {
        if !self.enabled || self.status == AccountStatus::Invalid {
            return false;
        }
        if self.status == AccountStatus::Cooling {
            return now >= self.cooldown_until;
        }
        true
    }

    /// Cookie value as a plain string, if present and a JSON string.
    pub fn cookie(&self, name: &str) -> String {
        self.cookies
            .get(name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// Mask one cookie value for public output (Store.cs `Public()`):
    /// `***` when ≤ 8 chars, else `aaaa...zzzz` (first/last 4 chars).
    pub fn mask_cookie(value: &str) -> String {
        let chars: Vec<char> = value.chars().collect();
        if chars.len() <= 8 {
            "***".to_string()
        } else {
            let first: String = chars.iter().take(4).collect();
            let last: String = chars.iter().skip(chars.len() - 4).collect();
            format!("{first}...{last}")
        }
    }

    /// `Account.Public()` (Store.cs:46-69): the account as JSON with cookie
    /// values masked; `cookie_expires` and `meta` are passed through in full.
    pub fn public(&self) -> Value {
        let mut masked = Map::new();
        for (k, v) in &self.cookies {
            let s = v.as_str().unwrap_or("");
            masked.insert(k.clone(), Value::String(Self::mask_cookie(s)));
        }
        let mut obj = Map::new();
        obj.insert("id".into(), Value::String(self.id.clone()));
        obj.insert("label".into(), Value::String(self.label.clone()));
        obj.insert("enabled".into(), Value::Bool(self.enabled));
        obj.insert(
            "status".into(),
            Value::String(self.status.as_str().to_string()),
        );
        obj.insert("cookies".into(), Value::Object(masked));
        obj.insert(
            "cookie_expires".into(),
            Value::Object(self.cookie_expires.clone()),
        );
        obj.insert(
            "created_at".into(),
            dotnet_number(self.created_at),
        );
        obj.insert(
            "last_used_at".into(),
            dotnet_number(self.last_used_at),
        );
        obj.insert(
            "cooldown_until".into(),
            dotnet_number(self.cooldown_until),
        );
        obj.insert(
            "last_error".into(),
            self.last_error
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        obj.insert(
            "success_count".into(),
            Value::Number(self.success_count.into()),
        );
        obj.insert("fail_count".into(), Value::Number(self.fail_count.into()));
        obj.insert("meta".into(), Value::Object(self.meta.clone()));
        Value::Object(obj)
    }
}

/// Render an `f64` timestamp the way `System.Text.Json` writes a `double`:
/// integral values without a decimal point.
fn dotnet_number(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
        Value::Number((v as i64).into())
    } else {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// `BrowserSessionStore.AuthenticationHash` (BrowserSessionStore.cs:14-16):
/// UPPERCASE hex SHA-256 of `"hatch_sess=<v>\nhatch_native_auth_device=<v>"`,
/// missing cookies counting as empty strings.
pub fn authentication_hash(account: &Account) -> String {
    let joined = SESSION_COOKIES
        .iter()
        .map(|name| format!("{name}={}", account.cookie(name)))
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex_upper(joined.as_bytes())
}

/// Names of the session cookies that are missing or empty
/// (`Upstream.MissingSessionCookies`, Upstream.cs:23-24).
pub fn missing_session_cookies(cookies: &Map<String, Value>) -> Vec<String> {
    SESSION_COOKIES
        .iter()
        .filter(|name| {
            cookies
                .get(**name)
                .and_then(|v| v.as_str())
                .map(|s| s.is_empty())
                .unwrap_or(true)
        })
        .map(|s| s.to_string())
        .collect()
}

/// `AccountPool`'s storage half: load/save `accounts.json` plus the
/// single-account mutations. The scheduling half (acquire/release) lives in
/// [`AccountPool`] below.
pub struct AccountStore {
    file: PathBuf,
    accounts: Vec<Account>,
    protector: Arc<dyn Protector>,
    /// Cacat A (3 Nov 2026): `load()` gagal (file corrupt / DPAPI /
    /// I/O) sementara `persist()` tetap menulis — pool yang start kosong
    /// lalu menimpa `accounts.json` berisi 1 akun saja dan akun lama
    /// HILANG diam-diam (bukti: gateway 11:51:01 membuat ID baru padahal
    /// file berisi akun lama). Bila `true`, `persist()` MENOLAK menulis
    /// apa pun sampai file berhasil dimuat lagi; `load()` yang gagal
    /// selalu menyalin file asli ke `accounts.json.corrupt-<ts>-<pid>`
    /// dulu, jadi tidak ada data yang pernah tertimpa.
    load_failed: bool,
    /// Salinan darurat dari `load()` terakhir yang gagal (None bila
    /// tidak ada / load terakhir berhasil).
    load_failure_backup: Option<PathBuf>,
    /// Cacat B (3 Nov 2026): guard lama hanya menahan `persist()` saat
    /// `load()` GAGAL — tapi akun hilang lagi padahal load SUKSES dengan
    /// 0 akun (login.log 12:27:07: `ok accounts=0` sementara gateway
    /// membaca 1 dari file yang sama 2 menit sebelumnya). Jumlah akun
    /// yang terakhir diketahui BENAR (saat `load()` sukses terakhir /
    /// `persist()` sukses terakhir). `persist()` MENOLAK menulis bila
    /// jumlah baru LEBIH KECIL tanpa delete eksplisit — penyusutan
    /// diam-diam (N→0 atau N→lebih sedikit) tidak pernah boleh terjadi
    /// lewat upsert / release / quota / start / normalisasi. None =
    /// store belum pernah load/persist (fresh, belum ada patokan).
    known_count: Option<usize>,
    /// Satu-tembakan untuk `remove()`: satu-satunya jalur sah yang
    /// mengecilkan store. Disetel sebelum `persist()` di `remove()`,
    /// dibersihkan lagi sesudahnya (juga bila persist gagal).
    allow_shrink: bool,
}

impl AccountStore {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        AccountStore {
            file: file.into(),
            accounts: Vec::new(),
            protector: Arc::new(PlaintextProtector),
            load_failed: false,
            load_failure_backup: None,
            known_count: None,
            allow_shrink: false,
        }
    }

    /// Same store, but file contents go through `protector` (a DPAPI
    /// protector from `muse-core` on Windows; plaintext on Linux).
    pub fn with_protector(file: impl Into<PathBuf>, protector: Arc<dyn Protector>) -> Self {
        AccountStore {
            file: file.into(),
            accounts: Vec::new(),
            protector,
            load_failed: false,
            load_failure_backup: None,
            known_count: None,
            allow_shrink: false,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `true` bila `load()` terakhir gagal padahal file ADA — artinya isi
    /// di memori (kosong / lama) TIDAK mencerminkan file di disk, dan
    /// `persist()` wajib menolak menulis sampai load berhasil lagi.
    pub fn load_failed(&self) -> bool {
        self.load_failed
    }

    /// Salinan darurat file asli dari `load()` terakhir yang gagal.
    pub fn load_failure_backup(&self) -> Option<&Path> {
        self.load_failure_backup.as_deref()
    }

    /// Salin `accounts.json` yang gagal dimuat ke
    /// `accounts.json.corrupt-<unix>-<pid>` di direktori yang sama
    /// (best effort — kegagalan menyalin tidak menutupi error aslinya).
    fn backup_corrupt_file(&mut self) {
        let name = self
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "accounts.json".to_string());
        let backup = self.file.with_file_name(format!(
            "{name}.corrupt-{}-{}",
            now_unix() as i64,
            std::process::id()
        ));
        if std::fs::copy(&self.file, &backup).is_ok() {
            self.load_failure_backup = Some(backup);
        }
    }

    /// Salin isi file SAAT INI ke `accounts.json.shrink-<unix>-<pid>`
    /// sebelum `persist()` yang ditolak — bukti forensik isi file yang
    /// nyaris tertimpa oleh store yang mengecil (best effort).
    /// Mengembalikan path backup bila penyalinan berhasil.
    fn backup_shrink_file(&self) -> Option<PathBuf> {
        let name = self
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "accounts.json".to_string());
        let backup = self.file.with_file_name(format!(
            "{name}.shrink-{}-{}",
            now_unix() as i64,
            std::process::id()
        ));
        if std::fs::copy(&self.file, &backup).is_ok() {
            Some(backup)
        } else {
            None
        }
    }

    /// Satu baris audit ke `<data_dir>/gateway.log` (file yang sama
    /// dipakai gateway; best effort, tidak pernah menggagalkan caller).
    /// Format: `[store <unix>] <line>`. Dipakai `persist()` (sukses +
    /// penolakan) supaya setiap tulis store — termasuk tulis yang
    /// ditolak — punya jejak jumlah sebelum/sesudah di log.
    fn audit_log(&self, line: &str) {
        use std::io::Write;
        let Some(dir) = self.file.parent() else {
            return;
        };
        let path = dir.join("gateway.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "[store {}] {line}", now_unix() as i64);
        }
    }

    /// Jumlah akun yang terakhir diketahui benar (None = belum ada
    /// patokan load/persist). Dipakai diagnosis, bukan rahasia.
    pub fn known_count(&self) -> Option<usize> {
        self.known_count
    }

    /// `AccountPool.Load` (Store.cs:103-115): a missing file means an empty
    /// store; a corrupt file is an error and the file is left untouched.
    ///
    /// Cacat A (3 Nov 2026): a failed load MUST back the file up first
    /// and mark the store, so a later `persist()` can never silently
    /// overwrite the unreadable file with an (empty) in-memory pool.
    pub fn load(&mut self) -> Result<(), StoreError> {
        if !self.file.exists() {
            self.load_failed = false;
            // Tidak ada file = store kosong yang sah; patokan 0 supaya
            // persist berikutnya yang mengecil dari angka ini ketahuan.
            self.known_count = Some(0);
            return Ok(());
        }
        let result = (|| -> Result<Vec<Account>, StoreError> {
            let raw = read_file_to_string(&self.file)?;
            let text = self.protector.unprotect(&raw)?;
            let list: Vec<Account> =
                serde_json::from_str(&text).map_err(|e| StoreError::json_path(&self.file, e))?;
            Ok(list)
        })();
        match result {
            Ok(list) => {
                self.load_failed = false;
                self.accounts.clear();
                for a in list {
                    if let Some(existing) = self.accounts.iter_mut().find(|x| x.id == a.id) {
                        *existing = a;
                    } else {
                        self.accounts.push(a);
                    }
                }
                self.known_count = Some(self.accounts.len());
                Ok(())
            }
            Err(e) => {
                self.backup_corrupt_file();
                self.load_failed = true;
                Err(e)
            }
        }
    }

    /// `AccountPool.Persist` (Store.cs:117-125): indented JSON, written
    /// atomically (temp file + rename). The temp name mirrors
    /// `PrivateDataProtection.WriteFile`: `<path>.muse-protect-<32hex>.tmp`.
    ///
    /// Cacat A guard: after a failed `load()` this REFUSES to write —
    /// persisting an empty / stale pool over a file that could not be
    /// read is exactly how the old account was wiped on 3 Nov 2026.
    ///
    /// Cacat B guard (3 Nov 2026): a successful load of 0 accounts is
    /// NOT proof the store is empty — the 12:27:07 login read `ok
    /// accounts=0` from the same file the gateway had loaded as 1 two
    /// minutes earlier. So `persist()` ALSO refuses any SHRINK
    /// (new count < `known_count`) unless `remove()` explicitly allowed
    /// it for this one write: the shrinking write is refused, the
    /// current file is backed up to `accounts.json.shrink-*`, and the
    /// refusal (with before/after counts) is logged to `gateway.log`.
    /// Every successful persist updates `known_count` and logs one
    /// audit line, so `gateway.log` shows every store write.
    pub fn persist(&mut self) -> Result<(), StoreError> {
        if self.load_failed {
            return Err(StoreError::invalid(format!(
                "refusing to persist {}: it failed to load (see {} backup); fix or remove the file first",
                self.file.display(),
                self.load_failure_backup
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "accounts.json.corrupt-*".to_string())
            )));
        }
        if let Some(known) = self.known_count {
            let now = self.accounts.len();
            if now < known && !self.allow_shrink {
                let backup = self.backup_shrink_file();
                let msg = format!(
                    "refusing to persist {}: would shrink accounts {}->{} without an explicit delete (backup: {}); \
                     the in-memory pool does not match the file — fix the writer, do not retry blindly",
                    self.file.display(),
                    known,
                    now,
                    backup
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
                self.audit_log(&format!("persist-refused: {msg}"));
                return Err(StoreError::invalid(msg));
            }
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::io_path(dir, e))?;
        }
        let json = serde_json::to_string_pretty(&self.accounts)
            .map_err(StoreError::JsonPlain)?;
        let protected = self.protector.protect(&json)?;
        let tmp_name = format!(".muse-protect-{}.tmp", crate::error::new_guid_n());
        let before = self.known_count;
        write_atomic(&self.file, &protected, &tmp_name)?;
        self.known_count = Some(self.accounts.len());
        self.audit_log(&format!(
            "persist: accounts {}->{} file={}",
            before.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string()),
            self.accounts.len(),
            self.file.display()
        ));
        Ok(())
    }

    pub fn all(&self) -> Vec<Account> {
        self.accounts.clone()
    }

    pub fn get(&self, id: &str) -> Option<Account> {
        self.accounts.iter().find(|a| a.id == id).cloned()
    }

    /// `AccountPool.Upsert` (Store.cs:130-135).
    pub fn upsert(&mut self, acc: Account) -> Result<Account, StoreError> {
        if let Some(existing) = self.accounts.iter_mut().find(|x| x.id == acc.id) {
            *existing = acc.clone();
        } else {
            self.accounts.push(acc.clone());
        }
        self.persist()?;
        Ok(acc)
    }

    /// `AccountPool.UpsertSession` (Store.cs:137-155): when the incoming
    /// account carries all session cookies and an account with the same
    /// authentication hash already exists, refresh that account in place
    /// (cookies, expiries, label, enabled; an `Invalid` account becomes
    /// `Active` with a cleared error) instead of adding a duplicate.
    ///
    /// Rust addition (3 Nov 2026, not in .NET): the site rotates
    /// `hatch_sess` on every fresh interactive login, so the hash changes
    /// and the hash match above cannot fire — that is how one account got
    /// saved 5 times (2 Nov 2026: five `Personal` entries, one per login
    /// attempt, each with a fresh `hatch_sess`). As a second step, match
    /// an existing account with the same label AND the same
    /// `hatch_native_auth_device` (the browser-profile device id, stable
    /// across re-logins in one profile while `hatch_sess` rotates) and
    /// refresh that one in place instead. A genuinely different account
    /// must use the login window's "other account" flow, which clears all
    /// cookies and mints a fresh device id, so it still appends as new.
    pub fn upsert_session(&mut self, incoming: Account) -> Result<Account, StoreError> {
        let existing_id = if missing_session_cookies(&incoming.cookies).is_empty() {
            let hash = authentication_hash(&incoming);
            self.accounts
                .iter()
                .find(|a| authentication_hash(a) == hash)
                .map(|a| a.id.clone())
                .or_else(|| self.find_same_profile(&incoming).map(|a| a.id.clone()))
        } else {
            None
        };
        if let Some(id) = existing_id {
            let mut updated = self.get(&id).unwrap();
            updated.cookies = incoming.cookies;
            updated.cookie_expires = incoming.cookie_expires;
            updated.label = incoming.label;
            updated.enabled = incoming.enabled;
            if updated.status == AccountStatus::Invalid {
                updated.status = AccountStatus::Active;
                updated.last_error = None;
            }
            return self.upsert(updated);
        }
        self.upsert(incoming)
    }

    /// Same label + same `hatch_native_auth_device`: a re-login of the
    /// same account in the same browser profile (the site keeps the
    /// device id while rotating `hatch_sess`). Returns the most recently
    /// used such account, if any. Empty device id never matches.
    fn find_same_profile(&self, incoming: &Account) -> Option<Account> {
        let device = incoming.cookie("hatch_native_auth_device");
        if device.is_empty() {
            return None;
        }
        self.accounts
            .iter()
            .filter(|a| {
                a.label == incoming.label
                    && a.cookie("hatch_native_auth_device") == device
            })
            .max_by(|a, b| {
                a.last_used_at
                    .partial_cmp(&b.last_used_at)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    /// `AccountPool.Remove` (Store.cs:157-163).
    ///
    /// Satu-satunya jalur sah yang mengecilkan store: mengibarkan
    /// `allow_shrink` satu-tembakan untuk `persist()` berikut, lalu
    /// menurunkannya lagi (juga bila persist gagal) supaya tulis
    /// berikutnya tetap dijaga guard anti-penyusutan.
    pub fn remove(&mut self, id: &str) -> Result<bool, StoreError> {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        let removed = self.accounts.len() != before;
        if removed {
            self.allow_shrink = true;
            let r = self.persist();
            self.allow_shrink = false;
            r?;
        }
        Ok(removed)
    }
}

/// Outcome of a finished lease, mirroring the `error` argument of
/// `AccountPool.Release` (Store.cs:228-266).
#[derive(Debug, Clone)]
pub enum ReleaseOutcome {
    /// No error: success count +1, error cleared, `Cooling` -> `Active`.
    Success,
    /// The lease was cancelled (`OperationCanceledException`): counts and
    /// status are untouched, only the in-flight slot is freed.
    Cancelled,
    /// A failure. `code` is the `MuseException`/`UpstreamException` code
    /// (`"upstream_auth_failed"`, `"upstream_quota_exhausted"`, …);
    /// `retryable` is `UpstreamException.Retryable`.
    Failure {
        message: String,
        code: Option<String>,
        retryable: bool,
    },
}

/// `AccountPool` (Store.cs:76-283): scheduling, concurrency limits, cooldown
/// and failover state over an [`AccountStore`].
///
/// Not ported: `AcquireAsync`'s timed wait loop (Store.cs:183-201) — that
/// needs an async runtime and cancellation token, so it belongs to the
/// gateway built on this crate, which can poll [`AccountPool::acquire`]
/// until [`AccountPool::acquire_timeout`] elapses. The `Lease` RAII guard is
/// likewise a gateway concern: `acquire` returns the chosen account and the
/// caller must call [`AccountPool::release`].
pub struct AccountPool {
    pub store: AccountStore,
    pub strategy: String,
    pub max_concurrency: usize,
    pub cooldown: f64,
    pub acquire_timeout: f64,
    inflight: HashMap<String, usize>,
    affinity: HashMap<String, String>,
    round_robin: i64,
}

impl AccountPool {
    pub fn new(
        file: impl Into<PathBuf>,
        strategy: &str,
        max_concurrency: usize,
        cooldown: f64,
        acquire_timeout: f64,
    ) -> Self {
        AccountPool {
            store: AccountStore::new(file),
            strategy: strategy.to_string(),
            max_concurrency: max_concurrency.max(1),
            cooldown,
            acquire_timeout,
            inflight: HashMap::new(),
            affinity: HashMap::new(),
            round_robin: 0,
        }
    }

    pub fn load(&mut self) -> Result<(), StoreError> {
        self.store.load()
    }

    pub fn persist(&mut self) -> Result<(), StoreError> {
        self.store.persist()
    }

    /// `AccountPool.Stats` (Store.cs:165-175), plus waterfall counts
    /// (`weekly` / `extra` / `exhausted`, dari snapshot kuota terakhir —
    /// untuk diagnostik `/readyz`).
    pub fn stats(&self) -> HashMap<String, usize> {
        let now = now_unix();
        let accounts = self.store.all();
        let mut out = HashMap::new();
        out.insert("total".to_string(), accounts.len());
        out.insert(
            "available".to_string(),
            accounts.iter().filter(|a| a.is_available(now)).count(),
        );
        out.insert(
            "invalid".to_string(),
            accounts
                .iter()
                .filter(|a| a.status == AccountStatus::Invalid)
                .count(),
        );
        out.insert("inflight".to_string(), self.inflight.values().sum());
        let (weekly, extra, exhausted) = self.waterfall_counts(now);
        out.insert("weekly".to_string(), weekly);
        out.insert("extra".to_string(), extra);
        out.insert("exhausted".to_string(), exhausted);
        out
    }

    /// Klasifikasi satu akun ke fase waterfall kuota dari snapshot kuota
    /// terakhirnya (`meta["quota"]`, ditulis `GET /admin/accounts/{id}/quota`).
    pub fn quota_phase(&self, account: &Account, now: f64) -> QuotaPhase {
        classify_quota(crate::quota::QuotaSnapshot::from_meta(&account.meta).as_ref(), now)
    }

    /// (weekly, extra, exhausted) atas akun yang enabled + bukan Invalid.
    pub fn waterfall_counts(&self, now: f64) -> (usize, usize, usize) {
        let accounts = self.store.all();
        let mut weekly = 0;
        let mut extra = 0;
        let mut exhausted = 0;
        for a in accounts
            .iter()
            .filter(|a| a.enabled && a.status != AccountStatus::Invalid)
        {
            match self.quota_phase(a, now) {
                QuotaPhase::Weekly => weekly += 1,
                QuotaPhase::Extra => extra += 1,
                QuotaPhase::Exhausted => exhausted += 1,
            }
        }
        (weekly, extra, exhausted)
    }

    /// `Some(detail)` bila pool memegang >= 1 akun enabled + bukan Invalid
    /// TAPI semuanya waterfall-exhausted (mingguan habis, extra 0) — alias
    /// pool out of credit. `None` bila masih ada akun fase mingguan/extra,
    /// atau bila tidak ada akun yang bisa dipakai sama sekali (yang terakhir
    /// ini tetap jalur "no usable account" biasa, bukan out of credit).
    pub fn out_of_credit(&self, now: f64) -> Option<String> {
        let accounts = self.store.all();
        let usable: Vec<&Account> = accounts
            .iter()
            .filter(|a| a.enabled && a.status != AccountStatus::Invalid)
            .collect();
        if usable.is_empty() {
            return None;
        }
        if usable
            .iter()
            .any(|a| self.quota_phase(a, now) != QuotaPhase::Exhausted)
        {
            return None;
        }
        let mut parts = Vec::new();
        for a in &usable {
            let snap = crate::quota::QuotaSnapshot::from_meta(&a.meta);
            let pct = snap
                .as_ref()
                .and_then(|s| s.weekly_used_pct)
                .map(|p| format!("{p}%"))
                .unwrap_or_else(|| "?".to_string());
            let resets = snap
                .as_ref()
                .and_then(|s| s.resets_at.clone())
                .unwrap_or_else(|| "?".to_string());
            let extra = snap
                .as_ref()
                .and_then(|s| s.extra_left.clone())
                .unwrap_or_else(|| "0".to_string());
            parts.push(format!(
                "{}: weekly {} (resets {}), extra {}",
                a.label, pct, resets, extra
            ));
        }
        Some(format!(
            "out of credit: all {} account(s) exhausted their weekly quota and extra tokens ({})",
            usable.len(),
            parts.join("; ")
        ))
    }

    /// Error saat `acquire` tidak menemukan akun yang bisa melayani:
    /// terminal out-of-credit bila semua akun waterfall-exhausted, kalau
    /// tidak transient "no usable account" (cooling / in-flight — wait loop
    /// gateway tetap menunggu yang itu).
    fn no_candidate_error(&self, now: f64) -> StoreError {
        if let Some(detail) = self.out_of_credit(now) {
            return StoreError::out_of_credit(detail);
        }
        StoreError::invalid("no usable account in pool")
    }

    fn kind_blocked(account: &Account, kind: Option<&str>, now: f64) -> bool {
        let Some(kind) = kind else { return false };
        let Some(Value::Object(blocks)) = account.meta.get("kind_blocked_until") else {
            return false;
        };
        match blocks.get(kind) {
            Some(Value::Number(n)) => n.as_f64().map(|until| until > now).unwrap_or(false),
            _ => false,
        }
    }

    fn candidates(&self, exclude: &[String], kind: Option<&str>, now: f64) -> Vec<Account> {
        self.store
            .all()
            .into_iter()
            .filter(|a| {
                !exclude.iter().any(|e| e == &a.id)
                    && a.enabled
                    && a.status != AccountStatus::Invalid
                    && a.is_available(now)
                    && !Self::kind_blocked(a, kind, now)
                    && self.inflight.get(&a.id).copied().unwrap_or(0) < self.max_concurrency
            })
            .collect()
    }

    /// `AccountPool.HasAlternative` (Store.cs:203-208).
    pub fn has_alternative(&self, exclude: &[String]) -> bool {
        self.store.all().iter().any(|a| {
            !exclude.iter().any(|e| e == &a.id)
                && a.enabled
                && a.status != AccountStatus::Invalid
        })
    }

    /// `AccountPool.Acquire` (Store.cs:210-226): pick a free account by
    /// strategy (`affinity` default, `lru`, `round_robin`), mark it
    /// in-flight and stamp `last_used_at`. Errors with `NoAccount` when no
    /// usable account exists.
    ///
    /// Quota waterfall (3 Nov 2026): kandidat dipartisi dulu menurut fase
    /// kuota — fase 1 (mingguan tersisa) selalu didahulukan; baru bila tidak
    /// ada, fase 2 (extra token, urutan store = "akun 1 extra, habis, baru
    /// akun 2 extra"); bila tidak ada keduanya, error out-of-credit yang
    /// jelas (bila semua akun waterfall-exhausted) atau "no usable account"
    /// biasa (transient: cooling / in-flight). Fase mengalahkan strategi:
    /// akun yang di-pin affinity tapi mingguan-nya habis TIDAK dipilih
    /// selama ada akun lain yang mingguan-nya tersisa.
    pub fn acquire(
        &mut self,
        exclude: &[String],
        hint: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Account, StoreError> {
        let now = now_unix();
        let free = self.candidates(exclude, kind, now);
        if free.is_empty() {
            return Err(self.no_candidate_error(now));
        }
        let mut weekly = Vec::new();
        let mut extra = Vec::new();
        for acc in free {
            match self.quota_phase(&acc, now) {
                QuotaPhase::Weekly => weekly.push(acc),
                QuotaPhase::Extra => extra.push(acc),
                QuotaPhase::Exhausted => {}
            }
        }
        if !weekly.is_empty() {
            let chosen = self.pick_by_strategy(weekly, hint);
            return Ok(self.take(chosen));
        }
        if let Some(first) = extra.into_iter().next() {
            return Ok(self.take(first));
        }
        Err(self.no_candidate_error(now))
    }

    /// Pilih satu akun dari daftar yang sudah satu fase, menurut strategi
    /// pool (`affinity` default, `lru`, `round_robin`). Daftar masuk dalam
    /// urutan store; `round_robin` mengurutkan by id seperti sebelumnya.
    fn pick_by_strategy(&mut self, pool: Vec<Account>, hint: Option<&str>) -> Account {
        if self.strategy == "round_robin" {
            let mut sorted = pool;
            sorted.sort_by(|a, b| a.id.cmp(&b.id));
            self.round_robin = self.round_robin.wrapping_add(1);
            let idx = ((self.round_robin - 1) % sorted.len() as i64) as usize;
            sorted.into_iter().nth(idx).unwrap()
        } else if self.strategy == "lru" {
            pool.into_iter()
                .min_by(|a, b| {
                    a.last_used_at
                        .partial_cmp(&b.last_used_at)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap()
        } else {
            // affinity, default
            if let Some(hint) = hint {
                if let Some(id) = self.affinity.get(hint) {
                    if let Some(pinned) = pool.iter().find(|a| &a.id == id) {
                        return pinned.clone();
                    }
                }
            }
            let chosen = pool
                .into_iter()
                .min_by(|a, b| {
                    a.last_used_at
                        .partial_cmp(&b.last_used_at)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            if let Some(hint) = hint {
                self.affinity.insert(hint.to_string(), chosen.id.clone());
            }
            chosen
        }
    }

    fn take(&mut self, mut chosen: Account) -> Account {
        *self.inflight.entry(chosen.id.clone()).or_insert(0) += 1;
        chosen.last_used_at = now_unix();
        // .NET's Acquire mutates the pooled account in place (LastUsedAt)
        // but does not persist until Release; mirror that: update memory only.
        if let Some(slot) = self
            .store
            .accounts
            .iter_mut()
            .find(|a| a.id == chosen.id)
        {
            slot.last_used_at = chosen.last_used_at;
        }
        chosen
    }

    /// `AccountPool.Release` (Store.cs:228-266) without the `Lease` wrapper:
    /// frees the in-flight slot, applies the success/failure transitions,
    /// and persists.
    pub fn release(&mut self, id: &str, outcome: ReleaseOutcome) -> Result<(), StoreError> {
        let idx = match self.store.accounts.iter().position(|a| a.id == id) {
            Some(i) => i,
            None => return Ok(()),
        };
        let inflight = self.inflight.entry(id.to_string()).or_insert(0);
        *inflight = inflight.saturating_sub(1);
        match outcome {
            ReleaseOutcome::Success => {
                let acc = &mut self.store.accounts[idx];
                acc.success_count += 1;
                acc.last_error = None;
                if acc.status == AccountStatus::Cooling {
                    acc.status = AccountStatus::Active;
                }
            }
            ReleaseOutcome::Cancelled => {}
            ReleaseOutcome::Failure {
                message,
                code,
                retryable,
            } => {
                let now = now_unix();
                let acc = &mut self.store.accounts[idx];
                acc.fail_count += 1;
                let truncated: String = message.chars().take(500).collect();
                acc.last_error = Some(truncated);
                if code.as_deref() == Some("upstream_auth_failed") {
                    acc.status = AccountStatus::Invalid;
                } else if code.as_deref() == Some("upstream_quota_exhausted") {
                    acc.status = AccountStatus::Cooling;
                    acc.cooldown_until = now + self.cooldown.max(3600.0);
                } else if retryable {
                    acc.status = AccountStatus::Cooling;
                    acc.cooldown_until = now + self.cooldown;
                }
            }
        }
        self.store.persist()
    }

    /// `AccountPool.NoteWake` (Store.cs:268-283): an unconfirmed VM wake
    /// blocks video for 600 s; a confirmed wake clears the block.
    pub fn note_wake(&mut self, id: &str, meta: &HashMap<String, Option<String>>) {
        let Some(Some(ok)) = meta.get("vm_wake_ok") else {
            return;
        };
        let Some(acc) = self.store.accounts.iter_mut().find(|a| a.id == id) else {
            return;
        };
        let now = now_unix();
        if ok == "false" {
            let mut blocks = Map::new();
            blocks.insert("video".to_string(), dotnet_number(now + 600.0));
            acc.meta
                .insert("kind_blocked_until".to_string(), Value::Object(blocks));
        } else if ok == "true" {
            acc.meta.remove("kind_blocked_until");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("muse-store-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_account() -> Account {
        let mut acc = Account::new();
        acc.id = "acc_0123456789".to_string();
        acc.label = "bree-1".to_string();
        let mut cookies = Map::new();
        cookies.insert(
            "hatch_sess".to_string(),
            Value::String("abcdefghij1234567890".to_string()),
        );
        cookies.insert(
            "hatch_native_auth_device".to_string(),
            Value::String("device-aaa-111".to_string()),
        );
        cookies.insert("short".to_string(), Value::String("abc".to_string()));
        acc.cookies = cookies;
        let mut expires = Map::new();
        expires.insert(
            "hatch_sess".to_string(),
            Value::Number(1_800_000_000.into()),
        );
        acc.cookie_expires = expires;
        acc.success_count = 3;
        acc
    }

    #[test]
    fn public_masks_cookies_like_dotnet() {
        let acc = sample_account();
        let p = acc.public();
        assert_eq!(p["id"], "acc_0123456789");
        assert_eq!(p["status"], "active");
        // >8 chars -> first4...last4
        assert_eq!(p["cookies"]["hatch_sess"], "abcd...7890");
        // <=8 chars -> ***
        assert_eq!(p["cookies"]["short"], "***");
        // expires + counts pass through unmasked
        assert_eq!(p["cookie_expires"]["hatch_sess"], 1_800_000_000);
        assert_eq!(p["success_count"], 3);
    }

    #[test]
    fn is_available_rules() {
        let now = 1_000_000.0;
        let mut acc = sample_account();
        assert!(acc.is_available(now));
        acc.enabled = false;
        assert!(!acc.is_available(now));
        acc.enabled = true;
        acc.status = AccountStatus::Invalid;
        assert!(!acc.is_available(now));
        acc.status = AccountStatus::Cooling;
        acc.cooldown_until = now + 60.0;
        assert!(!acc.is_available(now));
        acc.cooldown_until = now;
        assert!(acc.is_available(now));
    }

    #[test]
    fn roundtrip_write_read() {
        let dir = tmpdir("account-rt");
        let file = dir.join("accounts.json");
        let mut store = AccountStore::new(&file);
        store.upsert(sample_account()).unwrap();
        assert!(file.exists());
        // File must be valid JSON with the contract field names.
        let text = std::fs::read_to_string(&file).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        // PascalCase + numeric enum on disk (CONTRACTS.md §6).
        assert_eq!(v[0]["Id"], "acc_0123456789");
        assert_eq!(v[0]["Status"], 0);
        assert_eq!(v[0]["Cookies"]["hatch_sess"], "abcdefghij1234567890");
        let mut store2 = AccountStore::new(&file);
        store2.load().unwrap();
        let back = store2.get("acc_0123456789").unwrap();
        assert_eq!(back.label, "bree-1");
        assert_eq!(back.success_count, 3);
        assert_eq!(back.cookie_expires["hatch_sess"], 1_800_000_000);
    }

    #[test]
    fn reads_dotnet_shaped_file() {
        // What .NET actually writes (PascalCase keys, numeric enum).
        let dir = tmpdir("account-dotnet");
        let file = dir.join("accounts.json");
        let mut f = std::fs::File::create(&file).unwrap();
        writeln!(
            f,
            r#"[{{"Id": "acc_abcdef0123", "Label": "net", "Enabled": true, "Status": 1, "Cookies": {{}}, "CookieExpires": {{}}, "CreatedAt": 1761972245, "LastUsedAt": 0, "CooldownUntil": 1761973000, "LastError": null, "SuccessCount": 0, "FailCount": 2, "Meta": {{}}}}]"#
        )
        .unwrap();
        drop(f);
        let mut store = AccountStore::new(&file);
        store.load().unwrap();
        let back = store.get("acc_abcdef0123").unwrap();
        assert_eq!(back.status, AccountStatus::Cooling);
        assert_eq!(back.fail_count, 2);
        assert_eq!(back.cooldown_until, 1_761_973_000.0);
    }

    #[test]
    fn reads_legacy_snake_case_file() {
        // Files written by older Rust (snake_case keys, string status)
        // must still load after the PascalCase switch.
        let dir = tmpdir("account-legacy");
        let file = dir.join("accounts.json");
        std::fs::write(
            &file,
            r#"[{"id": "acc_legacy0123", "label": "old", "enabled": true, "status": "cooling", "cookies": {}, "cookie_expires": {}, "created_at": 1761972245, "last_used_at": 0, "cooldown_until": 1761973000, "last_error": null, "success_count": 1, "fail_count": 0, "meta": {}}]"#,
        )
        .unwrap();
        let mut store = AccountStore::new(&file);
        store.load().unwrap();
        let back = store.get("acc_legacy0123").unwrap();
        assert_eq!(back.status, AccountStatus::Cooling);
        assert_eq!(back.success_count, 1);
    }

    #[test]
    fn upsert_session_refreshes_by_auth_hash() {
        let dir = tmpdir("account-sess");
        let file = dir.join("accounts.json");
        let mut store = AccountStore::new(&file);
        let mut first = sample_account();
        first.status = AccountStatus::Invalid;
        first.last_error = Some("bad".into());
        store.upsert(first).unwrap();
        let mut incoming = sample_account();
        // A fresh id: the refresh must come from the auth-hash match,
        // not from the plain id match in `upsert`.
        incoming.id = "acc_fedcba9876".to_string();
        incoming.label = "bree-1-renamed".to_string();
        let saved = store.upsert_session(incoming).unwrap();
        assert_eq!(saved.id, "acc_0123456789");
        assert_eq!(saved.label, "bree-1-renamed");
        assert_eq!(saved.status, AccountStatus::Active);
        assert_eq!(saved.last_error, None);
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn upsert_session_refreshes_by_device_when_sess_rotates() {
        // Same label + same device id, fresh hatch_sess (the site rotates
        // it on every fresh login): refresh in place, do not append.
        let dir = tmpdir("account-sess-dev");
        let file = dir.join("accounts.json");
        let mut store = AccountStore::new(&file);
        store.upsert(sample_account()).unwrap();
        let mut incoming = sample_account();
        incoming.id = "acc_fedcba9876".to_string();
        incoming.cookies.insert(
            "hatch_sess".to_string(),
            Value::String("rotated-sess-zzzz".to_string()),
        );
        let saved = store.upsert_session(incoming).unwrap();
        assert_eq!(saved.id, "acc_0123456789");
        assert_eq!(saved.cookie("hatch_sess"), "rotated-sess-zzzz");
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn upsert_session_appends_when_device_differs() {
        let dir = tmpdir("account-sess-dev2");
        let file = dir.join("accounts.json");
        let mut store = AccountStore::new(&file);
        store.upsert(sample_account()).unwrap();
        let mut incoming = sample_account();
        incoming.id = "acc_fedcba9876".to_string();
        incoming.cookies.insert(
            "hatch_sess".to_string(),
            Value::String("rotated-sess-zzzz".to_string()),
        );
        incoming.cookies.insert(
            "hatch_native_auth_device".to_string(),
            Value::String("device-bbb-222".to_string()),
        );
        let saved = store.upsert_session(incoming).unwrap();
        assert_eq!(saved.id, "acc_fedcba9876");
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn upsert_session_appends_when_label_differs() {
        let dir = tmpdir("account-sess-lbl");
        let file = dir.join("accounts.json");
        let mut store = AccountStore::new(&file);
        store.upsert(sample_account()).unwrap();
        let mut incoming = sample_account();
        incoming.id = "acc_fedcba9876".to_string();
        incoming.label = "other-account".to_string();
        incoming.cookies.insert(
            "hatch_sess".to_string(),
            Value::String("rotated-sess-zzzz".to_string()),
        );
        let saved = store.upsert_session(incoming).unwrap();
        assert_eq!(saved.id, "acc_fedcba9876");
        assert_eq!(store.all().len(), 2);
    }

    // --- Cacat A guard (3 Nov 2026) --------------------------------------
    // Rangkaian yang menghapus akun lama: gateway start, load gagal,
    // pool kosong, POST /admin/accounts -> upsert_session append ->
    // persist menimpa file hanya dengan akun baru. Sesudah perbaikan:
    // load yang gagal membackup file + menandai store, dan persist
    // MENOLAK menulis sampai load berhasil lagi — file asli utuh.
    #[test]
    fn corrupt_load_backs_up_and_refuses_persist() {
        let dir = tmpdir("account-corrupt-guard");
        let file = dir.join("accounts.json");
        let original = r#"[{"Id": "acc_0123456789", "Label": "lama", "Enabled": true, "Status": 0, "Cookies": {}, "CookieExpires": {}, "CreatedAt": 1761972245, "LastUsedAt": 0, "CooldownUntil": 0, "LastError": null, "SuccessCount": 0, "FailCount": 0, "Meta": {}}]"#;
        std::fs::write(&file, original).unwrap();
        // Rusak filenya (seperti gagal parse / gagal unprotect di live).
        std::fs::write(&file, "{bukan json yang valid").unwrap();

        let mut store = AccountStore::new(&file);
        let err = store.load().unwrap_err();
        assert!(store.load_failed());
        // Backup darurat dibuat dan isinya = file rusak (bukti forensik).
        let backup = store.load_failure_backup().expect("backup path recorded");
        assert!(backup.exists(), "backup file must exist: {}", backup.display());
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "{bukan json yang valid");
        assert!(err.to_string().contains("accounts.json"), "error names the file: {err}");

        // Upsert apa pun (jalur POST /admin/accounts) GAGAL, bukan wipe.
        let err = store.upsert_session(sample_account()).unwrap_err();
        assert!(
            err.to_string().contains("refusing to persist"),
            "persist must refuse, got: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{bukan json yang valid",
            "file on disk untouched by the refused persist"
        );

        // Setelah file diperbaiki, load berhasil lagi dan persist jalan.
        std::fs::write(&file, original).unwrap();
        store.load().unwrap();
        assert!(!store.load_failed());
        store.upsert(sample_account()).unwrap();
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn missing_file_load_is_not_a_failure() {
        // Tidak ada file = store kosong yang sah; persist tetap boleh.
        let dir = tmpdir("account-missing-ok");
        let file = dir.join("accounts.json");
        let mut store = AccountStore::new(&file);
        store.load().unwrap();
        assert!(!store.load_failed());
        store.upsert(sample_account()).unwrap();
        assert!(file.exists());
    }

    // --- Cacat B guard (3 Nov 2026) --------------------------------------
    // Skenario 12:27:07: file berisi 1 akun tapi dibaca 0 (atau pool
    // yang start kosong), lalu tulis berikutnya menimpa file hanya
    // dengan akun baru. Sesudah perbaikan: `persist()` yang mengecilkan
    // store tanpa delete eksplisit DITOLAK + file dibackup + penolakan
    // dicatat ke gateway.log. Satu-satunya penyusutan sah adalah
    // `remove()` (DELETE eksplisit).
    fn two_accounts_json() -> String {
        let a = sample_account();
        let mut b = sample_account();
        b.id = "acc_abcdef0123".to_string();
        b.label = "kedua".to_string();
        serde_json::to_string_pretty(&vec![a, b]).unwrap()
    }

    #[test]
    fn persist_refuses_shrink_without_delete() {
        let dir = tmpdir("account-shrink-guard");
        let file = dir.join("accounts.json");
        std::fs::write(&file, two_accounts_json()).unwrap();
        let mut store = AccountStore::new(&file);
        store.load().unwrap();
        assert_eq!(store.all().len(), 2);
        assert_eq!(store.known_count(), Some(2));

        // Pool di memori dikosongkan (meniru pool yang start / dibaca 0)
        // lalu ada yang mencoba persist: HARUS ditolak, file utuh.
        store.accounts.clear();
        let err = store.persist().unwrap_err();
        assert!(
            err.to_string().contains("refusing to persist")
                && err.to_string().contains("2->0"),
            "shrink must be refused with counts, got: {err}"
        );
        let back: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(back.as_array().unwrap().len(), 2, "file on disk untouched");
        // Backup darurat + jejak di gateway.log.
        let shrink_backup = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .any(|n| n.contains(".shrink-"));
        assert!(shrink_backup, "shrink backup must exist in {dir:?}");
        let log = std::fs::read_to_string(dir.join("gateway.log")).unwrap();
        assert!(log.contains("persist-refused"), "refusal logged: {log}");

        // Jalur sah tetap jalan: remove() eksplisit 2->1, lalu upsert 1->2.
        store.load().unwrap();
        assert!(store.remove("acc_abcdef0123").unwrap());
        assert_eq!(store.all().len(), 1);
        let mut third = sample_account();
        third.id = "acc_abcdef0123".to_string();
        third.label = "kedua".to_string();
        store.upsert(third).unwrap();
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn load_zero_byte_file_is_error_with_backup() {
        // File 0-byte (truncate-in-place / tulis yang terpotong) HARUS
        // gagal load — bukan dibaca sebagai "ok accounts=0". Bedakan
        // dari `[]` yang valid (tes berikutnya).
        let dir = tmpdir("account-zero-byte");
        let file = dir.join("accounts.json");
        std::fs::write(&file, "").unwrap();
        let mut store = AccountStore::new(&file);
        let err = store.load().unwrap_err();
        assert!(store.load_failed());
        assert!(store.load_failure_backup().is_some());
        assert!(err.to_string().contains("accounts.json"), "{err}");
        // Persist tetap menolak selama load_failed.
        assert!(store.upsert(sample_account()).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
    }

    #[test]
    fn load_empty_array_then_growth_ok() {
        // `[]` yang valid = 0 akun yang sah; upsert (tumbuh 0->1) jalan,
        // dan patokan known_count ikut naik.
        let dir = tmpdir("account-empty-array");
        let file = dir.join("accounts.json");
        std::fs::write(&file, "[]").unwrap();
        let mut store = AccountStore::new(&file);
        store.load().unwrap();
        assert!(!store.load_failed());
        assert_eq!(store.all().len(), 0);
        assert_eq!(store.known_count(), Some(0));
        store.upsert(sample_account()).unwrap();
        assert_eq!(store.all().len(), 1);
        assert_eq!(store.known_count(), Some(1));
    }

    #[test]
    fn persist_audit_log_records_writes() {
        let dir = tmpdir("account-persist-audit");
        let file = dir.join("accounts.json");
        let mut store = AccountStore::new(&file);
        store.load().unwrap();
        store.upsert(sample_account()).unwrap();
        let log = std::fs::read_to_string(dir.join("gateway.log")).unwrap();
        assert!(
            log.contains("persist: accounts 0->1"),
            "audit line expected, got: {log}"
        );
    }

    #[test]
    fn pool_acquire_release_transitions() {
        let dir = tmpdir("account-pool");
        let file = dir.join("accounts.json");
        let mut pool = AccountPool::new(&file, "affinity", 1, 120.0, 60.0);
        pool.store.upsert(sample_account()).unwrap();
        let acc = pool.acquire(&[], None, None).unwrap();
        assert_eq!(pool.stats()["inflight"], 1);
        // MaxConcurrency 1: no second lease while the first is out.
        assert!(pool.acquire(&[], None, None).is_err());
        pool.release(
            &acc.id,
            ReleaseOutcome::Failure {
                message: "quota gone".into(),
                code: Some("upstream_quota_exhausted".into()),
                retryable: false,
            },
        )
        .unwrap();
        let back = pool.store.get(&acc.id).unwrap();
        assert_eq!(back.status, AccountStatus::Cooling);
        assert_eq!(back.fail_count, 1);
        assert!(back.cooldown_until > now_unix() + 3500.0);
        assert_eq!(pool.stats()["inflight"], 0);
    }

    // --- Quota waterfall (3 Nov 2026) -------------------------------------
    // Tanggal reset dihitung relatif terhadap `now` nyata (dengan tahun
    // eksplisit) supaya tes tidak tergantung tanggal kalender saat dijalankan.
    fn reset_on(days: i64) -> String {
        let d = chrono::Utc::now() + chrono::Duration::days(days);
        d.format("%e %b %Y").to_string().trim().to_string()
    }

    fn account_with_quota(
        id: &str,
        label: &str,
        weekly: f64,
        resets: &str,
        extra: &str,
    ) -> Account {
        let mut acc = Account::new();
        acc.id = id.to_string();
        acc.label = label.to_string();
        let quota = serde_json::json!({
            "plan": "Paket gratis",
            "weekly_used_pct": weekly,
            "resets_at": resets,
            "extra_left": extra,
            "found": true,
            "checked_at": now_unix(),
        });
        acc.meta.insert("quota".to_string(), quota);
        acc
    }

    #[test]
    fn pool_waterfall_weekly_first_then_extra_then_out_of_credit() {
        let dir = tmpdir("account-pool-waterfall");
        let file = dir.join("accounts.json");
        let mut pool = AccountPool::new(&file, "affinity", 1, 120.0, 60.0);
        let reset = reset_on(6);
        pool.store
            .upsert(account_with_quota(
                "acc_a",
                "Akun 1",
                48.0,
                &reset,
                "sisa 31 M token",
            ))
            .unwrap();
        pool.store
            .upsert(account_with_quota(
                "acc_b",
                "Akun 2",
                100.0,
                &reset,
                "1B tokens left",
            ))
            .unwrap();
        pool.store
            .upsert(account_with_quota(
                "acc_c",
                "Akun 3",
                100.0,
                &reset,
                "0 left",
            ))
            .unwrap();

        // Fase 1: mingguan Akun 1 tersisa -> Akun 1, bukan Akun 2/3.
        let a = pool.acquire(&[], None, None).unwrap();
        assert_eq!(a.id, "acc_a");
        pool.release(&a.id, ReleaseOutcome::Success).unwrap();

        // Mingguan Akun 1 habis -> fase 2: extra Akun 2 (Akun 3 tanpa extra).
        pool.store
            .upsert(account_with_quota(
                "acc_a",
                "Akun 1",
                100.0,
                &reset,
                "0 left",
            ))
            .unwrap();
        let b = pool.acquire(&[], None, None).unwrap();
        assert_eq!(b.id, "acc_b");
        pool.release(&b.id, ReleaseOutcome::Success).unwrap();

        // Extra Akun 2 habis -> fase 3: out of credit, pesan jelas.
        pool.store
            .upsert(account_with_quota(
                "acc_b",
                "Akun 2",
                100.0,
                &reset,
                "0 left",
            ))
            .unwrap();
        let err = pool.acquire(&[], None, None).unwrap_err();
        assert!(
            matches!(err, StoreError::OutOfCredit(_)),
            "expected OutOfCredit, got {err}"
        );
        assert!(err.to_string().starts_with("out of credit:"));
        assert_eq!(pool.stats()["weekly"], 0);
        assert_eq!(pool.stats()["extra"], 0);
        assert_eq!(pool.stats()["exhausted"], 3);
        assert!(pool.out_of_credit(now_unix()).is_some());
    }

    #[test]
    fn pool_waterfall_phase_beats_affinity() {
        let dir = tmpdir("account-pool-waterfall-affinity");
        let file = dir.join("accounts.json");
        let mut pool = AccountPool::new(&file, "affinity", 1, 120.0, 60.0);
        let mut a = Account::new();
        a.id = "acc_a".to_string();
        a.label = "Akun 1".to_string();
        a.last_used_at = 1000.0;
        let mut b = Account::new();
        b.id = "acc_b".to_string();
        b.label = "Akun 2".to_string();
        b.last_used_at = 2000.0;
        pool.store.upsert(a).unwrap();
        pool.store.upsert(b).unwrap();

        // Tanpa snapshot: LRU + affinity pin ke Akun 1.
        let first = pool.acquire(&[], Some("s1"), None).unwrap();
        assert_eq!(first.id, "acc_a");
        pool.release(&first.id, ReleaseOutcome::Success).unwrap();

        // Akun 1 mingguan habis (reset 6 hari lagi): pin affinity TIDAK
        // boleh menariknya kembali selama Akun 2 masih fase mingguan.
        pool.store
            .upsert(account_with_quota(
                "acc_a",
                "Akun 1",
                100.0,
                &reset_on(6),
                "0 left",
            ))
            .unwrap();
        let second = pool.acquire(&[], Some("s1"), None).unwrap();
        assert_eq!(second.id, "acc_b");
    }

    #[test]
    fn pool_waterfall_reset_passed_means_weekly_again() {
        let dir = tmpdir("account-pool-waterfall-reset");
        let file = dir.join("accounts.json");
        let mut pool = AccountPool::new(&file, "affinity", 1, 120.0, 60.0);
        // Mingguan 100% tapi tanggal reset sudah lewat -> fase mingguan lagi.
        pool.store
            .upsert(account_with_quota(
                "acc_a",
                "Akun 1",
                100.0,
                &reset_on(-3),
                "0 left",
            ))
            .unwrap();
        let acc = pool.acquire(&[], None, None).unwrap();
        assert_eq!(acc.id, "acc_a");
    }

    #[test]
    fn pool_waterfall_no_snapshot_is_weekly() {
        let dir = tmpdir("account-pool-waterfall-nosnap");
        let file = dir.join("accounts.json");
        let mut pool = AccountPool::new(&file, "affinity", 1, 120.0, 60.0);
        let mut a = sample_account();
        a.id = "acc_a".to_string();
        let mut b = sample_account();
        b.id = "acc_b".to_string();
        pool.store.upsert(a).unwrap();
        pool.store.upsert(b).unwrap();
        // Tanpa snapshot sama sekali: perilaku lama (optimistis, tidak error).
        assert!(pool.acquire(&[], None, None).is_ok());
        assert!(pool.out_of_credit(now_unix()).is_none());
    }
}
