//! `skills/`: `SkillStore` (Skills.cs).
//!
//! Layout inside the store directory (Skills.cs:14-17): single-file skills
//! live as `<alias>.md`, folder skills as `<alias>/SKILL.md`, and
//! `skills.json` keeps the disabled alias list. Bad files are skipped on
//! load instead of breaking the whole registry.
//!
//! One porting note: `Skills.Mentions` uses the regex
//! `(?<![A-Za-z0-9_@])@([A-Za-z0-9][A-Za-z0-9_-]{0,39})`. The Rust `regex`
//! crate does not support look-around, so the mention scan runs the same
//! pattern without the lookbehind and rejects matches whose preceding
//! character is in `[A-Za-z0-9_@]` by hand. The observable behaviour is
//! identical (verified by the `mentions` test, including the `a@b.com`
//! email case).

use std::io::Read;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{new_guid_n, StoreError};

pub const MAX_TEXT_BYTES: u64 = 1024 * 1024;
pub const MAX_ZIP_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_ZIP_ENTRIES: usize = 200;
pub const MAX_CONTENT_CHARS: usize = 12000;
pub const MAX_EXPANDED_CHARS: usize = 30000;

const STATE_FILE: &str = "skills.json";

/// One installed skill (Skills.cs:12).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub alias: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub source_path: String,
    pub is_folder: bool,
}

