//! JSON helpers that mirror `System.Text.Json` behaviour used by Store.cs.
//!
//! Two deliberate choices, both documented where they matter:
//! * **Write side (PascalCase + numeric enums):** CONTRACTS.md §6
//!   (koreksi koordinator 2 Nov 2026): .NET serializes with default options,
//!   i.e. PascalCase property names and numeric enums (`"Id"`,
//!   `"Status": 0`). This crate writes that form.
//! * **Read side (tolerant):** every struct also accepts the PascalCase
//!   spelling (via serde `alias`) and enums accept both the string form
//!   (`"active"`) and the numeric form (`0`), so files genuinely written
//!   by the .NET gateway still load.

use serde::Serializer;

use crate::error::StoreError;
use std::path::Path;

/// Serialize an `f64` the way `System.Text.Json` writes a `double`: an
/// integral value is written without a decimal point (`1761972245`, not
/// `1761972245.0`); a fractional value keeps the shortest round-trippable
/// form.
pub fn ser_f64_dotnet<S>(v: &f64, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if v.is_finite() && v.fract() == 0.0 && *v >= i64::MIN as f64 && *v <= i64::MAX as f64 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

/// Serialize a `serde_json::Value` indented with two spaces, like
/// `new JsonSerializerOptions { WriteIndented = true }`.
pub fn to_string_pretty(value: &serde_json::Value) -> Result<String, StoreError> {
    serde_json::to_string_pretty(value).map_err(StoreError::JsonPlain)
}

/// Atomically replace `path` with `contents`: write a temp file in the same
/// directory, then rename over the destination. `tmp_name` is the temp file
/// name (no directory); each .NET store uses its own naming, preserved by
/// the callers.
pub fn write_atomic(path: &Path, contents: &str, tmp_name: &str) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| StoreError::io_path(dir, e))?;
    let tmp = dir.join(tmp_name);
    std::fs::write(&tmp, contents).map_err(|e| StoreError::io_path(&tmp, e))?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(StoreError::io_path(path, e))
        }
    }
}

/// Read a whole file to a string. Missing file is reported as
/// `std::io::ErrorKind::NotFound` so callers can treat "no file yet" as an
/// empty store, exactly like the .NET `Load()` methods that return early
/// when `!File.Exists(...)`.
pub fn read_file_to_string(path: &Path) -> Result<String, StoreError> {
    std::fs::read_to_string(path).map_err(|e| StoreError::io_path(path, e))
}
