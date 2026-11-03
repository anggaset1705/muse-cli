//! Goal tracking — port of `GoalRun.cs` (goal state) and the
//! `GoalVerification` receipt gate.
//!
//! The goal object itself is persisted in the session history
//! (`.NET ChatSession.Goal`); the verification state is per-turn only.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::intent;
use crate::parser::ToolCall;
use crate::tool_outcome;

/// A long-running objective the agent works towards across turns.
/// Serialized PascalCase like .NET (`SessionView`'s `goal` object);
/// snake_case spellings are accepted on read (integrasi 2 Nov 2026).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GoalRun {
    #[serde(alias = "objective")]
    pub objective: String,
    #[serde(alias = "status")]
    pub status: String,
    #[serde(alias = "summary")]
    pub summary: String,
    #[serde(default, alias = "updated_at")]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "pending_question")]
    pub pending_question: Option<String>,
    #[serde(default, alias = "follow_ups")]
    pub follow_ups: Vec<String>,
}

impl Default for GoalRun {
    fn default() -> Self {
        Self {
            objective: String::new(),
            status: "paused".to_string(),
            summary: "Not verified yet.".to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            pending_question: None,
            follow_ups: Vec::new(),
        }
    }
}

impl GoalRun {
    pub fn describe_status(status: &str) -> String {
        match status {
            "running" => "Running".to_string(),
            "completed" => "Verified and complete".to_string(),
            "waiting_input" => "Waiting for more information from you".to_string(),
            _ => "Paused".to_string(),
        }
    }

    /// `/goal …` command (`.NET GoalRun.IsCommand`).
    pub fn is_command(text: &str) -> bool {
        text.len() >= 5
            && text[..5].eq_ignore_ascii_case("/goal")
            && (text.len() == 5 || text.as_bytes()[5].is_ascii_whitespace())
    }

    pub fn is_local_command(text: &str) -> bool {
        Self::is_command(text)
            && matches!(
                text[5..].trim().to_lowercase().as_str(),
                "status" | "stop"
            )
    }

    /// Does this message continue a waiting/paused goal?
    pub fn accepts_follow_up(&self, text: &str) -> bool {
        !Self::is_command(text)
            && ((self.status == "waiting_input"
                && self.pending_question.as_deref().map(|q| !q.trim().is_empty()).unwrap_or(false)
                && !intent::is_thanks_or_greeting(text)
                && !intent::is_explanation(text))
                || (self.status == "paused" && intent::is_continue(text)))
    }

    pub fn add_follow_up(&mut self, text: &str) {
        self.follow_ups
            .push(text.chars().take(2000).collect());
        if self.follow_ups.len() > 8 {
            let skip = self.follow_ups.len() - 8;
            self.follow_ups.drain(..skip);
        }
    }

    /// The prompt sent to the planner for this goal.
    pub fn task_prompt(&self) -> String {
        if self.follow_ups.is_empty() {
            self.objective.clone()
        } else {
            format!(
                "{}\n\nAdditional user information for the current goal:\n{}",
                self.objective,
                self.follow_ups.join("\n")
            )
        }
    }

    /// Goal instructions with the reply language filled in
    /// (`.NET GoalRun.InstructionsFor`).
    pub fn instructions_for(reply_language: &str) -> String {
        const TEMPLATE: &str = r#"GOAL MODE: Keep implementing the objective until verified, without inventing a step budget.
Inspect the repository and establish a concise acceptance/test checklist. Implement, run ALL relevant
automated test suites, fix failures, and repeat. Use real test runners (dotnet test, pytest, npm test,
cargo test, go test, etc.), not echo/printed claims. Discover all relevant suites; a filtered subset
does not establish completion. Add meaningful missing tests for the requested behavior.
Launch the resulting application using run_command background:true when needed. If the app to test
is not open yet, launch it first the same way (for example `start <url>` opens the browser at that
page), then computer_inspect — the newly opened window is auto-selected. Only ask the user to pick
a window when the app is already open but the wrong window was selected. Then use the local
computer tools to inspect the user-authorized test window, perform the actual feature workflows,
assert expected results with computer_assert and capture computer_screenshot AFTER the final action.
The local host shows a live preview of the authorized window; never bypass its window chooser.
After any source edit rerun checks. Use process_status wait_ms:30000 while a check is running;
starting a process is not a passing test. Report actual test counts, failures and remaining issues.
Do not mark done based on a plan, old receipts, empty/zero tests, or skipped/cancelled tests.
If access, credentials, screen consent or requirements are missing, ask a concrete question and pause.
Do not retry network/rate-limit errors, start extra conversations, or claim perfect/bug-free software.
Brief progress in __REPLY_LANGUAGE__ only; detailed logs belong in the local Terminal."#;
        TEMPLATE.replace("__REPLY_LANGUAGE__", reply_language)
    }
}

