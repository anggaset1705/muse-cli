//! Runtime configuration, mirroring .NET `Settings` (`Muse2Api/Settings.cs`).
//! Every value comes from a `MUSE2API_*` environment variable with the same
//! name and default as the .NET original.
//!
//! This is the canonical owner of `Settings` (see CONTRACTS.md §11): the
//! `muse-gateway` skeleton's `settings.rs` was the Phase 1 copy and the
//! gateway will switch to this crate at integration. Behaviour is identical
//! to that copy except `resolve_api_key`, which now goes through
//! `crate::dpapi` (real DPAPI on Windows, .NET's
//! `PlatformNotSupportedException` message on other platforms) and mirrors
//! .NET `ResolveApiKey` exactly, including the legacy-plaintext migration.

use std::path::PathBuf;

use crate::dpapi;

#[derive(Debug, Clone)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub log_level: String,
    pub public_base: String,
    pub api_key: String,
    pub admin_key: String,
    pub data_dir: PathBuf,
    pub request_log_retention_days: f64,
    pub media_retention_days: f64,
    pub driver: String,
    pub site_url: String,
    pub chat_timeout: f64,
    pub first_token_timeout: f64,
    pub chat_idle_timeout: f64,
    pub reply_render_grace: f64,
    pub stream_heartbeat_interval: f64,
    pub image_timeout: f64,
    pub video_timeout: f64,
    pub pool_strategy: String,
    pub pool_acquire_timeout: f64,
    pub account_max_concurrency: i64,
    pub account_cooldown: f64,
    pub max_failover: i64,
    pub quota_cache_seconds: f64,
    pub wake_confirm_seconds: f64,
    pub keepalive_enabled: bool,
    pub keepalive_interval: f64,
    pub chromium_path: String,
    /// CDP port Chromium listens on. 0 = pick a free loopback port per
    /// launch (an orphaned Chrome from a force-killed gateway can no
    /// longer squat a fixed port and break the next launch); an explicit
    /// `MUSE2API_CDP_PORT` value wins (diagnostics).
    pub cdp_port: u16,
    pub headless: bool,
    pub browser_proxy: String,
    pub browser_profile_dir: Option<String>,
    pub page_ready_timeout: f64,
}

