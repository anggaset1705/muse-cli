//! Workspace path handling — ports of `WorkspaceFiles.cs` and the
//! jail logic in `Tools.cs` (`Resolve` / `RejectLinks`).
//!
//! Every file path is jailed inside the workspace root: `..`, absolute
//! paths outside the root, and symlinked escapes are all rejected before
//! any read or write happens. Enumeration never traverses symlinks or
//! generated/dependency folders.

use std::path::{Component, Path, PathBuf};

use crate::error::{AgentError, AgentResult, CancellationToken};

/// Directories skipped while browsing (`.NET WorkspaceFiles.Ignored`).
pub const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".vs",
    ".idea",
    "bin",
    "obj",
    "artifacts",
    "node_modules",
    ".next",
    "dist",
    "coverage",
    "__pycache__",
    ".venv",
    "venv",
];

/// Lexically normalise an absolute path (resolve `.` and `..` without
/// touching the filesystem).
pub fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// Resolve `path` (relative to `root`, or absolute) and verify it stays
/// inside `root`. Symlinks in every existing path component are rejected.
pub fn resolve(root: &Path, path: &str) -> AgentResult<PathBuf> {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    let full = normalize_lexically(&joined);
    let root_norm = normalize_lexically(root);
    if full != root_norm && !full.starts_with(&root_norm) {
        return Err(AgentError::Invalid(format!(
            "path is outside the workspace: {path}"
        )));
    }
    reject_links(&full)?;
    Ok(full)
}

/// Reject the path if it, or any existing ancestor, is a symbolic link
/// (`.NET RejectLinks`; on Windows this also covers junctions — on Linux a
/// symlink check is the equivalent).
pub fn reject_links(full: &Path) -> AgentResult<()> {
    let mut current = Some(full);
    while let Some(path) = current {
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(AgentError::Invalid(format!(
                        "symbolic links and junctions are not allowed: {}",
                        path.display()
                    )));
                }
            }
            Err(_) => {
                // Missing component: keep walking up to existing ancestors.
            }
        }
        current = path.parent();
    }
    Ok(())
}

/// Relative path of `full` inside `root`, with `/` separators (`.NET
/// Path.GetRelativePath(...).Replace('\\', '/')`).
pub fn relative_to(root: &Path, full: &Path) -> String {
    let rel = full
        .strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .replace('\\', "/");
    rel
}

/// SHA-256 of a text, hex-encoded uppercase (`.NET ContentHash` /
/// `Convert.ToHexString`).
pub fn content_hash(text: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode_upper(hasher.finalize())
}

/// SHA-256 of raw bytes, hex-encoded uppercase.
pub fn content_hash_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hex::encode_upper(hasher.finalize())
}

/// Bounded project browsing; never traverses links or ignored folders.
/// Mirrors `WorkspaceFiles.Enumerate(root, recursive, cancel)` including
/// the 2000-directory visit cap and the files-first, case-insensitive
/// name ordering.
pub fn enumerate(
    root: &Path,
    recursive: bool,
    cancel: &CancellationToken,
) -> AgentResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut visited = 0;
    while let Some(directory) = pending.pop() {
        cancel.throw_if_cancelled()?;
        visited += 1;
        if visited > 2000 {
            break;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        let mut items: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        items.sort_by(|a, b| {
            let a_dir = a.is_dir();
            let b_dir = b.is_dir();
            a_dir
                .cmp(&b_dir)
                .then_with(|| {
                    a.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase()
                        .cmp(
                            &b.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_lowercase(),
                        )
                })
        });
        for entry in items {
            cancel.throw_if_cancelled()?;
            let attributes = match std::fs::symlink_metadata(&entry) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if attributes.file_type().is_symlink() {
                continue;
            }
            let is_dir = attributes.is_dir();
            if is_dir {
                let name = entry
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if IGNORED_DIRS
                    .iter()
                    .any(|ignored| ignored.eq_ignore_ascii_case(&name))
                {
                    continue;
                }
            }
            out.push(entry.clone());
            if recursive && is_dir {
                pending.push(entry);
            }
        }
        if !recursive {
            break;
        }
    }
    Ok(out)
}