/// Fresh receipts gate completion; model text cannot set a check to passed
/// (`.NET GoalVerification`).
#[derive(Debug, Default)]
pub struct GoalVerification {
    revision: i64,
    tests: bool,
    interacted: bool,
    asserted: bool,
    captured: bool,
    processes: HashMap<String, (String, String, i64)>,
    failed_checks: HashSet<String>,
    failed_assertions: HashSet<String>,
    pub last_test_summary: Option<String>,
}

impl GoalVerification {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks still missing before the goal may be called complete.
    pub fn missing(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if !self.tests {
            missing.push(
                "Run all relevant test suites: real tests with exit_code=0 after the last edit"
                    .to_string(),
            );
        }
        if !self.failed_checks.is_empty() {
            missing.push(
                "Re-run and fix the test commands that still fail or are unverified".to_string(),
            );
        }
        if self
            .processes
            .values()
            .any(|(command, _, _)| Self::is_test_command(command))
        {
            missing.push("Wait for the test processes to finish".to_string());
        }
        if !self.interacted {
            missing.push("Test directly in the authorized window".to_string());
        }
        if !self.asserted || !self.failed_assertions.is_empty() {
            missing.push(
                "Verify the UI result with computer_assert after the last action".to_string(),
            );
        }
        if !self.captured {
            missing.push("Capture the UI result after the last action".to_string());
        }
        missing
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "source_revision": self.revision,
            "automated_tests_passed": self.tests,
            "test_summary": self.last_test_summary,
            "ui_interacted": self.interacted,
            "ui_asserted": self.asserted,
            "ui_captured": self.captured,
            "failed_checks": self.failed_checks.iter().collect::<Vec<_>>(),
            "missing": self.missing(),
        })
    }

    /// Record one tool result (`.NET GoalVerification.Record`).
    pub fn record(
        &mut self,
        call: &ToolCall,
        result: &str,
        files_changed: bool,
        image_captured: bool,
    ) {
        if files_changed {
            self.revision += 1;
            self.tests = false;
            self.interacted = false;
            self.asserted = false;
            self.captured = false;
            self.last_test_summary = None;
        }
        let failed = tool_outcome::is_failure(result);
        let command = if call.name == "run_command" {
            crate::error::json::str_arg(&call.args, "command", "")
        } else {
            String::new()
        };
        let command = if command.is_empty() {
            None
        } else {
            Some(command)
        };
        let workdir = crate::error::json::str_arg(&call.args, "workdir", ".");
        let revision = self.revision;
        let id_re = Regex::new(r"(?m)^process_id=([a-f0-9]+)\s*$").expect("process id regex");
        let id = id_re
            .captures(result)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .or_else(|| crate::error::json::opt_str_arg(&call.args, "process_id"));
        let mut command = command;
        let mut workdir = workdir;
        let mut rev = revision;
        if call.name == "run_command" {
            if let Some(id) = &id {
                if tool_outcome::is_running(result) {
                    self.processes
                        .insert(id.clone(), (command.clone().unwrap_or_default(), workdir.clone(), revision));
                }
            }
        } else if matches!(
            call.name.as_str(),
            "process_status" | "process_input" | "process_stop"
        ) {
            if let Some(id) = &id {
                if let Some((pending_command, pending_workdir, pending_rev)) =
                    self.processes.get(id).cloned()
                {
                    command = Some(pending_command);
                    workdir = pending_workdir;
                    rev = pending_rev;
                    if !tool_outcome::is_running(result) {
                        self.processes.remove(id);
                    }
                }
            }
        }
        if let Some(command) = &command {
            if Self::is_test_command(command) && !tool_outcome::is_running(result) {
                let key = format!(
                    "{}\ncwd={}",
                    strip_test_filter(command),
                    workdir
                );
                let filtered = Regex::new(r"(?i)(?:^|\s)(?:--filter|-k|-t|--testNamePattern)(?:\s|=|$)")
                    .expect("filter regex")
                    .is_match(command);
                if call.name != "process_stop"
                    && rev == self.revision
                    && tool_outcome::exited_successfully(result)
                    && Self::has_passing_tests(result)
                    && !filtered
                {
                    self.tests = true;
                    self.failed_checks.remove(&key);
                    self.last_test_summary = Some(Self::extract_test_summary(result));
                } else if !filtered || failed {
                    self.tests = false;
                    self.failed_checks.insert(key);
                }
            }
        }
        if intent::evidence(&call.name) & intent::work::COMPUTER_INTERACT != 0 {
            self.asserted = false;
            self.captured = false;
            if !failed {
                self.interacted = true;
            }
        }
        if call.name == "computer_assert" {
            let mut assertion = serde_json::Map::new();
            let mut keys: Vec<&String> = call
                .args
                .keys()
                .filter(|k| !matches!(k.as_str(), "element_id" | "wait_ms"))
                .collect();
            keys.sort();
            for key in keys {
                if let Some(value) = call.args.get(key) {
                    assertion.insert(key.clone(), value.clone());
                }
            }
            let key = serde_json::Value::Object(assertion).to_string();
            if failed || !result.starts_with("assertion_passed=true") {
                self.asserted = false;
                self.failed_assertions.insert(key);
            } else {
                self.asserted = true;
                self.failed_assertions.remove(&key);
            }
        }
        if call.name == "computer_screenshot" && !failed && image_captured {
            self.captured = true;
        }
    }

    /// Match an invoked runner at a command boundary, not a runner name
    /// quoted by echo (`.NET GoalVerification.IsTestCommand`).
    pub fn is_test_command(command: &str) -> bool {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(r#"(?im)(?:^|[;\n]|&&|\|\|)\s*(?:&\s*)?(?:["'](?:[^"'\r\n]*[\\/])?|[^\s"'\r\n]*[\\/])?(?:dotnet(?:\.exe)?["']?\s+test\b|(?:npm|pnpm|yarn)(?:\.cmd)?["']?\s+(?:run\s+)?test\b|(?:python(?:3|\.exe)?["']?\s+-m\s+)?pytest["']?\b|cargo["']?\s+test\b|go["']?\s+test\b|(?:mvn|gradle|gradlew)(?:\.cmd|\.bat)?["']?\s+test\b|(?:npx\s+)?(?:vitest|jest|playwright)["']?\s+(?:run|test)\b)"#).expect("test command regex")
        });
        re.is_match(command)
    }

    /// Does the output show passing tests and no failures
    /// (`.NET HasPassingTests`)?
    ///
    /// NOTE: the .NET failure pattern uses a negative lookahead
    /// (`FAIL(?:ED)?\b\s+(?!\d)[\w/]`), which the Rust `regex` crate does
    /// not support; the equivalent clause here requires the character
    /// after `FAIL` to be a letter/`_`/`/` instead of "not a digit".
    pub fn has_passing_tests(output: &str) -> bool {
        static FAIL_RE: OnceLock<Regex> = OnceLock::new();
        let fail_re = FAIL_RE.get_or_init(|| {
            Regex::new(r"(?i)\b(?:failed|failures|errors|skipped|cancelled|canceled)\s*[:=]?\s*[1-9]\d*|\b[1-9]\d*\s+(?:failed|failures|errors|skipped|cancelled|canceled)\b|\bfail\s+[1-9]\d*|\bFAIL(?:ED)?\s+[A-Za-z_/]").expect("fail regex")
        });
        if fail_re.is_match(output) {
            return false;
        }
        static PASS_RE: OnceLock<Regex> = OnceLock::new();
        let pass_re = PASS_RE.get_or_init(|| {
            Regex::new(r"(?i)\bpassed\s*[:=]\s*[1-9]\d*|\b[1-9]\d*\s+(?:passed|passing)\b|\bTests:\s*[1-9]\d*\s+passed|(?m)^ok\s+\S+\s+[\d.]+s|\bTests run:\s*[1-9]\d*,\s*Failures:\s*0,\s*Errors:\s*0|(?m)^[#ℹ]\s+pass [1-9]\d*\s*$").expect("pass regex")
        });
        pass_re.is_match(output)
    }

    fn extract_test_summary(result: &str) -> String {
        let re = Regex::new(r"(?i)passed|passing|Tests run:|^ok\s|^[#ℹ]\s+pass ").expect("summary regex");
        let matching: Vec<&str> = result.lines().filter(|line| re.is_match(line)).collect();
        let start = matching.len().saturating_sub(8);
        matching[start..]
            .iter()
            .map(|line| line.chars().take(250).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn strip_test_filter(command: &str) -> String {
    let re = Regex::new(r#"(?i)\s+(?:--filter|-k|-t|--testNamePattern)(?:=|\s+)(?:"[^"]*"|'[^']*'|\S+)"#)
        .expect("strip filter regex");
    re.replace_all(command, "").trim().to_string()
}
