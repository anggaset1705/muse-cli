//! Single-hunk review diff — port of `TextDiff.Create` in `ToolEditing.cs`.
//! Line endings are normalised for display; the hunk shows up to 3 lines
//! of context on each side.

/// Build a unified-style diff hunk for `path`, from `before` (`None` =
/// file did not exist) to `after`.
pub fn create(path: &str, before: Option<&str>, after: &str) -> String {
    let old_normalized = before.map(|text| text.replace("\r\n", "\n"));
    let old_lines: Vec<&str> = match &old_normalized {
        None => Vec::new(),
        Some(text) => text.split('\n').collect(),
    };
    let new_normalized = after.replace("\r\n", "\n");
    let new_lines: Vec<&str> = new_normalized.split('\n').collect();

    let mut prefix = 0;
    while prefix < old_lines.len().min(new_lines.len()) && old_lines[prefix] == new_lines[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_lines.len().min(new_lines.len()) - prefix
        && old_lines[old_lines.len() - (suffix + 1)] == new_lines[new_lines.len() - (suffix + 1)]
    {
        suffix += 1;
    }
    if prefix == old_lines.len() && prefix == new_lines.len() {
        return "(no changes)".to_string();
    }
    let start = prefix.saturating_sub(3);
    let old_end = (old_lines.len() - suffix + 3).min(old_lines.len());
    let new_end = (new_lines.len() - suffix + 3).min(new_lines.len());
    let old_start = if old_end == 0 { 0 } else { start + 1 };
    let mut text = format!(
        "--- {}\n+++ b/{}\n@@ -{},{} +{},{} @@\n",
        if before.is_none() {
            "/dev/null".to_string()
        } else {
            format!("a/{path}")
        },
        path,
        old_start,
        old_end - start,
        start + 1,
        new_end - start
    );
    for line in &old_lines[start..prefix] {
        text.push(' ');
        text.push_str(line);
        text.push('\n');
    }
    for line in &old_lines[prefix..(old_lines.len() - suffix)] {
        text.push('-');
        text.push_str(line);
        text.push('\n');
    }
    for line in &new_lines[prefix..(new_lines.len() - suffix)] {
        text.push('+');
        text.push_str(line);
        text.push('\n');
    }
    for line in &new_lines[(new_lines.len() - suffix)..new_end] {
        text.push(' ');
        text.push_str(line);
        text.push('\n');
    }
    text
}
