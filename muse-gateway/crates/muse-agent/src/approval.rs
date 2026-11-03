//! Approval policy — port of `TurboApprovalPolicy.cs`.
//!
//! Turbo auto-approves workspace development, including shell/process
//! operations. Credentials and agent/CI policy files still require
//! explicit review. In Ask mode every sensitive action
//! (`write_file` / `edit_file` / `run_command` / `process_input` /
//! `process_stop` — see `ToolExecutor::needs_confirmation`) needs approval;
//! Turbo only skips the approval step when this policy says the call does
//! not need explicit review.

use crate::parser::ToolCall;

/// Ask the user for every sensitive action, or auto-approve (Turbo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Sensitive actions need an explicit approval round-trip.
    Ask,
    /// Sensitive actions run immediately, except protected paths.
    Turbo,
}

/// `TurboApprovalPolicy.RequiresExplicitApproval`: true when even Turbo
/// must ask first. Mirrors the .NET rules exactly, including the protected
/// path components and file names.
pub fn requires_explicit_approval(call: &ToolCall) -> bool {
    let call = crate::parser::normalize(call);
    if matches!(
        call.name.as_str(),
        "run_command" | "process_input" | "process_stop"
    ) {
        return false;
    }
    if !matches!(call.name.as_str(), "write_file" | "edit_file") {
        return false;
    }
    let path = crate::error::json::str_arg(&call.args, "path", "")
        .replace('\\', "/")
        .trim()
        .to_string();
    if path.is_empty() {
        return true;
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return true;
    }
    // Windows-only: Win32 can resolve `AGENTS.md.` / `.github.` to the
    // protected originals, so ambiguous aliases must never be auto-approved.
    #[cfg(windows)]
    {
        if parts.iter().any(|part| {
            *part != "." && *part != ".."
                && (part.ends_with('.')
                    || part.ends_with(' ')
                    || (part.contains(':') && !(part.len() == 2 && part.as_bytes()[0].is_ascii_alphabetic())))
        }) {
            return true;
        }
    }
    if parts.iter().any(|part| {
        part.eq_ignore_ascii_case(".git")
            || part.eq_ignore_ascii_case(".github")
            || part.eq_ignore_ascii_case("secrets")
            || part.eq_ignore_ascii_case("credentials")
            || part.eq_ignore_ascii_case("browser-sessions")
    }) {
        return true;
    }
    let name = parts[parts.len() - 1];
    name.eq_ignore_ascii_case(".env")
        || name.to_lowercase().starts_with(".env.")
        || name.eq_ignore_ascii_case("AGENTS.md")
        || name.to_lowercase().ends_with(".key")
        || name.to_lowercase().ends_with(".pem")
        || name.to_lowercase().ends_with(".pfx")
        || name.to_lowercase().ends_with(".p12")
        || name.eq_ignore_ascii_case("accounts.json")
        || name.eq_ignore_ascii_case("api_key")
}