impl Default for Settings {
    /// Hardcoded fallbacks, identical to `load()`'s fallbacks but without
    /// reading the environment (integration addition, 2 Nov 2026: the
    /// browser crate's `DriverSettings` became an alias for this struct,
    /// and its tests use `..Settings::default()` as a struct-update base).
    fn default() -> Self {
        Settings {
            host: "127.0.0.1".to_string(),
            port: 18610,
            log_level: "INFO".to_string(),
            public_base: String::new(),
            api_key: String::new(),
            admin_key: String::new(),
            data_dir: PathBuf::from("data"),
            request_log_retention_days: 14.0,
            media_retention_days: 14.0,
            driver: "mock".to_string(),
            site_url: "https://muse.ai".to_string(),
            chat_timeout: 300.0,
            first_token_timeout: 45.0,
            chat_idle_timeout: 90.0,
            reply_render_grace: 15.0,
            stream_heartbeat_interval: 0.0,
            image_timeout: 240.0,
            video_timeout: 600.0,
            pool_strategy: "affinity".to_string(),
            pool_acquire_timeout: 60.0,
            account_max_concurrency: 1,
            account_cooldown: 120.0,
            max_failover: 2,
            quota_cache_seconds: 1800.0,
            wake_confirm_seconds: 20.0,
            keepalive_enabled: false,
            keepalive_interval: 6.0 * 3600.0,
            chromium_path: String::new(),
            cdp_port: 0, // 0 = free port per launch (see field comment)
            headless: true,
            browser_proxy: String::new(),
            browser_profile_dir: None,
            page_ready_timeout: 45.0,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let browser_profile_dir = match env("BROWSER_PROFILE_DIR", "") {
            s if !s.is_empty() => Some(s),
            _ => None,
        };
        Settings {
            host: env("HOST", "127.0.0.1"),
            port: env_int("PORT", 18610),
            log_level: env("LOG_LEVEL", "INFO"),
            public_base: env("PUBLIC_BASE", ""),
            api_key: env("API_KEY", ""),
            admin_key: env("ADMIN_KEY", ""),
            data_dir: PathBuf::from(env("DATA_DIR", "data")),
            request_log_retention_days: env_f64("REQUEST_LOG_RETENTION_DAYS", 14.0),
            media_retention_days: env_f64("MEDIA_RETENTION_DAYS", 14.0),
            driver: env("DRIVER", "mock"),
            site_url: env("SITE_URL", "https://muse.ai"),
            chat_timeout: env_f64("CHAT_TIMEOUT", 300.0),
            first_token_timeout: env_f64("FIRST_TOKEN_TIMEOUT", 45.0),
            chat_idle_timeout: env_f64("CHAT_IDLE_TIMEOUT", 90.0),
            reply_render_grace: env_f64("REPLY_RENDER_GRACE", 15.0),
            stream_heartbeat_interval: env_f64("STREAM_HEARTBEAT_INTERVAL", 0.0),
            image_timeout: env_f64("IMAGE_TIMEOUT", 240.0),
            video_timeout: env_f64("VIDEO_TIMEOUT", 600.0),
            pool_strategy: env("POOL_STRATEGY", "affinity"),
            pool_acquire_timeout: env_f64("POOL_ACQUIRE_TIMEOUT", 60.0),
            account_max_concurrency: env_int("ACCOUNT_MAX_CONCURRENCY", 1) as i64,
            account_cooldown: env_f64("ACCOUNT_COOLDOWN", 120.0),
            max_failover: env_int("MAX_FAILOVER", 2) as i64,
            quota_cache_seconds: env_f64("QUOTA_CACHE_S", 1800.0),
            wake_confirm_seconds: env_f64("WAKE_CONFIRM_S", 20.0),
            keepalive_enabled: env_bool("KEEPALIVE_ENABLED", false),
            keepalive_interval: env_f64("KEEPALIVE_INTERVAL", 6.0 * 3600.0),
            chromium_path: env("CHROMIUM_PATH", ""),
            cdp_port: env_int("CDP_PORT", 0), // 0 = free port per launch
            headless: env_bool("HEADLESS", true),
            browser_proxy: env("BROWSER_PROXY", ""),
            browser_profile_dir,
            page_ready_timeout: env_f64("PAGE_READY_TIMEOUT", 45.0),
        }
    }

    /// Effective admin key: explicit admin key, else the API key
    /// (mirrors .NET `Settings.EffectiveAdminKey`).
    pub fn effective_admin_key(&self) -> &str {
        if !self.admin_key.is_empty() {
            &self.admin_key
        } else {
            &self.api_key
        }
    }

    pub fn profile_dir(&self) -> PathBuf {
        match &self.browser_profile_dir {
            Some(p) => PathBuf::from(p),
            None => self.data_dir.join("browser-profile"),
        }
    }

    pub fn media_dir(&self) -> PathBuf {
        self.data_dir.join("media")
    }
    pub fn accounts_file(&self) -> PathBuf {
        self.data_dir.join("accounts.json")
    }
    pub fn tasks_file(&self) -> PathBuf {
        self.data_dir.join("tasks.json")
    }
    pub fn requests_file(&self) -> PathBuf {
        self.data_dir.join("requests.db")
    }
    pub fn key_file(&self) -> PathBuf {
        self.data_dir.join("api_key")
    }
    pub fn keys_file(&self) -> PathBuf {
        self.data_dir.join("keys.json")
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("MUSE2API_PORT must be between 1 and 65535".into());
        }
        if !matches!(self.driver.as_str(), "mock" | "browser" | "http") {
            return Err("MUSE2API_DRIVER must be mock, browser or http".into());
        }
        if !matches!(
            self.pool_strategy.as_str(),
            "affinity" | "lru" | "round_robin"
        ) {
            return Err("invalid MUSE2API_POOL_STRATEGY".into());
        }
        if self.account_max_concurrency < 1 || self.max_failover < 0 {
            return Err("account concurrency must be positive and max failover must be non-negative".into());
        }
        for (name, v) in [
            ("CHAT_TIMEOUT", self.chat_timeout),
            ("FIRST_TOKEN_TIMEOUT", self.first_token_timeout),
            ("CHAT_IDLE_TIMEOUT", self.chat_idle_timeout),
            ("REPLY_RENDER_GRACE", self.reply_render_grace),
            ("IMAGE_TIMEOUT", self.image_timeout),
            ("VIDEO_TIMEOUT", self.video_timeout),
            ("PAGE_READY_TIMEOUT", self.page_ready_timeout),
            ("KEEPALIVE_INTERVAL", self.keepalive_interval),
        ] {
            if !v.is_finite() || v <= 0.0 {
                return Err(format!("MUSE2API_{name} must be finite and positive"));
            }
        }
        for (name, v) in [
            ("POOL_ACQUIRE_TIMEOUT", self.pool_acquire_timeout),
            ("ACCOUNT_COOLDOWN", self.account_cooldown),
            ("STREAM_HEARTBEAT_INTERVAL", self.stream_heartbeat_interval),
            ("REQUEST_LOG_RETENTION_DAYS", self.request_log_retention_days),
            ("MEDIA_RETENTION_DAYS", self.media_retention_days),
            ("QUOTA_CACHE_S", self.quota_cache_seconds),
            ("WAKE_CONFIRM_S", self.wake_confirm_seconds),
        ] {
            if !v.is_finite() || v < 0.0 {
                return Err(format!("MUSE2API_{name} must be finite and non-negative"));
            }
        }
        Ok(())
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(self.media_dir())?;
        Ok(())
    }

