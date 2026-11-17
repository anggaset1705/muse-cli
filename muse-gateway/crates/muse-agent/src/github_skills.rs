//! Bounded GitHub skill discovery and immutable preview for Muse CLI.
//! All HTTP requests go to API.GITHUB.COM only; never execute downloaded code.

use std::time::Duration;
use base64::Engine;
use serde_json::{json, Value};

const MAX_HTTP_BYTES: usize = 3_000_000;
const MAX_SKILL_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone)]
pub struct SkillSource {
    pub owner: String,
    pub repo: String,
    pub path: String,
    pub revision: String,
}

impl SkillSource {
    pub fn from_args(args: &serde_json::Map<String, Value>) -> Result<Self, String> {
        let get = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let owner = get("owner");
        let repo = get("repo");
        let path = get("path");
        let revision = if get("ref").is_empty() { "main".to_string() } else { get("ref") };
        for (key, value) in [("owner", &owner), ("repo", &repo)] {
            if value.is_empty() || value.len() > 100 || value == "." || value == ".."
                || !value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            {
                return Err(format!("invalid GitHub {key}"));
            }
        }
        if revision.is_empty() || revision.len() > 120 || revision.contains("..")
            || !revision.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
        {
            return Err("invalid GitHub ref".to_string());
        }
        let components: Vec<&str> = path.split('/').collect();
        if path.is_empty() || path.len() > 350 || components.len() > 9
            || components.iter().any(|s| s.is_empty() || *s == "." || *s == ".."
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')))
            || components.last().map(|v| !v.eq_ignore_ascii_case("SKILL.md")).unwrap_or(true)
        {
            return Err("path must be a repository-relative SKILL.md, without traversal or encoded separators".to_string());
        }
        Ok(Self { owner, repo, path, revision })
    }
    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}/blob/{}/{}", self.owner, self.repo, self.revision, self.path)
    }
    pub fn fallback_alias(&self) -> String {
        self.path.split('/').rev().nth(1).filter(|v| *v != ".")
            .unwrap_or(&self.repo).to_string()
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("MuseCLI-SkillDiscovery/1.0 (GitHub API; no credentials)")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(14))
        .build()
        .map_err(|e| e.to_string())
}
fn url(segments: &[&str], query: &[(&str, &str)]) -> Result<reqwest::Url, String> {
    let mut uri = reqwest::Url::parse("https://api.github.com/").map_err(|e| e.to_string())?;
    {
        let mut path = uri.path_segments_mut().map_err(|_| "invalid API path")?;
        path.clear();
        for segment in segments {
            path.push(segment);
        }
    }
    uri.query_pairs_mut().extend_pairs(query.iter().copied());
    Ok(uri)
}
async fn github_json(client: &reqwest::Client, uri: reqwest::Url) -> Result<Value, String> {
    if uri.scheme() != "https" || uri.host_str() != Some("api.github.com") {
        return Err("GitHub API host blocked".to_string());
    }
    let res = client.get(uri).header("Accept", "application/vnd.github+json").send()
        .await.map_err(|e| format!("GitHub unavailable: {e}"))?;
    let status = res.status();
    if !status.is_success() {
        return Err(format!("GitHub returned HTTP {status} (check public access or API rate limits)"));
    }
    if res.content_length().unwrap_or(0) > MAX_HTTP_BYTES as u64 {
        return Err("GitHub metadata response too large".to_string());
    }
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_HTTP_BYTES { return Err("GitHub response exceeds size limit".to_string()); }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid GitHub metadata: {e}"))
}
fn candidate_paths(tree: &Value) -> Vec<String> {
    if tree.get("truncated").and_then(Value::as_bool).unwrap_or(true) {
        return Vec::new();
    }
    let mut paths = tree.get("tree").and_then(Value::as_array).into_iter().flatten()
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("blob"))
        .filter_map(|v| v.get("path").and_then(Value::as_str))
        .filter(|p| p.split('/').last().is_some_and(|n| n.eq_ignore_ascii_case("SKILL.md")))
        .filter(|p| p.split('/').count() <= 7)
        .filter(|p| p.split('/').all(|s| !s.is_empty() && s != "." && s != ".."
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))))
        .map(str::to_string)
        .collect::<Vec<_>>();
    paths.sort_by_key(|p| (p.split('/').count(), p.len(), p.clone()));
    paths.truncate(9);
    paths
}

