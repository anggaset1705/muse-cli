//! File protection for credential stores.
//!
//! Mirrors `PrivateDataProtection` (`PrivateDataProtection.cs`):
//! * On Windows, .NET encrypts `accounts.json` (and the `api_key` file) with
//!   DPAPI `CurrentUser` and prefixes the ciphertext with `muse-dpapi-v1:`.
//!   Plaintext files stay readable once, then are migrated in place.
//! * On non-Windows, .NET keeps the existing plaintext behaviour.
//!
//! DPAPI itself belongs to `muse-core` per CONTRACTS.md §11, so this crate
//! does **not** implement it: the default protector is the plaintext one
//! (correct on Linux, where the gateway runs today). A gateway that runs on
//! Windows injects a DPAPI protector via [`AccountStore::with_protector`]
//! (see `account.rs`). Reading a `muse-dpapi-v1:` file with the plaintext
//! protector fails with [`StoreError::Protected`], mirroring the .NET
//! `PlatformNotSupportedException` on a non-Windows host.

use crate::error::StoreError;

/// The prefix .NET writes in front of DPAPI ciphertext
/// (`PrivateDataProtection.Prefix`).
pub const DPAPI_PREFIX: &str = "muse-dpapi-v1:";

/// Something that can protect/unprotect a file's text contents.
pub trait Protector: Send + Sync {
    /// Turn plaintext into storable text (encrypt on Windows, identity on Linux).
    fn protect(&self, plaintext: &str) -> Result<String, StoreError>;
    /// Turn storable text back into plaintext.
    fn unprotect(&self, contents: &str) -> Result<String, StoreError>;
}

/// Plaintext protector: the non-Windows .NET behaviour. `protect` is the
/// identity; `unprotect` passes plaintext through and refuses DPAPI blobs.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlaintextProtector;

impl Protector for PlaintextProtector {
    fn protect(&self, plaintext: &str) -> Result<String, StoreError> {
        Ok(plaintext.to_string())
    }

    fn unprotect(&self, contents: &str) -> Result<String, StoreError> {
        if contents.starts_with(DPAPI_PREFIX) {
            Err(StoreError::Protected)
        } else {
            Ok(contents.to_string())
        }
    }
}

/// True when `contents` carry the DPAPI prefix
/// (`PrivateDataProtection.IsEncrypted`).
pub fn is_encrypted(contents: &str) -> bool {
    contents.starts_with(DPAPI_PREFIX)
}
