//! `ToolOutcome` — port of `ToolOutcome.cs`.
//!
//! Classifies a tool result string: still running, exited successfully, or
//! failed. The metadata is the part of the result before the first
//! `stdout:` / `stderr:` / `output (` boundary line.

use regex::Regex;
use std::sync::OnceLock;

fn boundary_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^(?:stdout:|stderr:|output \()").expect("boundary regex"))
}

fn running_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?im)^running=true\s*$").expect("running regex"))
}

fn exit_code_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^exit_code=(-?\d+)\s*$").expect("exit code regex"))
}

fn metadata(result: &str) -> &str {
    match boundary_regex().find(result) {
        Some(m) => &result[..m.start()],
        None => result,
    }
}

/// True when the result says a process is still running.
pub fn is_running(result: &str) -> bool {
    running_regex().is_match(metadata(result))
}

/// True when the result reports `exit_code=0` and is not a failure.
pub fn exited_successfully(result: &str) -> bool {
    let meta = metadata(result);
    let ok = exit_code_regex()
        .captures(meta)
        .map(|c| c.get(1).map(|m| m.as_str()) == Some("0"))
        .unwrap_or(false);
    ok && !is_failure(result)
}

/// True when the result is a failure: an `error:` prefix, a non-zero exit
/// code, or a timeout. A `stopped=true` result is never a failure.
pub fn is_failure(result: &str) -> bool {
    if result.starts_with("stopped=true\n") {
        return false;
    }
    if result.to_lowercase().starts_with("error:") {
        return true;
    }
    let meta = metadata(result);
    if let Some(code) = exit_code_regex().captures(meta) {
        if code.get(1).map(|m| m.as_str()) != Some("0") {
            return true;
        }
    }
    meta.contains("timed_out=true")
}
