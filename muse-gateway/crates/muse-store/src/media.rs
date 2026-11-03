//! `media/`: `MediaStore` (Store.cs:845-910).
//!
//! Generated images/video are saved under `<data>/media/` and served back
//! at `GET /v1/media/{name}`. Retention comes from
//! `MUSE2API_MEDIA_RETENTION_DAYS` (default 14, Settings.cs); the store
//! itself takes seconds, like `MediaStore.Prune(double maxAgeSeconds)`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{new_guid_n, StoreError};

/// Extension by MIME type (Store.cs:848-857). Lookup is case-insensitive
/// and ignores MIME parameters (`image/png; charset=…`).
const EXT_BY_MIME: [(&str, &str); 7] = [
    ("image/png", ".png"),
    ("image/jpeg", ".jpg"),
    ("image/webp", ".webp"),
    ("image/gif", ".gif"),
    ("video/mp4", ".mp4"),
    ("video/webm", ".webm"),
    ("video/quicktime", ".mov"),
];

/// `MediaStore` (Store.cs:862-910).
#[derive(Debug, Clone)]
pub struct MediaStore {
    root: PathBuf,
}

impl MediaStore {
    /// Creates the root directory, like the .NET constructor.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| StoreError::io_path(&root, e))?;
        Ok(MediaStore { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `MediaStore.Save` (Store.cs:868-874): the file name is
    /// `$"{prefix}_{Guid:N}"[..20] + ext` — with the default prefix `m`
    /// that is `m_` + 18 hex chars + the extension.
    pub fn save(&self, data: &[u8], mime: &str, prefix: &str) -> Result<String, StoreError> {
        let ext = ext_for_mime(mime);
        let base = format!("{prefix}_{}", new_guid_n());
        let truncated: String = base.chars().take(20).collect();
        let name = format!("{truncated}{ext}");
        let path = self.root.join(&name);
        std::fs::write(&path, data).map_err(|e| StoreError::io_path(&path, e))?;
        Ok(name)
    }

    /// `MediaStore.PathOf` (Store.cs:876-883): the name must match
    /// `^[A-Za-z0-9_\-]+\.[A-Za-z0-9]+$` and the file must exist, else
    /// "media not found".
    pub fn path_of(&self, name: &str) -> Result<PathBuf, StoreError> {
        if !valid_media_name(name) {
            return Err(StoreError::not_found("media not found"));
        }
        let path = self.root.join(name);
        if !path.is_file() {
            return Err(StoreError::not_found("media not found"));
        }
        Ok(path)
    }

    /// `MediaStore.MimeOf` (Store.cs:885-888): MIME by file extension
    /// (case-insensitive), else `application/octet-stream`.
    pub fn mime_of(name: &str) -> &'static str {
        let ext = Path::new(name)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        for (mime, e) in EXT_BY_MIME {
            if e == ext {
                return mime;
            }
        }
        "application/octet-stream"
    }

    /// `MediaStore.Prune` (Store.cs:890-905): delete files whose last
    /// write time is older than `max_age_seconds`; `<= 0` deletes nothing.
    /// Returns the number removed; per-file failures are ignored, like .NET.
    pub fn prune(&self, max_age_seconds: f64) -> usize {
        if max_age_seconds <= 0.0 {
            return 0;
        }
        let cutoff = SystemTime::now() - Duration::from_secs_f64(max_age_seconds);
        let mut removed = 0;
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let Ok(mtime) = meta.modified() else {
                continue;
            };
            if mtime < cutoff && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// Prune by retention **days** (what `Settings.MediaRetentionDays`
    /// carries): `days * 86400` seconds.
    pub fn prune_retention_days(&self, days: f64) -> usize {
        self.prune(days * 86400.0)
    }
}

fn ext_for_mime(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    for (m, ext) in EXT_BY_MIME {
        if m == base {
            return ext;
        }
    }
    ".bin"
}

/// The `PathOf` name regex, hand-rolled (no regex crate needed for this):
/// `^[A-Za-z0-9_\-]+\.[A-Za-z0-9]+$`.
fn valid_media_name(name: &str) -> bool {
    let mut parts = name.splitn(2, '.');
    let stem = parts.next().unwrap_or("");
    let ext = parts.next().unwrap_or("");
    !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !ext.is_empty()
        && !ext.contains('.')
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
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
    fn save_and_serve_roundtrip() {
        let dir = tmpdir("media");
        let store = MediaStore::new(dir.join("media")).unwrap();
        let name = store.save(b"\x89PNG\r\n", "image/png", "m").unwrap();
        // m_ + 18 hex + .png  (".NET: $\"{prefix}_{Guid:N}\"[..20] + ext")
        assert!(name.starts_with("m_"));
        assert!(name.ends_with(".png"));
        assert_eq!(name.len(), 20 + 4);
        assert_eq!(store.path_of(&name).unwrap().file_name().unwrap(), name.as_str());
        assert_eq!(MediaStore::mime_of(&name), "image/png");
        assert_eq!(MediaStore::mime_of("x.JPG"), "image/jpeg");
        assert_eq!(MediaStore::mime_of("x.unknown"), "application/octet-stream");
        // Unknown MIME -> .bin
        let bin = store.save(b"??", "application/pdf", "m").unwrap();
        assert!(bin.ends_with(".bin"));
        // MIME parameters are ignored.
        let jpg = store.save(b"xx", "image/jpeg; charset=binary", "m").unwrap();
        assert!(jpg.ends_with(".jpg"));
    }

    #[test]
    fn path_of_rejects_bad_names() {
        let dir = tmpdir("media-bad");
        let store = MediaStore::new(dir.join("media")).unwrap();
        for bad in ["../secret.png", "a/b.png", "noext", ".png", "a.b.c.png", "a b.png", ""] {
            assert!(store.path_of(bad).is_err(), "{bad} should be rejected");
        }
        assert!(store.path_of("missing.png").is_err());
    }

    #[test]
    fn prune_by_age() {
        let dir = tmpdir("media-prune");
        let store = MediaStore::new(dir.join("media")).unwrap();
        let old = store.save(b"old", "image/png", "m").unwrap();
        let fresh = store.save(b"new", "image/png", "m").unwrap();
        // Backdate the old file by 2 days.
        let old_path = store.path_of(&old).unwrap();
        let two_days_ago = SystemTime::now() - Duration::from_secs(2 * 86400);
        let f = std::fs::File::options().write(true).open(&old_path).unwrap();
        f.set_modified(two_days_ago).unwrap();
        drop(f);
        assert_eq!(store.prune(86400.0), 1);
        assert!(store.path_of(&old).is_err());
        assert!(store.path_of(&fresh).is_ok());
        assert_eq!(store.prune(0.0), 0);
        assert_eq!(store.prune_retention_days(14.0), 0);
    }
}
