//! DPAPI credential protection, mirroring .NET `PrivateDataProtection.cs`.
//!
//! Format: `muse-dpapi-v1:<base64>` — DPAPI `CurrentUser` with entropy
//! `Muse2Api/local-private-storage/v1` (`ProtectedData.Protect` / `Unprotect`).
//! On non-Windows the .NET behaviour is kept exactly: `protect` returns the
//! plaintext unchanged, and `unprotect` of an encrypted blob throws
//! `PlatformNotSupportedException` ("Muse credential file is Windows
//! DPAPI-protected and requires the original Windows account.") — here
//! surfaced as `MuseError::not_implemented` with that same message.

use crate::errors::MuseError;

pub const PREFIX: &str = "muse-dpapi-v1:";
pub const ENTROPY: &[u8] = b"Muse2Api/local-private-storage/v1";

/// Message thrown by .NET `PrivateDataProtection.Unprotect` on non-Windows.
pub const NON_WINDOWS_MESSAGE: &str =
    "Muse credential file is Windows DPAPI-protected and requires the original Windows account.";

pub fn is_encrypted(contents: &str) -> bool {
    contents.starts_with(PREFIX)
}

/// Mirrors .NET `PrivateDataProtection.Protect`: on non-Windows returns the
/// plaintext unchanged; on Windows returns `muse-dpapi-v1:<base64>`.
pub fn protect(plaintext: &str) -> Result<String, MuseError> {
    #[cfg(windows)]
    {
        protect_windows(plaintext)
    }
    #[cfg(not(windows))]
    {
        Ok(plaintext.to_string())
    }
}

/// Mirrors .NET `PrivateDataProtection.Unprotect`: plaintext passes through
/// unchanged; a `muse-dpapi-v1:` blob is decrypted on Windows and is an
/// error elsewhere.
pub fn unprotect(contents: &str) -> Result<String, MuseError> {
    if !is_encrypted(contents) {
        return Ok(contents.to_string());
    }
    #[cfg(windows)]
    {
        unprotect_windows(contents)
    }
    #[cfg(not(windows))]
    {
        Err(MuseError::not_implemented(NON_WINDOWS_MESSAGE))
    }
}

/// Mirrors .NET `PrivateDataProtection.ReadFile`.
pub fn read_file(path: &std::path::Path) -> Result<String, MuseError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| MuseError::internal(format!("cannot read {}: {e}", path.display())))?;
    unprotect(&raw)
}

/// Mirrors .NET `PrivateDataProtection.WriteFile`: encrypt first, then write
/// atomically (temp file + rename); the temp file is removed on failure.
pub fn write_file(path: &std::path::Path, plaintext: &str) -> Result<(), MuseError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MuseError::internal(e.to_string()))?;
        }
    }
    let encrypted = protect(plaintext)?;
    let tmp = path.with_extension(format!(
        "muse-protect-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<(), MuseError> {
        std::fs::write(&tmp, encrypted.as_bytes())
            .map_err(|e| MuseError::internal(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| MuseError::internal(e.to_string()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Mirrors .NET `PrivateDataProtection.MigrateLegacyFile`: on Windows,
/// re-write a plaintext file encrypted; never touch an unreadable or already
/// encrypted file; no-op on non-Windows.
pub fn migrate_legacy_file(path: &std::path::Path) -> Result<(), MuseError> {
    #[cfg(windows)]
    {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| MuseError::internal(format!("cannot read {}: {e}", path.display())))?;
        if !is_encrypted(&raw) {
            write_file(path, &raw)?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use base64::Engine;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    fn blob_from_slice(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    fn blob_to_vec(blob: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if blob.pbData.is_null() || blob.cbData == 0 {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec() }
    }

    fn free_blob(blob: &CRYPT_INTEGER_BLOB) {
        if !blob.pbData.is_null() {
            unsafe {
                LocalFree(Some(HLOCAL(blob.pbData as *mut core::ffi::c_void)));
            }
        }
    }

    pub(super) fn protect_windows(plaintext: &str) -> Result<String, MuseError> {
        let plain = plaintext.as_bytes();
        let entropy = blob_from_slice(ENTROPY);
        let input = blob_from_slice(plain);
        let mut output = CRYPT_INTEGER_BLOB::default();
        let result = unsafe {
            CryptProtectData(
                &input,
                windows::core::PCWSTR::null(),
                Some(&entropy as *const CRYPT_INTEGER_BLOB),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if let Err(e) = result {
            return Err(MuseError::internal(format!("DPAPI protect failed: {e}")));
        }
        let bytes = blob_to_vec(&output);
        free_blob(&output);
        Ok(format!(
            "{PREFIX}{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    }

    pub(super) fn unprotect_windows(contents: &str) -> Result<String, MuseError> {
        let b64 = contents[PREFIX.len()..].trim();
        let encrypted = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| MuseError::invalid(format!("invalid DPAPI base64 in credential file: {e}")))?;
        let entropy = blob_from_slice(ENTROPY);
        let input = blob_from_slice(&encrypted);
        let mut output = CRYPT_INTEGER_BLOB::default();
        let result = unsafe {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy as *const CRYPT_INTEGER_BLOB),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if let Err(e) = result {
            return Err(MuseError::internal(format!("DPAPI unprotect failed: {e}")));
        }
        let bytes = blob_to_vec(&output);
        free_blob(&output);
        String::from_utf8(bytes).map_err(|e| {
            MuseError::invalid(format!("DPAPI-decrypted credential is not valid UTF-8: {e}"))
        })
    }
}

#[cfg(windows)]
use windows_impl::{protect_windows, unprotect_windows};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_encrypted_prefix_check() {
        assert!(is_encrypted("muse-dpapi-v1:AAAA"));
        assert!(!is_encrypted("m2a-abcdef"));
        assert!(!is_encrypted(""));
    }

    #[test]
    fn unprotect_passthrough_for_plaintext() {
        assert_eq!(unprotect("m2a-abcdef").unwrap(), "m2a-abcdef");
        assert_eq!(unprotect("").unwrap(), "");
    }

    #[cfg(not(windows))]
    #[test]
    fn protect_is_identity_off_windows_like_dotnet() {
        // .NET PrivateDataProtection.Protect returns plaintext on non-Windows.
        assert_eq!(protect("m2a-secret").unwrap(), "m2a-secret");
    }

    #[cfg(not(windows))]
    #[test]
    fn unprotect_encrypted_blob_errors_like_dotnet_platform_not_supported() {
        // .NET throws PlatformNotSupportedException with this exact message.
        let err = unprotect("muse-dpapi-v1:AAAA").unwrap_err();
        assert_eq!(err.status_code, 501);
        assert_eq!(err.message, NON_WINDOWS_MESSAGE);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_returns_original_value() {
        // Proves the real Windows DPAPI path: encrypt then decrypt.
        let original = "m2a-0123456789abcdef0123456789abcdef0123";
        let protected = protect(original).unwrap();
        assert!(is_encrypted(&protected));
        assert_ne!(protected, original);
        let back = unprotect(&protected).unwrap();
        assert_eq!(back, original);
        // Plaintext passthrough still works on Windows too.
        assert_eq!(unprotect(original).unwrap(), original);
    }

    #[test]
    fn write_and_read_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "muse-core-dpapi-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("api_key");
        write_file(&path, "m2a-testkey").unwrap();
        assert_eq!(read_file(&path).unwrap(), "m2a-testkey");
        // Atomic write leaves no temp files behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