    /// Mirrors .NET `Settings.ResolveApiKey`: explicit env key wins; else read
    /// `<data>/api_key` (DPAPI-decrypted on Windows; a DPAPI-prefixed file on
    /// non-Windows is an error with .NET's `PlatformNotSupportedException`
    /// message), migrating a legacy plaintext file to DPAPI storage on
    /// Windows; else generate `m2a-<36 hex lowercase>` and persist it
    /// (atomically, DPAPI-encrypted on Windows) via `dpapi::write_file`.
    ///
    /// Note the .NET order: `EnsureDirs()` runs before the file is read.
    pub fn resolve_api_key(mut self) -> Result<Self, String> {
        if !self.api_key.is_empty() {
            return Ok(self);
        }
        self.ensure_dirs()
            .map_err(|e| format!("cannot create data dir: {e}"))?;
        let key_file = self.key_file();
        if key_file.exists() {
            let raw = std::fs::read_to_string(&key_file)
                .map_err(|e| format!("cannot read {}: {e}", key_file.display()))?;
            let key = dpapi::unprotect(raw.trim()).map_err(|e| e.message)?;
            if !key.is_empty() {
                self.api_key = key;
                // Preserve the key and upgrade plaintext storage for existing
                // desktop profiles (Windows only; no-op elsewhere).
                dpapi::migrate_legacy_file(&key_file).map_err(|e| e.message)?;
                return Ok(self);
            }
        }
        let key = generate_api_key();
        dpapi::write_file(&key_file, &key).map_err(|e| e.message)?;
        self.api_key = key;
        Ok(self)
    }
}

/// Generate a fresh gateway key: `m2a-<36 lowercase hex>` (18 random bytes),
/// mirroring .NET `ResolveApiKey` (`"m2a-" + Convert.ToHexString(
/// RandomNumberGenerator.GetBytes(18)).ToLowerInvariant()`).
pub fn generate_api_key() -> String {
    format!("m2a-{}", random_hex(18))
}

fn random_hex(n_bytes: usize) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..n_bytes).map(|_| rng.gen()).collect();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn env(name: &str, fallback: &str) -> String {
    match std::env::var(format!("MUSE2API_{name}")) {
        Ok(v) if !v.is_empty() => v,
        _ => fallback.to_string(),
    }
}