/// `SkillStore.IsValidAlias` (Skills.cs:33-39).
pub fn is_valid_alias(alias: &str) -> bool {
    if alias.is_empty() || alias.len() > 40 {
        return false;
    }
    let mut chars = alias.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    alias
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// `SkillStore.Slugify` (Skills.cs:41-56).
pub fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true; // trims leading separators
    for raw in value.trim().to_lowercase().chars() {
        if raw.is_ascii_lowercase() || raw.is_ascii_digit() || raw == '_' {
            out.push(raw);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// `SkillStore.Parse` (Skills.cs:62-106): split a skill document into
/// frontmatter metadata and body. Returns `(alias, name, description, body)`.
pub fn parse_skill(
    text: &str,
    fallback_alias: &str,
) -> Result<(String, String, String, String), StoreError> {
    let normalized = text.replace("\r\n", "\n");
    let mut alias = String::new();
    let mut name = String::new();
    let mut description = String::new();
    let mut body = normalized.clone();
    if normalized.starts_with("---\n") {
        if let Some(end) = normalized[3..].find("\n---").map(|i| i + 3) {
            if end > 0 {
                for line in normalized[4..end].split('\n') {
                    let Some(colon) = line.find(':') else {
                        continue;
                    };
                    if colon == 0 {
                        continue;
                    }
                    let key = line[..colon].trim().to_ascii_lowercase();
                    let value = line[colon + 1..]
                        .trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .to_string();
                    if key == "alias" || key == "slug" {
                        alias = value;
                    } else if key == "name" {
                        name = value;
                    } else if key == "description" {
                        description = value;
                    }
                }
                let after = &normalized[end + 4..];
                body = after
                    .strip_prefix('\n')
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| after.to_string());
            }
        }
    }
    if name.is_empty() {
        for line in body.split('\n') {
            if let Some(rest) = line.strip_prefix("# ") {
                name = rest.trim().to_string();
                break;
            }
        }
    }
    if description.is_empty() {
        for line in body.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            description = if trimmed.len() <= 160 {
                trimmed.to_string()
            } else {
                trimmed.chars().take(160).collect()
            };
            break;
        }
    }
    // Precedence: explicit frontmatter alias, then the file/folder name the
    // user chose, then the document title (Skills.cs:100-103).
    let mut final_alias = slugify(if !alias.is_empty() { &alias } else { fallback_alias });
    if !is_valid_alias(&final_alias) {
        final_alias = slugify(&name);
    }
    if !is_valid_alias(&final_alias) {
        return Err(StoreError::invalid(
            "Could not determine a valid short name (alias) for the skill. \
             Set a frontmatter alias: or use a lowercase file name, e.g. fe.md.",
        ));
    }
    let name = if name.is_empty() {
        final_alias.clone()
    } else {
        name
    };
    Ok((final_alias, name, description, body.trim().to_string()))
}

/// File-backed skill registry (Skills.cs:19-31).
#[derive(Debug, Clone)]
pub struct SkillStore {
    directory: PathBuf,
}

impl SkillStore {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let directory = directory.into();
        if directory.as_os_str().is_empty() {
            return Err(StoreError::invalid("Missing skills directory."));
        }
        Ok(SkillStore {
            directory: std::path::absolute(&directory)
                .map_err(|e| StoreError::IoPlain(e))?,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn state_path(&self) -> PathBuf {
        self.directory.join(STATE_FILE)
    }

    fn disabled_aliases(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.state_path()) else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<String>>(&text).unwrap_or_default()
    }

    fn save_disabled(&self, disabled: &[String]) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.directory)
            .map_err(|e| StoreError::io_path(&self.directory, e))?;
        let mut seen: Vec<String> = Vec::new();
        for a in disabled {
            if !seen.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                seen.push(a.clone());
            }
        }
        let json = serde_json::to_string(&seen).map_err(StoreError::JsonPlain)?;
        std::fs::write(self.state_path(), json)
            .map_err(|e| StoreError::io_path(self.state_path(), e))
    }

    pub fn is_enabled(&self, alias: &str) -> bool {
        !self
            .disabled_aliases()
            .iter()
            .any(|a| a.eq_ignore_ascii_case(alias))
    }

    /// `SkillStore.SetEnabled` (Skills.cs:147-153).
    pub fn set_enabled(&self, alias: &str, enabled: bool) -> Result<(), StoreError> {
        let mut disabled = self.disabled_aliases();
        disabled.retain(|a| !a.eq_ignore_ascii_case(alias));
        if !enabled {
            disabled.push(alias.to_ascii_lowercase());
        }
        self.save_disabled(&disabled)
    }

    /// `SkillStore.List` (Skills.cs:155-185): single-file skills, then
    /// folder skills; grouped by alias (case-insensitive, first wins),
    /// ordered by alias (case-insensitive). Broken files are skipped.
    pub fn list(&self) -> Vec<SkillDefinition> {
        let mut result: Vec<SkillDefinition> = Vec::new();
        if !self.directory.is_dir() {
            return result;
        }
        if let Ok(entries) = std::fs::read_dir(&self.directory) {
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension().map(|e| e == "md").unwrap_or(false)
                        && p
                            .file_name()
                            .map(|n| n.to_string_lossy() != STATE_FILE)
                            .unwrap_or(false)
                })
                .collect();
            files.sort();
            for file in files {
                let Ok(text) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let fallback = file
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let Ok((alias, name, description, body)) = parse_skill(&text, &fallback) else {
                    continue;
                };
                result.push(SkillDefinition {
                    alias,
                    name,
                    description,
                    content: body,
                    source_path: file.to_string_lossy().into_owned(),
                    is_folder: false,
                });
            }
            let mut folders: Vec<PathBuf> = std::fs::read_dir(&self.directory)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_dir())
                        .collect()
                })
                .unwrap_or_default();
            folders.sort();
            for folder in folders {
                let Some(skill_file) = std::fs::read_dir(&folder).ok().and_then(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .find(|p| {
                            p.is_file()
                                && p
                                    .file_name()
                                    .map(|n| n.to_string_lossy().eq_ignore_ascii_case("SKILL.md"))
                                    .unwrap_or(false)
                        })
                }) else {
                    continue;
                };
                let Ok(text) = std::fs::read_to_string(&skill_file) else {
                    continue;
                };
                let fallback = folder
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let Ok((alias, name, description, body)) = parse_skill(&text, &fallback) else {
                    continue;
                };
                result.push(SkillDefinition {
                    alias,
                    name,
                    description,
                    content: body,
                    source_path: skill_file.to_string_lossy().into_owned(),
                    is_folder: true,
                });
            }
        }
        // Group by alias (case-insensitive, first wins), order by alias.
        let mut out: Vec<SkillDefinition> = Vec::new();
        for s in result {
            if !out.iter().any(|x| x.alias.eq_ignore_ascii_case(&s.alias)) {
                out.push(s);
            }
        }
        out.sort_by(|a, b| a.alias.to_lowercase().cmp(&b.alias.to_lowercase()));
        out
    }

    pub fn get(&self, alias: &str) -> Option<SkillDefinition> {
        self.list()
            .into_iter()
            .find(|s| s.alias.eq_ignore_ascii_case(alias))
    }

    /// `SkillStore.Import` (Skills.cs:189-209): `.md`/`.txt` single file,
    /// or `.zip` holding a `SKILL.md`. Importing an existing alias
    /// replaces the old version.
    pub fn import(&self, source_path: &Path) -> Result<SkillDefinition, StoreError> {
        if !source_path.is_file() {
            return Err(StoreError::invalid(
                "Could not find the selected skill file.",
            ));
        }
        let extension = source_path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        std::fs::create_dir_all(&self.directory)
            .map_err(|e| StoreError::io_path(&self.directory, e))?;
        if extension == "md" || extension == "txt" {
            let len = std::fs::metadata(source_path)
                .map(|m| m.len())
                .unwrap_or(0);
            if len > MAX_TEXT_BYTES {
                return Err(StoreError::invalid("The skill file exceeds 1 MiB."));
            }
            let text = read_text_file(source_path)?;
            let fallback = source_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (alias, name, description, body) = parse_skill(&text, &fallback)?;
            std::fs::write(self.directory.join(format!("{alias}.md")), &text)
                .map_err(|e| StoreError::io_path(self.directory.join(format!("{alias}.md")), e))?;
            self.remove_folder_skill(&alias);
            return Ok(self.get(&alias).unwrap_or(SkillDefinition {
                alias: alias.clone(),
                name,
                description,
                content: body,
                source_path: self
                    .directory
                    .join(format!("{alias}.md"))
                    .to_string_lossy()
                    .into_owned(),
                is_folder: false,
            }));
        }
        if extension == "zip" {
            return self.import_zip(source_path);
        }
        Err(StoreError::invalid(
            "Unsupported format. Choose a .md or .txt file, or a .zip package containing SKILL.md.",
        ))
    }

    fn import_zip(&self, source_path: &Path) -> Result<SkillDefinition, StoreError> {
        let len = std::fs::metadata(source_path)
            .map(|m| m.len())
            .unwrap_or(0);
        if len > MAX_ZIP_BYTES {
            return Err(StoreError::invalid("The skill package exceeds 10 MiB."));
        }
        let file = std::fs::File::open(source_path)
            .map_err(|e| StoreError::io_path(source_path, e))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| StoreError::invalid(format!("Could not read the skill package: {e}")))?;
        if archive.len() > MAX_ZIP_ENTRIES {
            return Err(StoreError::invalid("The skill package has too many files."));
        }
        // Find SKILL.md (case-insensitive file name), shallowest first.
        let mut skill_idx: Option<usize> = None;
        let mut skill_depth = usize::MAX;
        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|e| StoreError::invalid(format!("Could not read the skill package: {e}")))?;
            let name = entry.name().to_string();
            let base = name.rsplit('/').next().unwrap_or(&name);
            if base.eq_ignore_ascii_case("SKILL.md") {
                let depth = name.matches('/').count();
                if depth < skill_depth {
                    skill_depth = depth;
                    skill_idx = Some(i);
                }
            }
        }
        let skill_idx = skill_idx.ok_or_else(|| {
            StoreError::invalid("The .zip package must contain a SKILL.md file.")
        })?;
        let skill_name = {
            let entry = archive.by_index(skill_idx).map_err(|e| {
                StoreError::invalid(format!("Could not read the skill package: {e}"))
            })?;
            entry.name().to_string()
        };
        let prefix = skill_name[..skill_name.len() - "SKILL.md".len()].to_string();
        let mut text = String::new();
        {
            let mut entry = archive.by_index(skill_idx).map_err(|e| {
                StoreError::invalid(format!("Could not read the skill package: {e}"))
            })?;
            entry
                .read_to_string(&mut text)
                .map_err(|e| StoreError::invalid(format!("Could not read SKILL.md: {e}")))?;
        }
        let fallback = prefix
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("skill")
            .to_string();
        let (alias, _, _, _) = parse_skill(&text, &fallback)?;
        let temp = self.directory.join(format!(".import-{}", new_guid_n()));
        std::fs::create_dir_all(&temp).map_err(|e| StoreError::io_path(&temp, e))?;
        let result = (|| -> Result<(), StoreError> {
            let mut total: u64 = 0;
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).map_err(|e| {
                    StoreError::invalid(format!("Could not read the skill package: {e}"))
                })?;
                let full = entry.name().to_string();
                if !full.starts_with(&prefix) {
                    continue;
                }
                let relative = &full[prefix.len()..];
                if relative.is_empty() || relative.ends_with('/') {
                    continue;
                }
                total += entry.size();
                if total > MAX_ZIP_BYTES {
                    return Err(StoreError::invalid(
                        "The extracted skill package exceeds 10 MiB.",
                    ));
                }
                let destination = temp.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
                // Path safety: the destination must stay inside the temp dir.
                let canonical_temp = std::path::absolute(&temp)
                    .map_err(|e| StoreError::IoPlain(e))?;
                let canonical_dest = std::path::absolute(&destination)
                    .map_err(|e| StoreError::IoPlain(e))?;
                if !canonical_dest.starts_with(&canonical_temp) {
                    return Err(StoreError::invalid(
                        "The skill package contains an unsafe path.",
                    ));
                }
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| StoreError::io_path(parent, e))?;
                }
                let mut out = std::fs::File::create(&destination)
                    .map_err(|e| StoreError::io_path(&destination, e))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| StoreError::io_path(&destination, e))?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(e);
        }
        let single = self.directory.join(format!("{alias}.md"));
        if single.is_file() {
            let _ = std::fs::remove_file(&single);
        }
        self.remove_folder_skill(&alias);
        let dest = self.directory.join(&alias);
        match std::fs::rename(&temp, &dest) {
            Ok(()) => {}
            Err(e) => {
                let _ = std::fs::remove_dir_all(&temp);
                return Err(StoreError::io_path(&dest, e));
            }
        }
        self.get(&alias).ok_or_else(|| {
            StoreError::invalid("Extracted the package but could not read the skill back.")
        })
    }

    fn remove_folder_skill(&self, alias: &str) {
        let folder = self.directory.join(alias);
        if folder.is_dir() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }

    /// `SkillStore.Remove` (Skills.cs:240-247): deletes the single file
    /// and/or the folder; a future re-import starts enabled.
    pub fn remove(&self, alias: &str) -> Result<bool, StoreError> {
        let mut removed = false;
        let file = self.directory.join(format!("{alias}.md"));
        if file.is_file() {
            std::fs::remove_file(&file).map_err(|e| StoreError::io_path(&file, e))?;
            removed = true;
        }
        let folder = self.directory.join(alias);
        if folder.is_dir() {
            self.remove_folder_skill(alias);
            removed = true;
        }
        if removed {
            self.set_enabled(alias, true)?;
        }
        Ok(removed)
    }

    /// `SkillStore.Mentions` (Skills.cs:249-261): `@alias` mentions in a
    /// prompt, in order, de-duplicated, lowercased.
    pub fn mentions(text: &str) -> Vec<String> {
        let re = Regex::new(r"@([A-Za-z0-9][A-Za-z0-9_-]{0,39})").unwrap();
        let mut found: Vec<String> = Vec::new();
        for m in re.find_iter(text) {
            // Manual lookbehind: the char before the '@' must not be
            // [A-Za-z0-9_@] (the `regex` crate has no look-around).
            let before = text[..m.start()]
                .chars()
                .next_back()
                .unwrap_or(' ');
            if before.is_ascii_alphanumeric() || before == '_' || before == '@' {
                continue;
            }
            let alias = m.as_str()[1..].to_ascii_lowercase();
            if !found.contains(&alias) {
                found.push(alias);
            }
        }
        found
    }

    /// `SkillStore.Expand` (Skills.cs:263-285): build the instruction block
    /// for the skills a prompt invokes with `@alias`. Unknown or disabled
    /// skills are ignored. Returns `(block, applied_aliases)`; an empty
    /// block means no skill applied. Content over 12,000 chars is cut with
    /// a `\n[... skill content truncated ...]` marker; the whole block
    /// stops growing at 30,000 chars (a skill that no longer fits ends the
    /// loop, like .NET's `break`).
    pub fn expand(&self, prompt: &str) -> (String, Vec<String>) {
        let mut used: Vec<String> = Vec::new();
        let mut blocks: Vec<String> = Vec::new();
        let mut budget = MAX_EXPANDED_CHARS;
        for alias in Self::mentions(prompt) {
            let Some(skill) = self.get(&alias) else {
                continue;
            };
            if !self.is_enabled(&skill.alias) {
                continue;
            }
            let content_chars: Vec<char> = skill.content.chars().collect();
            let content = if content_chars.len() <= MAX_CONTENT_CHARS {
                skill.content.clone()
            } else {
                let cut: String = content_chars.iter().take(MAX_CONTENT_CHARS).collect();
                format!("{cut}\n[... skill content truncated ...]")
            };
            let mut block = format!("### Skill @{} — {}", skill.alias, skill.name);
            if !skill.description.is_empty() {
                block.push('\n');
                block.push_str(&skill.description);
            }
            block.push_str("\n\n");
            block.push_str(&content);
            if block.len() > budget {
                break;
            }
            budget -= block.len();
            blocks.push(block);
            used.push(skill.alias.clone());
        }
        if blocks.is_empty() {
            return (String::new(), used);
        }
        let header = "USER SKILLS REQUESTED FOR THIS TASK (@alias in the request). \
            Follow each skill's content; the app's approval and safety rules still apply.\n\n";
        (format!("{header}{}", blocks.join("\n\n")), used)
    }
}