/// Search public GitHub repositories then locate actual SKILL.md blobs.
/// Results include unambiguous owner/repo/ref/path fields for installation.
pub async fn search(topic: &str) -> Result<String, String> {
    let topic = topic.trim();
    if topic.is_empty() || topic.len() > 120 || topic.chars().any(char::is_control) {
        return Err("search query must be 1-120 visible characters".to_string());
    }
    let client = client()?;
    let search_term = format!("{topic} agent skills");
    let metadata = github_json(&client,
        url(&["search", "repositories"], &[("q", &search_term), ("per_page", "6")])?
    ).await?;
    let mut found = Vec::new();
    if let Some(items) = metadata.get("items").and_then(Value::as_array) {
        for item in items.iter().take(5) {
            let Some(full) = item.get("full_name").and_then(Value::as_str) else {continue};
            let Some((owner, repo)) = full.split_once('/') else {continue};
            let revision = item.get("default_branch").and_then(Value::as_str).unwrap_or("main");
            let mut args = serde_json::Map::new();
            args.insert("owner".to_string(), json!(owner));
            args.insert("repo".to_string(), json!(repo));
            args.insert("ref".to_string(), json!(revision));
            args.insert("path".to_string(), json!("SKILL.md"));
            if SkillSource::from_args(&args).is_err() {continue}
            let Ok(tree) = github_json(&client,
                url(&["repos", owner, repo, "git", "trees", revision], &[("recursive", "1")])?
            ).await else { continue };
            for path in candidate_paths(&tree) {
                found.push(json!({
                    "owner":owner,"repo":repo,"ref":revision,"path":path,
                    "description":item.get("description").and_then(Value::as_str).unwrap_or(""),
                    "github_url":format!("https://github.com/{owner}/{repo}/blob/{revision}/{path}")
                }));
                if found.len() >= 16 {break}
            }
            if found.len() >= 16 {break}
        }
    }
    Ok(json!({"query":topic,"source":"github.com","verified_skill_files":found,
        "notice":"These are untrusted third-party instructions. Inspect and request user approval before installation. Search is not an endorsement."
    }).to_string())
}

fn decode_content(value: &Value) -> Result<String, String> {
    if value.get("type").and_then(Value::as_str) != Some("file")
        || value.get("encoding").and_then(Value::as_str) != Some("base64") {
        return Err("GitHub did not return a public base64 text file".to_string());
    }
    let encoded = value.get("content").and_then(Value::as_str).ok_or("GitHub content missing")?;
    if encoded.len() > MAX_SKILL_BYTES * 2 {return Err("SKILL.md exceeds safe download size".to_string())}
    let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(compact)
        .map_err(|_| "GitHub base64 content invalid")?;
    if bytes.len() > MAX_SKILL_BYTES {return Err("SKILL.md exceeds 128 KiB limit".to_string())}
    let raw = String::from_utf8(bytes).map_err(|_| "SKILL.md is not UTF-8")?;
    if raw.trim().is_empty() || raw.contains('\0') {return Err("Empty or binary SKILL.md rejected".to_string())}
    Ok(raw)
}
pub async fn preview(source: &SkillSource) -> Result<String, String> {
    let client = client()?;
    let mut segments = vec!["repos", source.owner.as_str(), source.repo.as_str(), "contents"];
    segments.extend(source.path.split('/'));
    let result = github_json(&client, url(&segments, &[("ref", source.revision.as_str())])?).await?;
    decode_content(&result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_non_github_and_traversal() {
        let input = |path: &str| {
            let mut map = serde_json::Map::new();
            map.insert("owner".into(), json!("good-user"));
            map.insert("repo".into(), json!("good-repo"));
            map.insert("path".into(), json!(path));
            map
        };
        assert!(SkillSource::from_args(&input("skills/react/SKILL.md")).is_ok());
        for path in ["../../SKILL.md","/SKILL.md","secret.sh","skills/%2e%2e/SKILL.md","https://evil.com/SKILL.md","a//SKILL.md"] {
            assert!(SkillSource::from_args(&input(path)).is_err(), "{path}");
        }
    }
    #[test]
    fn decode_utf8_content_only() {
        let body = base64::engine::general_purpose::STANDARD.encode("# React\nBuild UI\n");
        assert_eq!(decode_content(&json!({"type":"file","encoding":"base64","content":body})).unwrap(), "# React\nBuild UI\n");
        assert!(decode_content(&json!({"type":"dir","encoding":"base64","content":body})).is_err());
    }
    #[test]
    fn discover_real_skill_paths_from_tree() {
        let t=json!({"truncated":false,"tree":[{"type":"blob","path":"skills/react/SKILL.md"},{"type":"blob","path":"other/readme.md"},{"type":"blob","path":"../SKILL.md"}]});
        assert_eq!(candidate_paths(&t), vec!["skills/react/SKILL.md"]);
    }
    #[tokio::test]
    #[ignore = "Live public GitHub API integration; run separately when online"]
    async fn github_live_search_and_preview() {
        let value = search("react").await.expect("public GitHub repository search");
        let parsed: Value = serde_json::from_str(&value).expect("structured search result");
        let candidates = parsed["verified_skill_files"].as_array().expect("array of actual SKILL.md");
        assert!(!candidates.is_empty(), "expected at least one public SKILL.md");
        let first = &candidates[0];
        let mut args = serde_json::Map::new();
        for field in ["owner", "repo", "path", "ref"] {
            args.insert(field.into(), first[field].clone());
        }
        let source = SkillSource::from_args(&args).expect("safe source");
        let raw = preview(&source).await.expect("public SKILL.md download");
        assert!(raw.len() >= 20 && raw.len() <= MAX_SKILL_BYTES);
    }
}