fn env_int(name: &str, fallback: u16) -> u16 {
    std::env::var(format!("MUSE2API_{name}"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

fn env_f64(name: &str, fallback: f64) -> f64 {
    std::env::var(format!("MUSE2API_{name}"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

fn env_bool(name: &str, fallback: bool) -> bool {
    match std::env::var(format!("MUSE2API_{name}")) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "True" | "TRUE"),
        Err(_) => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Env vars are process-global, so env-touching tests run serialized.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&'static str, &str)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(k, _)| (*k, std::env::var(k).ok()))
                .collect();
            for (k, v) in pairs {
                std::env::set_var(k, v);
            }
            EnvGuard { saved }
        }

        fn clear(names: &[&'static str]) -> Self {
            let saved = names
                .iter()
                .map(|k| (*k, std::env::var(k).ok()))
                .collect();
            for k in names {
                std::env::remove_var(k);
            }
            EnvGuard { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    const ALL_ENV: &[&str] = &[
        "MUSE2API_HOST",
        "MUSE2API_PORT",
        "MUSE2API_LOG_LEVEL",
        "MUSE2API_PUBLIC_BASE",
        "MUSE2API_API_KEY",
        "MUSE2API_ADMIN_KEY",
        "MUSE2API_DATA_DIR",
        "MUSE2API_REQUEST_LOG_RETENTION_DAYS",
        "MUSE2API_MEDIA_RETENTION_DAYS",
        "MUSE2API_DRIVER",
        "MUSE2API_SITE_URL",
        "MUSE2API_CHAT_TIMEOUT",
        "MUSE2API_FIRST_TOKEN_TIMEOUT",
        "MUSE2API_CHAT_IDLE_TIMEOUT",
        "MUSE2API_REPLY_RENDER_GRACE",
        "MUSE2API_STREAM_HEARTBEAT_INTERVAL",
        "MUSE2API_IMAGE_TIMEOUT",
        "MUSE2API_VIDEO_TIMEOUT",
        "MUSE2API_POOL_STRATEGY",
        "MUSE2API_POOL_ACQUIRE_TIMEOUT",
        "MUSE2API_ACCOUNT_MAX_CONCURRENCY",
        "MUSE2API_ACCOUNT_COOLDOWN",
        "MUSE2API_MAX_FAILOVER",
        "MUSE2API_QUOTA_CACHE_S",
        "MUSE2API_WAKE_CONFIRM_S",
        "MUSE2API_KEEPALIVE_ENABLED",
        "MUSE2API_KEEPALIVE_INTERVAL",
        "MUSE2API_CHROMIUM_PATH",
        "MUSE2API_CDP_PORT",
        "MUSE2API_HEADLESS",
        "MUSE2API_BROWSER_PROXY",
        "MUSE2API_BROWSER_PROFILE_DIR",
        "MUSE2API_PAGE_READY_TIMEOUT",
    ];

    #[test]
    fn load_defaults_match_dotnet() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::clear(ALL_ENV);
        let s = Settings::load();
        assert_eq!(s.host, "127.0.0.1");
        assert_eq!(s.port, 18610);
        assert_eq!(s.log_level, "INFO");
        assert_eq!(s.driver, "mock");
        assert_eq!(s.site_url, "https://muse.ai");
        assert_eq!(s.data_dir, PathBuf::from("data"));
        assert_eq!(s.pool_strategy, "affinity");
        assert_eq!(s.cdp_port, 0); // 0 = free port per launch
        assert!(s.headless);
        assert_eq!(s.chat_timeout, 300.0);
        assert_eq!(s.keepalive_interval, 6.0 * 3600.0);
        assert_eq!(s.browser_profile_dir, None);
        assert_eq!(s.profile_dir(), PathBuf::from("data/browser-profile"));
        assert!(s.validate().is_ok());
    }

    #[test]
    fn load_env_overrides_and_paths() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::set(&[
            ("MUSE2API_HOST", "0.0.0.0"),
            ("MUSE2API_PORT", "8080"),
            ("MUSE2API_DRIVER", "browser"),
            ("MUSE2API_DATA_DIR", "/tmp/muse-test-data"),
            ("MUSE2API_API_KEY", "m2a-envkey"),
            ("MUSE2API_ADMIN_KEY", "adminkey"),
            ("MUSE2API_BROWSER_PROFILE_DIR", "/tmp/prof"),
            ("MUSE2API_KEEPALIVE_ENABLED", "1"),
            ("MUSE2API_HEADLESS", "false"),
        ]);
        let s = Settings::load();
        assert_eq!(s.host, "0.0.0.0");
        assert_eq!(s.port, 8080);
        assert_eq!(s.driver, "browser");
        assert_eq!(s.api_key, "m2a-envkey");
        assert_eq!(s.effective_admin_key(), "adminkey");
        assert_eq!(s.profile_dir(), PathBuf::from("/tmp/prof"));
        assert_eq!(s.media_dir(), PathBuf::from("/tmp/muse-test-data/media"));
        assert_eq!(s.accounts_file(), PathBuf::from("/tmp/muse-test-data/accounts.json"));
        assert_eq!(s.tasks_file(), PathBuf::from("/tmp/muse-test-data/tasks.json"));
        assert_eq!(s.requests_file(), PathBuf::from("/tmp/muse-test-data/requests.db"));
        assert_eq!(s.key_file(), PathBuf::from("/tmp/muse-test-data/api_key"));
        assert_eq!(s.keys_file(), PathBuf::from("/tmp/muse-test-data/keys.json"));
        assert!(s.keepalive_enabled);
        assert!(!s.headless);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn effective_admin_key_falls_back_to_api_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::set(&[("MUSE2API_API_KEY", "m2a-k"), ("MUSE2API_ADMIN_KEY", "")]);
        let s = Settings::load();
        assert_eq!(s.effective_admin_key(), "m2a-k");
    }

    #[test]
    fn env_bool_parsing_matches_dotnet() {
        let _lock = ENV_LOCK.lock().unwrap();
        for (v, expected) in [("1", true), ("true", true), ("True", true), ("TRUE", true), ("false", false), ("0", false), ("yes", false)] {
            let _guard = EnvGuard::set(&[("MUSE2API_HEADLESS", v)]);
            assert_eq!(Settings::load().headless, expected, "value {v}");
        }
    }

    #[test]
    fn env_int_and_f64_fall_back_on_garbage() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::set(&[("MUSE2API_PORT", "abc"), ("MUSE2API_CHAT_TIMEOUT", "xyz")]);
        let s = Settings::load();
        assert_eq!(s.port, 18610);
        assert_eq!(s.chat_timeout, 300.0);
    }

    #[test]
    fn validate_rejects_bad_values() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::clear(ALL_ENV);
        let mut s = Settings::load();
        s.port = 0;
        assert!(s.validate().is_err());
        s.port = 18610;
        s.driver = "nope".into();
        assert_eq!(s.validate().unwrap_err(), "MUSE2API_DRIVER must be mock, browser or http");
        s.driver = "mock".into();
        s.pool_strategy = "random".into();
        assert!(s.validate().is_err());
        s.pool_strategy = "lru".into();
        s.chat_timeout = 0.0;
        assert!(s.validate().unwrap_err().contains("MUSE2API_CHAT_TIMEOUT"));
        s.chat_timeout = 300.0;
        s.account_cooldown = -1.0;
        assert!(s.validate().unwrap_err().contains("MUSE2API_ACCOUNT_COOLDOWN"));
        s.account_cooldown = 120.0;
        assert!(s.validate().is_ok());
    }

    #[test]
    fn generate_api_key_format() {
        let key = generate_api_key();
        assert!(key.starts_with("m2a-"), "{key}");
        let hex = &key[4..];
        assert_eq!(hex.len(), 36, "{key}");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{key}");
        assert_ne!(generate_api_key(), generate_api_key());
    }

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("muse-core-settings-test-{}", uuid::Uuid::new_v4().simple()))
    }

    #[test]
    fn resolve_api_key_env_wins_and_generates_and_reads_back() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = temp_data_dir();
        // Env key wins, no file is created.
        let _guard = EnvGuard::set(&[
            ("MUSE2API_API_KEY", "m2a-from-env"),
            ("MUSE2API_DATA_DIR", dir.to_str().unwrap()),
        ]);
        let s = Settings::load().resolve_api_key().unwrap();
        assert_eq!(s.api_key, "m2a-from-env");
        assert!(!dir.join("api_key").exists());
        drop(_guard);

        // No env key: generates m2a-<36 hex>, persists, reads back the same key.
        let _guard = EnvGuard::set(&[
            ("MUSE2API_API_KEY", ""),
            ("MUSE2API_DATA_DIR", dir.to_str().unwrap()),
        ]);
        // Empty env var counts as unset (mirrors .NET Env()).
        let s = Settings::load().resolve_api_key().unwrap();
        assert!(s.api_key.starts_with("m2a-"));
        assert_eq!(s.api_key.len(), 40);
        let stored = std::fs::read_to_string(dir.join("api_key")).unwrap();
        assert_eq!(stored, s.api_key);
        let s2 = Settings::load().resolve_api_key().unwrap();
        assert_eq!(s2.api_key, s.api_key);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_api_key_reads_legacy_plaintext_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = temp_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("api_key"), "m2a-legacy-key\n").unwrap();
        let _guard = EnvGuard::set(&[
            ("MUSE2API_API_KEY", ""),
            ("MUSE2API_DATA_DIR", dir.to_str().unwrap()),
        ]);
        let s = Settings::load().resolve_api_key().unwrap();
        assert_eq!(s.api_key, "m2a-legacy-key");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn resolve_api_key_dpapi_file_errors_off_windows() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = temp_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("api_key"), "muse-dpapi-v1:AAAA").unwrap();
        let _guard = EnvGuard::set(&[
            ("MUSE2API_API_KEY", ""),
            ("MUSE2API_DATA_DIR", dir.to_str().unwrap()),
        ]);
        let err = Settings::load().resolve_api_key().unwrap_err();
        assert_eq!(err, dpapi::NON_WINDOWS_MESSAGE);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