/// Read a text file as UTF-8, stripping a BOM like .NET's `File.ReadAllText`.
fn read_text_file(path: &Path) -> Result<String, StoreError> {
    let text = std::fs::read_to_string(path).map_err(|e| StoreError::io_path(path, e))?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
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

    fn store(name: &str) -> (SkillStore, PathBuf) {
        let dir = tmpdir(name);
        let s = SkillStore::new(dir.join("skills")).unwrap();
        (s, dir)
    }

    #[test]
    fn parse_frontmatter_and_fallbacks() {
        let (alias, name, desc, body) = parse_skill(
            "---\nname: Frontend\nalias: fe\ndescription: Build UIs\n---\n# Ignored heading\nDo the thing.\n",
            "fallback",
        )
        .unwrap();
        assert_eq!(alias, "fe");
        assert_eq!(name, "Frontend");
        assert_eq!(desc, "Build UIs");
        assert_eq!(body, "# Ignored heading\nDo the thing.");

        // No frontmatter: first "# " heading is the name, file name is alias.
        let (alias, name, desc, body) =
            parse_skill("# Review\nCheck the code carefully.\n", "my-review").unwrap();
        assert_eq!(alias, "my-review");
        assert_eq!(name, "Review");
        assert_eq!(desc, "Check the code carefully.");
        assert_eq!(body, "# Review\nCheck the code carefully.");

        // A heading must not rename the skill when the file name was chosen.
        let (alias, _, _, _) = parse_skill("# Review\nBody.\n", "my-review").unwrap();
        assert_eq!(alias, "my-review");

        // Quoted frontmatter values + slug alias.
        let (alias, _, _, _) = parse_skill(
            "---\nalias: \"My Skill\"\n---\nBody\n",
            "fallback",
        )
        .unwrap();
        assert_eq!(alias, "my-skill");
        assert!(parse_skill("", "!!!").is_err());
    }

    #[test]
    fn slugify_and_alias_rules() {
        assert_eq!(slugify("Hello World!"), "hello-world");
        assert_eq!(slugify("  --a__b--  "), "a__b");
        assert!(is_valid_alias("fe"));
        assert!(is_valid_alias("a-1_b"));
        assert!(is_valid_alias("1a-b-c")); // first char may be a digit
        assert!(!is_valid_alias("FE")); // must be lowercase
        assert!(!is_valid_alias("-ab"));
        assert!(!is_valid_alias(""));
        assert!(!is_valid_alias("a".repeat(41).as_str()));
    }

    #[test]
    fn mentions_rules() {
        assert_eq!(SkillStore::mentions("use @fe and @seo please"), vec!["fe", "seo"]);
        assert_eq!(SkillStore::mentions("@fe @fe @FE"), vec!["fe"]);
        assert_eq!(SkillStore::mentions("mail a@b.com"), Vec::<String>::new());
        assert_eq!(SkillStore::mentions("x@@fe"), Vec::<String>::new());
        assert_eq!(SkillStore::mentions("(@fe)"), vec!["fe"]);
        assert_eq!(SkillStore::mentions("email@fe"), Vec::<String>::new());
    }

    #[test]
    fn list_enable_expand_remove() {
        let (s, _dir) = store("skills-list");
        std::fs::create_dir_all(s.directory()).unwrap();
        std::fs::write(
            s.directory().join("fe.md"),
            "---\nname: Frontend\ndescription: Build UIs\n---\nFollow the design system.\n",
        )
        .unwrap();
        std::fs::create_dir_all(s.directory().join("seo")).unwrap();
        std::fs::write(
            s.directory().join("seo").join("SKILL.md"),
            "# SEO\nRank the page.\n",
        )
        .unwrap();
        std::fs::write(s.directory().join("broken.md"), "---\nname: [\n").unwrap();
        // broken.md has no valid alias source? it parses with fallback "broken".
        let list = s.list();
        let aliases: Vec<&str> = list.iter().map(|x| x.alias.as_str()).collect();
        assert!(aliases.contains(&"fe"));
        assert!(aliases.contains(&"seo"));
        let seo = s.get("seo").unwrap();
        assert!(seo.is_folder);
        assert_eq!(seo.name, "SEO");
        let fe = s.get("FE").unwrap();
        assert!(!fe.is_folder);
        assert_eq!(fe.content, "Follow the design system.");

        let (block, applied) = s.expand("please use @fe and @seo");
        assert_eq!(applied, vec!["fe", "seo"]);
        assert!(block.starts_with("USER SKILLS REQUESTED FOR THIS TASK"));
        assert!(block.contains("### Skill @fe — Frontend"));
        assert!(block.contains("### Skill @seo — SEO"));

        s.set_enabled("fe", false).unwrap();
        assert!(!s.is_enabled("fe"));
        let (block, applied) = s.expand("use @fe");
        assert_eq!(block, "");
        assert!(applied.is_empty());
        s.set_enabled("fe", true).unwrap();
        assert!(s.is_enabled("fe"));

        assert!(s.remove("fe").unwrap());
        assert!(s.get("fe").is_none());
        assert!(!s.remove("fe").unwrap());
        assert!(s.is_enabled("fe")); // re-import starts enabled
    }

    #[test]
    fn import_md_and_zip() {
        let (s, dir) = store("skills-import");
        let src = dir.join("my-skill.md");
        std::fs::write(&src, "# My Skill\nDo it well.\n").unwrap();
        let imported = s.import(&src).unwrap();
        assert_eq!(imported.alias, "my-skill");
        assert!(s.directory().join("my-skill.md").exists());

        // Zip package with SKILL.md in a subfolder.
        let zip_path = dir.join("pack.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("pack/SKILL.md", opts).unwrap();
            zip.write_all(b"# Packed\nPacked content.\n").unwrap();
            zip.start_file("pack/extra.txt", opts).unwrap();
            zip.write_all(b"extra").unwrap();
            zip.finish().unwrap();
        }
        let imported = s.import(&zip_path).unwrap();
        assert_eq!(imported.alias, "pack");
        assert!(imported.is_folder);
        assert_eq!(imported.name, "Packed");
        // The "# " heading stays in the body; only the name is extracted.
        assert_eq!(imported.content, "# Packed\nPacked content.");
        assert!(s.directory().join("pack").join("extra.txt").exists());

        assert!(s.import(&dir.join("nope.md")).is_err());
        let bad = dir.join("bad.bin");
        std::fs::write(&bad, b"x").unwrap();
        assert!(s.import(&bad).is_err());
    }

    #[test]
    fn expand_truncates_long_content() {
        let (s, _dir) = store("skills-trunc");
        std::fs::create_dir_all(s.directory()).unwrap();
        let long = "x".repeat(13_000);
        std::fs::write(s.directory().join("big.md"), format!("# Big\n{long}")).unwrap();
        let (block, applied) = s.expand("use @big");
        assert_eq!(applied, vec!["big"]);
        assert!(block.contains("[... skill content truncated ...]"));
        assert!(block.len() <= MAX_EXPANDED_CHARS + 200);
    }
}
