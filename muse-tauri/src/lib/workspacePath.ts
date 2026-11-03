// Workspace path normalisation (defect B, 3 Nov 2026).
//
// Root cause: on Windows, Rust `Path::canonicalize()` returns the verbatim
// form `\\?\C:\...`. That string was written back into settings, and the
// conversation list is keyed by the RAW path string — so the same folder
// suddenly looked like a different workspace and its conversations
// "disappeared" (they were still stored under the old key).
//
// Every place that compares, stores, or keys off a workspace path must go
// through here, so the canonical form and the verbatim form are the SAME
// workspace. Rules:
//   - strip the verbatim prefix `\\?\` (and `\\?\UNC\server\share` ->
//     `\\server\share`);
//   - Windows-looking paths (drive letter or UNC): unify `/` -> `\`,
//     uppercase the drive letter, drop a trailing separator;
//   - anything else (Unix paths on Linux/dev): leave separators alone,
//     only trim + drop a trailing `/`;
//   - comparison / storage keys are case-insensitive on Windows paths
//     (NTFS is case-insensitive), exact otherwise.

export function isWindowsPath(p: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("\\\\") || p.startsWith("\\\\?\\");
}

export function stripVerbatimPrefix(p: string): string {
  if (p.startsWith("\\\\?\\UNC\\")) return `\\\\${p.slice(8)}`;
  if (p.startsWith("\\\\?\\")) return p.slice(4);
  return p;
}

export function normalizeWorkspacePath(raw: string): string {
  let s = (raw || "").trim();
  if (!s) return s;
  s = stripVerbatimPrefix(s);
  if (isWindowsPath(s)) {
    s = s.replace(/\//g, "\\");
    // Collapse duplicate separators, but keep the leading `\\` of a UNC path.
    const unc = s.startsWith("\\\\");
    s = s.replace(/\\+/g, "\\");
    if (unc) s = `\\${s}`;
    if (/^[a-z]:\\/.test(s)) s = s[0].toUpperCase() + s.slice(1);
    if (s.length > 3 && s.endsWith("\\")) s = s.replace(/\\+$/, "");
  } else {
    if (s.length > 1 && s.endsWith("/")) s = s.replace(/\/+$/, "");
  }
  return s;
}

/** Canonical key for comparing / keying a workspace: normalized, and
 *  lowercased for Windows paths (drive `C:` vs `c:` = same folder). */
export function workspacePathKey(raw: string): string {
  const n = normalizeWorkspacePath(raw);
  return isWindowsPath(n) ? n.toLowerCase() : n;
}

export function sameWorkspacePath(a: string, b: string): boolean {
  return workspacePathKey(a) === workspacePathKey(b);
}
