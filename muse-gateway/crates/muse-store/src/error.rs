//! Error type shared by all stores in this crate.

use std::path::PathBuf;
use thiserror::Error;

/// Errors a store operation can produce. Messages mirror the .NET
/// `InvalidOperationException` texts where one exists, so a gateway built on
/// this crate can surface the same wording.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("I/O error: {0}")]
    IoPlain(#[from] std::io::Error),
    #[error("Invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("Invalid JSON: {0}")]
    JsonPlain(#[from] serde_json::Error),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
    /// Terminal quota waterfall state: every usable account exhausted its
    /// weekly quota and its extra tokens (3 Nov 2026). Unlike `Invalid`
    /// "no usable account" (transient: cooling / in-flight), this must
    /// fail fast — waiting cannot help until a weekly reset.
    #[error("{0}")]
    OutOfCredit(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(
        "Credential file is Windows DPAPI-protected (muse-dpapi-v1:) and cannot be read on this platform; DPAPI support lives in muse-core"
    )]
    Protected,
    #[error("value too large: {0}")]
    TooLarge(String),
}

impl StoreError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        StoreError::Invalid(msg.into())
    }

    pub fn out_of_credit(msg: impl Into<String>) -> Self {
        StoreError::OutOfCredit(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        StoreError::NotFound(msg.into())
    }

    pub fn io_path(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        StoreError::Io {
            path: path.into(),
            source,
        }
    }

    pub fn json_path(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        StoreError::Json {
            path: path.into(),
            source,
        }
    }
}

/// Current Unix time in seconds as `f64`, like
/// `DateTimeOffset.UtcNow.ToUnixTimeSeconds()` stored in a .NET `double`.
pub fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `n` random bytes rendered as lowercase hex (the .NET `Guid.NewGuid().ToString("N")`
/// building block: 16 bytes -> 32 hex chars).
pub fn random_hex_bytes(n: usize) -> String {
    use rand::RngCore;
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 32 lowercase hex chars, i.e. a .NET `Guid.NewGuid().ToString("N")`.
pub fn new_guid_n() -> String {
    random_hex_bytes(16)
}

/// SHA-256 of `input`, lowercase hex (what .NET's
/// `Convert.ToHexString(SHA256.HashData(...)).ToLowerInvariant()` produces).
pub fn sha256_hex_lower(input: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(input);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of `input`, UPPERCASE hex (what .NET's `Convert.ToHexString(...)`
/// produces without the `ToLowerInvariant()` call).
pub fn sha256_hex_upper(input: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(input);
    digest.iter().map(|b| format!("{b:02X}")).collect()
}
