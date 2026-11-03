//! Chat turn — port of `AgentLoop.RunAsync` (the shared, cancellable
//! chat/tool loop) and the SSE event flow of `AgentEndpoints.ChatTurn`.
//!
//! One turn: build the planner messages (system prompt + workspace
//! context + history + user turn), ask the driver for a plan, parse the
//! JSON actions, run them through the executor with Ask/Turbo approval,
//! and feed tool receipts back until the model answers or the turn ends
//! (`completed` / `incomplete` / `cancelled` / `declined` /
//! `waiting_input`).
//!
//! The gateway owns HTTP/SSE framing; this module emits [`AgentEvent`]s
//! (plus `assistant_delta` events from the driver) through an
//! [`EventSink`], and the gateway formats them as `data: {json}\n\n`
//! frames followed by `done` + `[DONE]` (see `session::done_event`).

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use crate::approval;
use crate::context;
use crate::driver::{ChatRequest, DeltaSink, MuseDriver};
use crate::error::{AgentError, AgentResult, CancellationToken};
use crate::executor::{ToolApproval, ToolExecutor};
use crate::goal::{GoalRun, GoalVerification};
use crate::intent::{self, work};
use crate::parser::{self, ToolCall};
use crate::session::{is_turn_outcome, AgentEvent, AgentSession};
use crate::system_prompt;
use crate::tool_outcome;

/// Outcome of one chat turn (`.NET AgentLoop` return + last outcome).
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub ok: bool,
    pub status: String,
    pub summary: String,
}

impl TurnOutcome {
    pub fn completed(summary: impl Into<String>) -> Self {
        Self {
            ok: true,
            status: "completed".to_string(),
            summary: summary.into(),
        }
    }
    pub fn incomplete(summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            status: "incomplete".to_string(),
            summary: summary.into(),
        }
    }
    pub fn cancelled() -> Self {
        Self {
            ok: false,
            status: "cancelled".to_string(),
            summary: "Stopped as requested.".to_string(),
        }
    }
    pub fn declined(summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            status: "declined".to_string(),
            summary: summary.into(),
        }
    }
    pub fn waiting_input(summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            status: "waiting_input".to_string(),
            summary: summary.into(),
        }
    }
}

/// Receives turn events. The gateway forwards these to the SSE stream;
/// tests collect them in a `Vec`.
pub trait EventSink: Send {
    fn emit(&mut self, event: AgentEvent);
}

impl EventSink for Vec<AgentEvent> {
    fn emit(&mut self, event: AgentEvent) {
        self.push(event);
    }
}

impl<F> EventSink for F
where
    F: FnMut(AgentEvent) + Send,
{
    fn emit(&mut self, event: AgentEvent) {
        self(event);
    }
}

/// Options for one turn (`.NET ChatTurn` body fields that affect the loop).
#[derive(Debug, Clone, Default)]
pub struct TurnOptions {
    /// Streaming turn: approvals can be asked interactively. A
    /// non-interactive turn declines every sensitive action, like .NET.
    pub interactive: bool,
    /// Override the session reply language for this turn.
    pub reply_language: Option<String>,
    /// Override the session Turbo flag for this turn.
    pub turbo: Option<bool>,
}

/// Refusal message for screen control in read-only mode
/// (`.NET UiStrings.ReadOnlyScreenFirst`, E6): the WinForms host
/// refuses computer authorization while the Read-only permission mode
/// is on, so the loop reports the same refusal instead of running the
/// call. The model must not retry the action in this turn.
pub const READ_ONLY_SCREEN_MESSAGE: &str = "Screen control is disabled in read-only mode. Switch out of Read-only mode in Settings before turning on UI testing.";

fn lang(session: &AgentSession, english: &str, indonesian: &str) -> String {
    if session.reply_language == "Bahasa Indonesia" {
        indonesian.to_string()
    } else {
        english.to_string()
    }
}

/// Ask for confirmation of a sensitive action (`.NET
/// AgentSession.ConfirmAsync`): Turbo auto-approves unless the call
/// touches a protected path; a non-interactive turn declines; otherwise
/// an `approval_required` event is emitted and the answer is awaited on
/// the session's approval registry.
async fn confirm(
    session: &AgentSession,
    approval: &ToolApproval,
    cancel: &CancellationToken,
    sink: &mut dyn EventSink,
    interactive: bool,
) -> AgentResult<bool> {
    if session.turbo && !approval::requires_explicit_approval(&approval.call) {
        return Ok(true);
    }
    if !interactive {
        return Ok(false);
    }
    let approval_id = uuid::Uuid::new_v4().simple().to_string();
    let (tx, rx) = oneshot::channel::<bool>();
    session
        .approvals
        .lock()
        .unwrap()
        .insert(approval_id.clone(), tx);
    let mut payload = json!({
        "event": "approval_required",
        "approval_id": approval_id,
        "tool": approval.call.name,
        "description": approval.description,
        "preview": approval.preview,
    });
    if let Some(path) = &approval.file_path {
        payload["path"] = Value::String(path.clone());
    }
    sink.emit(AgentEvent {
        kind: "approval_required".to_string(),
        text: payload.to_string(),
        tool: Some(approval.call.name.clone()),
        path: approval.file_path.clone(),
    });
    let answered = tokio::select! {
        result = rx => result.unwrap_or(false),
        _ = cancel.cancelled() => {
            session.approvals.lock().unwrap().remove(&approval_id);
            return Err(AgentError::Cancelled);
        }
    };
    session.approvals.lock().unwrap().remove(&approval_id);
    Ok(answered)
}

/// Forwards driver deltas as `assistant_delta` events.
struct DeltaForwarder<'a> {
    sink: &'a mut dyn EventSink,
}

impl DeltaSink for DeltaForwarder<'_> {
    fn on_delta(&mut self, text: &str) {
        self.sink.emit(AgentEvent::new("assistant_delta", text));
    }
}


/// Turn event observer: records the turn outcome and forwards events to
/// the sink (mirrors the `_observe` wrapper in .NET `AgentLoop`).
struct Observer<'a> {
    sink: &'a mut dyn EventSink,
    outcome: Option<(String, String)>,
}

impl Observer<'_> {
    fn emit(&mut self, event: AgentEvent) {
        if is_turn_outcome(&event.kind) {
            self.outcome = Some((event.kind.clone(), event.text.clone()));
        }
        self.sink.emit(event);
    }
}

/// Run one chat turn on a session (`.NET AgentLoop.RunAsync`).
///
/// The caller owns `cancel` (and registers a clone with the manager so
/// the cancel route can stop the turn while it runs).
pub async fn run_turn(
    session: &mut AgentSession,
    prompt: &str,
    images: Vec<String>,
    driver: &dyn MuseDriver,
    sink: &mut dyn EventSink,
    options: TurnOptions,
    cancel: &CancellationToken,
) -> TurnOutcome {
    if prompt.trim().is_empty() && images.is_empty() {
        return TurnOutcome::incomplete("Content must not be empty.");
    }
    if prompt.chars().count() > 32_000 {
        return TurnOutcome::incomplete("Request too long (maximum 32000 characters).");
    }
    if session.running {
        return TurnOutcome::incomplete("Agent is already running");
    }
    if let Some(language) = &options.reply_language {
        if !language.is_empty() {
            session.reply_language = language.clone();
        }
    }
    if let Some(turbo) = options.turbo {
        session.turbo = turbo;
    }
    session.running = true;
    session.run_token = Some(cancel.clone());
    session.updated_at = chrono::Utc::now();
    if session.title == "New conversation" && !prompt.is_empty() {
        let trimmed = prompt.trim();
        session.title = trimmed.chars().take(60).collect();
    }

    let outcome = run_turn_inner(session, prompt, images, driver, sink, &options, cancel).await;

    session.running = false;
    session.run_token = None;
    session.updated_at = chrono::Utc::now();
    outcome
}

async fn run_turn_inner(
    session: &mut AgentSession,
    prompt: &str,
    images: Vec<String>,
    driver: &dyn MuseDriver,
    sink: &mut dyn EventSink,
    options: &TurnOptions,
    cancel: &CancellationToken,
) -> TurnOutcome {
    // -- turn state -------------------------------------------------------
    let mut obs = Observer { sink, outcome: None };

    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool: Option<String> = None;
    let mut verification: Option<GoalVerification> = None;
    let user_prompt = prompt.to_string();
    let mut prompt = prompt.to_string();

    let result: AgentResult<TurnOutcome> = (async {
        // -- goal commands -------------------------------------------------
        let goal_follow_up = session
            .goal
            .as_ref()
            .map(|g| g.accepts_follow_up(&prompt))
            .unwrap_or(false);
        if GoalRun::is_command(&prompt) || goal_follow_up {
            let argument = if goal_follow_up {
                String::new()
            } else {
                prompt[5..].trim().to_string()
            };
            if argument.eq_ignore_ascii_case("status") {
                let text = match &session.goal {
                    None => lang(session, "No goal yet. Type /goal with a goal to start.", "Belum ada tujuan. Ketik /goal beserta tujuan untuk memulai."),
                    Some(goal) => lang(session,
                        &format!("Goal: {}\nStatus: {}\n{}", goal.objective, GoalRun::describe_status(&goal.status), goal.summary),
                        &format!("Tujuan: {}\nStatus: {}\n{}", goal.objective, GoalRun::describe_status(&goal.status), goal.summary)),
                };
                obs.emit(AgentEvent::new("assistant", text));
                obs.emit(AgentEvent::new("completed", lang(session, "Viewed the goal status.", "Melihat status tujuan.")));
                return Ok(TurnOutcome::completed("Viewed the goal status."));
            }
            if argument.eq_ignore_ascii_case("stop") {
                let paused_summary = lang(session, "Paused. Type /goal to continue and verify again.", "Dijeda. Ketik /goal untuk melanjutkan dan memverifikasi lagi.");
                if let Some(goal) = &mut session.goal {
                    goal.status = "paused".to_string();
                    goal.pending_question = None;
                    goal.summary = paused_summary;
                    goal.updated_at = chrono::Utc::now().to_rfc3339();
                }
                obs.emit(AgentEvent::new("assistant", lang(session, "Paused the goal. The goal and its changes are kept.", "Tujuan dijeda. Tujuan dan perubahannya tetap disimpan.")));
                obs.emit(AgentEvent::new("completed", lang(session, "Paused the goal.", "Tujuan dijeda.")));
                return Ok(TurnOutcome::completed("Paused the goal."));
            }
            if !argument.is_empty() {
                session.goal = Some(GoalRun {
                    objective: argument,
                    ..GoalRun::default()
                });
            }
            let building_summary = lang(session, "Building and verifying the goal.", "Membangun dan memverifikasi tujuan.");
            let Some(goal) = &mut session.goal else {
                obs.emit(AgentEvent::new("assistant", lang(session, "Type /goal with a goal, for example: /goal Finish the sign-in screen and test the whole flow.", "Ketik /goal beserta tujuan, misalnya: /goal Selesaikan layar login dan uji seluruh alurnya.")));
                obs.emit(AgentEvent::new("waiting_input", lang(session, "A goal is needed to start.", "Butuh tujuan untuk memulai.")));
                return Ok(TurnOutcome::waiting_input("A goal is needed to start."));
            };
            if goal.objective.trim().is_empty() {
                obs.emit(AgentEvent::new("assistant", lang(session, "Type /goal with a goal, for example: /goal Finish the sign-in screen and test the whole flow.", "Ketik /goal beserta tujuan, misalnya: /goal Selesaikan layar login dan uji seluruh alurnya.")));
                obs.emit(AgentEvent::new("waiting_input", lang(session, "A goal is needed to start.", "Butuh tujuan untuk memulai.")));
                return Ok(TurnOutcome::waiting_input("A goal is needed to start."));
            }
            if goal_follow_up && !intent::is_continue(&user_prompt) {
                goal.add_follow_up(&user_prompt);
            }
            goal.status = "running".to_string();
            goal.summary = building_summary;
            goal.pending_question = None;
            verification = Some(GoalVerification::new());
            prompt = goal.task_prompt();
            obs.emit(AgentEvent::new("goal", lang(session, &format!("Working towards the goal: {prompt}"), &format!("Mengerjakan tujuan: {prompt}"))),
            );
        }

        // -- skills ----------------------------------------------------------
        // .NET AgentLoop: when the prompt mentions a skill with @alias, its
        // content is added to this turn's instructions, between the workspace
        // context and CURRENT LOCAL TASK. The store is built fresh from the
        // session's skills directory every turn (like .NET's
        // `new SkillStore(SkillsDirectory)`), and `SkillStore` reads the
        // skill files + skills.json from disk on every call, so a skill
        // imported or enabled/disabled since the last turn is visible on
        // the next one. Unknown or disabled skills are skipped silently.
        // For a /goal turn `prompt` is already the goal task prompt here.
        let (skill_block, applied_skills): (String, Vec<String>) =
            match &session.skills_directory {
                Some(dir) => match muse_store::SkillStore::new(dir) {
                    Ok(store) => store.expand(&prompt),
                    Err(err) => {
                        obs.emit(AgentEvent::new(
                            "skill",
                            lang(
                                session,
                                &format!("Could not load skills: {err}"),
                                &format!("Tidak bisa memuat skill: {err}"),
                            ),
                        ));
                        (String::new(), Vec::new())
                    }
                },
                None => (String::new(), Vec::new()),
            };
        if !applied_skills.is_empty() {
            let joined = applied_skills
                .iter()
                .map(|a| format!("@{a}"))
                .collect::<Vec<_>>()
                .join(", ");
            obs.emit(AgentEvent::new(
                "skill",
                lang(
                    session,
                    &format!("Applying skills: {joined}"),
                    &format!("Menerapkan skill: {joined}"),
                ),
            ));
        }

        // -- build the planner messages ------------------------------------
        session.tools.begin_agent_turn();
        let context = session.tools.workspace_context(cancel).await?;
        let mut system = format!(
            "{}\n\n{}{}\n\nCURRENT LOCAL TASK (keep working on this):\n{}",
            system_prompt::system_prompt_for(&session.reply_language),
            context,
            if skill_block.is_empty() {
                String::new()
            } else {
                format!("\n\n{skill_block}")
            },
            prompt
        );
        if verification.is_some() {
            system += &format!("\n\n{}", GoalRun::instructions_for(&session.reply_language));
        }
        messages.push(json!({ "role": "system", "content": system }));
        for message in session.history.clone() {
            let mut copy = message.clone();
            if copy.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                let previous = context::message_text(&copy);
                if parser::parse(&previous)
                    .iter()
                    .any(|c| c.name == "__parse_error__")
                {
                    copy["content"] = Value::String("[Invalid plan: rejected before execution.]".to_string());
                } else if intent::is_environment_confusion(&previous) {
                    copy["content"] = Value::String("[Previous reply confused the web VM with the local desktop. It is not evidence about this workspace. Use the local executor receipts.]".to_string());
                }
            }
            messages.push(copy);
        }
        let user_message: Value = if images.is_empty() {
            json!({ "role": "user", "content": user_prompt })
        } else {
            let batch = uuid::Uuid::new_v4().simple().to_string();
            let mut parts = vec![json!({ "type": "text", "text": user_prompt })];
            for url in &images {
                parts.push(json!({ "type": "image_url", "attachment_id": batch, "image_url": { "url": url } }));
            }
            json!({ "role": "user", "content": parts })
        };
        messages.push(user_message);

        let mut failures: HashMap<String, u32> = HashMap::new();
        let mut required = intent::required(&prompt);
        if verification.is_some() {
            required |= work::READ
                | work::COMMAND_VERIFIED
                | work::COMPUTER_INTERACT
                | work::COMPUTER_ASSERT
                | work::COMPUTER_SCREENSHOT;
        }
        let mut evidence = work::NONE;
        let mut protocol_retries = 0u32;
        let mut malformed_replies = 0u32;
        let mut scope_retries = 0u32;
        let mut declined = false;
        let mut verified_no_change = false;
        let mut previous_observation: Option<String> = None;
        let mut repeated_observations = 0u32;
        let max_steps = session.max_steps.unwrap_or(u32::MAX);

        let mut step: u32 = 0;
        loop {
            if step >= max_steps {
                obs.emit(AgentEvent::new(
                        "incomplete",
                        lang(session, &format!("Ran out of {max_steps} steps; the task is not finished."), &format!("Kehabisan {max_steps} langkah; tugas belum selesai.")),
                    ),
                );
                return Ok(TurnOutcome::incomplete(format!(
                    "Ran out of {max_steps} steps; the task is not finished."
                )));
            }
            step += 1;
            cancel.throw_if_cancelled()?;
            if context::compact(&mut messages, 1) {
                obs.emit(AgentEvent::new("context_compacted", lang(session, "Compacted the old context on this computer; kept the goal, instructions, and recent results. No extra web chat turn was created.", "Konteks lama di komputer ini sudah diringkas; tujuan, arahan, dan hasil terbaru tetap disimpan. Tidak ada giliran chat web tambahan.")),
                );
            }
            obs.emit(AgentEvent::new("assistant_start", lang(session, "Thinking…", "Berpikir…")));

            // APP_STATE packet (.NET AgentLoop).
            let app_state = json!({
                "goal": prompt,
                "workspace": session.tools.root().to_string_lossy(),
                "observed_operations": intent::work_to_string(evidence),
                "conversation_context": context::snapshot(&messages[1..]),
                "retrieved_context": Value::Null,
                "active_skills": json!(applied_skills),
                "goal_session": session.goal.as_ref().map(|g| json!({
                    "objective": g.objective,
                    "status": g.status,
                    "pending_question": g.pending_question,
                })).unwrap_or(Value::Null),
                "goal_mode": if verification.is_some() {
                    json!({
                        "objective": session.goal.as_ref().map(|g| g.objective.clone()).unwrap_or_default(),
                        "instructions": GoalRun::instructions_for(&session.reply_language),
                        "verification": verification.as_ref().map(|v| v.snapshot()).unwrap_or(Value::Null),
                    })
                } else {
                    Value::Null
                },
                "remaining": intent::work_to_string(required & !evidence),
                "computer_scope": session.tools.screen_context_for(&session.reply_language),
                "next": if required == work::NONE {
                    format!("Respond to the current user message in {}. For discussion, advice or a comment, return status done with the actual answer in summary and no actions. Resolve references such as continue using conversation_context, but only propose actions requested by the current user; do not execute historical tasks for a casual comment. Do not claim unobserved execution.", session.reply_language)
                } else {
                    "Generate the next operation as JSON data for the app. Continue after reading; ask only for information genuinely missing. Do not claim native execution of supplied observations.".to_string()
                },
            });
            let mut request = context::fit_context(&messages);
            request.push(json!({ "role": "user", "content": format!("APP_STATE:\n{app_state}") }));

            // Ask the driver for the next plan.
            let req = ChatRequest {
                model: session.model.clone(),
                prompt: user_prompt.clone(),
                images: images.clone(),
                reply_language: session.reply_language.clone(),
                session_id: Some(session.id.clone()),
                messages: request,
            };
            let mut forwarder = DeltaForwarder { sink: &mut *obs.sink };
            let text = match driver.chat(req, cancel, &mut forwarder).await {
                Ok(text) => text,
                Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
                Err(err) => {
                    obs.emit(AgentEvent::new("error", err.to_string()));
                    return Ok(TurnOutcome::incomplete("Chat turn did not complete."));
                }
            };
            cancel.throw_if_cancelled()?;
            let calls: Vec<ToolCall> = parser::parse(&text)
                .iter()
                .map(parser::validate)
                .collect();
            messages.push(json!({ "role": "assistant", "content": text }));

            // Malformed plan: never run a valid prefix.
            if calls.iter().any(|c| c.name == "__parse_error__") {
                if let Some(last) = messages.last_mut() {
                    last["content"] = Value::String(format!(
                        "[Invalid plan: rejected before execution.]\n{text}"
                    ));
                }
                let errors = calls
                    .iter()
                    .filter(|c| c.name == "__parse_error__")
                    .map(|c| c.error_message())
                    .collect::<Vec<_>>()
                    .join("\n");
                obs.emit(AgentEvent::new("protocol_error", errors.clone()));
                malformed_replies += 1;
                if malformed_replies >= 3 {
                    obs.emit(AgentEvent::new("incomplete", lang(session, "Muse did not send a correctly formatted operation after 3 replies. No operation from the faulty replies was run; steps that already succeeded are kept.", "Muse belum mengirim operasi berformat benar setelah 3 balasan. Tidak ada operasi dari balasan yang salah yang dijalankan; langkah yang sudah berhasil tetap disimpan.")));
                    return Ok(TurnOutcome::incomplete("Muse did not send a correctly formatted operation after 3 replies."));
                }
                messages.push(json!({ "role": "user", "content": format!("APP_PROTOCOL_REPAIR:\n{}", json!({
                    "error": errors,
                    "max_actions": 1,
                    "executed_from_rejected_reply": false,
                    "instruction": "Resend only ONE small action in a json code fence. JSON-escape code quotes, backslashes and newlines. Do not repeat the rejected batch. No source code has been inferred or repaired by the app.",
                })) }));
                obs.emit(AgentEvent::new("protocol_retry", lang(session, "Muse is resending the operation in the correct format. No change from the faulty reply has been applied…", "Muse mengirim ulang operasi dengan format benar. Belum ada perubahan dari balasan yang salah…")));
                continue;
            }
            malformed_replies = 0;

            // Scope check: the batch must stay inside the current request.
            let goal_mode = verification.is_some();
            let out_of_scope = !calls.is_empty()
                && ((verification.is_none()
                    && session.goal.as_ref().map(|g| g.status.as_str()) == Some("completed")
                    && intent::is_continue(&user_prompt))
                    || calls.iter().any(|call| !intent::allows(&prompt, &call.name, goal_mode)));
            if out_of_scope {
                if let Some(last) = messages.last_mut() {
                    last["content"] = Value::String("[Unverified reply: action batch exceeded the current user's request.]".to_string());
                }
                obs.emit(AgentEvent::new("protocol_retry", lang(session, "Skipped an operation outside the current request. Adjusting the reply…", "Operasi di luar permintaan saat ini dilewati. Menyesuaikan balasan…")));
                scope_retries += 1;
                if scope_retries >= 2 {
                    obs.emit(AgentEvent::new("incomplete", lang(session, "Muse suggested operations outside the request, so the app blocked all of them and the project is unchanged. Please send a clearer request.", "Muse mengusulkan operasi di luar permintaan, jadi aplikasi memblokir semuanya dan proyek belum berubah. Kirim permintaan yang lebih jelas.")));
                    return Ok(TurnOutcome::incomplete("Muse suggested operations outside the request."));
                }
                messages.push(json!({ "role": "user", "content": "LOCAL EXECUTOR: The entire action batch was rejected BEFORE execution. This turn is discussion/advice or explicitly read-only. Respect the current user request; return an answer in summary, or read-only file inspection when requested. Do not run commands, edit files or control the screen. Do not resume historical work." }));
                continue;
            }

            let visible = parser::planner_reply(&parser::strip_tool_blocks(&text));
            let mut needs_input = parser::needs_input(&text);
            if verification.is_some() && intent::is_permission_question(&visible) {
                needs_input = false;
            }

            if calls.is_empty() {
                // No actions: check goal verification / required work.
                if let Some(v) = &verification {
                    if !v.missing().is_empty() && !needs_input {
                        if let Some(last) = messages.last_mut() {
                            last["content"] = Value::String(format!("[Unverified reply: goal checks are still missing.]\n{text}"));
                        }
                        protocol_retries += 1;
                        if protocol_retries >= 2 {
                            let missing = v.missing().join("; ");
                            obs.emit(AgentEvent::new("incomplete", lang(session, &format!("Goal not finished yet. {missing}. The goal is saved; type /goal to continue."), &format!("Tujuan belum selesai. {missing}. Tujuan sudah disimpan; ketik /goal untuk melanjutkan."))));
                            return Ok(TurnOutcome::incomplete(format!("Goal not finished yet. {missing}")));
                        }
                        obs.emit(AgentEvent::new("protocol_retry", lang(session, "Continuing to verify the goal…", "Melanjutkan verifikasi tujuan…")));
                        let missing = v.missing().join("; ");
                        messages.push(json!({ "role": "user", "content": format!("LOCAL EXECUTOR: Goal completion rejected. Missing verified checks: {missing}. Continue implementing/testing/fixing using local actions. If genuinely blocked, ask a concrete question and pause. {}", GoalRun::instructions_for(&session.reply_language)) }));
                        continue;
                    }
                }
                if text.trim().is_empty() {
                    obs.emit(AgentEvent::new("error", lang(session, "The model returned an empty reply.", "Model mengembalikan balasan kosong.")));
                    return Ok(TurnOutcome::incomplete("The model returned an empty reply."));
                }
                let mut missing = required & !evidence;
                if missing == work::EDIT && verified_no_change && intent::explains_no_change(&visible) {
                    missing = work::NONE;
                }
                if !declined
                    && (missing != work::NONE && !needs_input
                        || required != work::NONE && intent::is_environment_confusion(&visible))
                {
                    if let Some(last) = messages.last_mut() {
                        last["content"] = Value::String(format!("[Unverified reply: the required local operations did not run.]\n{text}"));
                    }
                    obs.emit(AgentEvent::new("assistant", ""));
                    protocol_retries += 1;
                    if protocol_retries >= 2 {
                        obs.emit(AgentEvent::new("incomplete", lang(session, "Muse has not done enough operations on this project yet. Completion cannot be confirmed; see the Terminal for the steps that ran.", "Muse belum melakukan cukup operasi di proyek ini. Penyelesaian belum bisa dikonfirmasi; lihat Terminal untuk langkah yang sudah berjalan.")));
                        return Ok(TurnOutcome::incomplete("Muse has not done enough operations on this project yet."));
                    }
                    obs.emit(AgentEvent::new("protocol_retry", lang(session, "Asking Muse to turn the reply into operations on the project…", "Meminta Muse mengubah balasan menjadi operasi di proyek…")));
                    let hint = if missing & work::EDIT != 0 && evidence & work::READ != 0 {
                        "You have read local files. Now emit edit_file/write_file requests for the actual requested changes; do not stop at a plan. "
                    } else if missing & (work::COMMAND | work::COMMAND_VERIFIED) != 0 {
                        "Emit run_command for the required check, or process_status until an already started check exits successfully. "
                    } else if missing & intent::work::COMPUTER_WORK != 0 {
                        "Complete the remaining UI operations: enter the requested text, invoke/click the requested button, then computer_assert the requested resulting label/value. An inspect or input alone does not complete a UI test. Inspect again if element IDs expired. "
                    } else {
                        "Emit read_file/list_files to inspect the actual project, or summarize verified receipts if all requested work is already done. "
                    };
                    messages.push(json!({ "role": "user", "content": format!("LOCAL EXECUTOR: The requested local work is still pending ({}). Your built-in VM is not the user's machine. Do not claim completion or ask for uploaded files. {hint}Use a JSON code block with actions in the application schema. Wait for the external local result. Current task: {prompt}", intent::work_to_string(missing)) }));
                    continue;
                }
                let reply = if verification.is_some() && !needs_input {
                    let summary = verification.as_ref().and_then(|v| v.last_test_summary.clone()).unwrap_or_default();
                    format!("{visible}\n\n{}\n{summary}", lang(session, "The goal was verified with automated tests and UI actions/assertions/screenshots.", "Tujuan sudah diverifikasi dengan pengujian otomatis dan tindakan/verifikasi/gambar antarmuka."))
                } else {
                    visible.clone()
                };
                obs.emit(AgentEvent::new("assistant", reply));
                if needs_input {
                    if verification.is_some() {
                        if let Some(goal) = &mut session.goal {
                            goal.pending_question = Some(visible.chars().take(2000).collect());
                        }
                    }
                    obs.emit(AgentEvent::new("waiting_input", lang(session, "Muse needs more information. You can reply right in the chat.", "Muse butuh informasi tambahan. Anda bisa menjawab langsung di chat.")));
                    return Ok(TurnOutcome::waiting_input(visible));
                }
                obs.emit(AgentEvent::new("completed", lang(session, "Chat turn complete.", "Percakapan selesai.")));
                return Ok(TurnOutcome::completed(visible));
            }

            // Execute the action batch.
            obs.emit(AgentEvent::new("assistant", visible.clone()));
            if calls.len() > 16 {
                obs.emit(AgentEvent::new("error", "Model requested too many tool calls in one step (limit 16)"));
                return Ok(TurnOutcome::incomplete("Model requested too many tool calls in one step (limit 16)"));
            }
            for call in &calls {
                cancel.throw_if_cancelled()?;
                pending_tool = Some(call.name.clone());
                let path = call
                    .args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                obs.emit(AgentEvent::new("tool_start", ToolExecutor::describe(call))
                        .with_tool(call.name.clone(), path.clone()),
                );
                let result: String = if session.read_only && call.name.starts_with("computer_") {
                    format!("error: {READ_ONLY_SCREEN_MESSAGE}")
                } else {
                    match session.tools.prepare_for_agent(call, cancel).await {
                    Ok(prepared) => {
                        if ToolExecutor::needs_confirmation(call)
                            && !confirm(session, &prepared, cancel, &mut *obs.sink, options.interactive).await?
                        {
                            declined = true;
                            "error: user declined this step. Do not retry this action unless the user explicitly requests it.".to_string()
                        } else {
                            let output_sink = &mut |chunk: &str| {
                                obs.sink.emit(AgentEvent::new("command_output", chunk).with_tool(call.name.clone(), path.clone()));
                            };
                            match session.tools.execute_prepared(&prepared, cancel, Some(output_sink)).await {
                                Ok(text) => text,
                                Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
                                Err(err) => format!("error: {err}"),
                            }
                        }
                    }
                    Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
                    Err(err) => format!("error: {err}"),
                    }
                };
                let files_changed = if matches!(call.name.as_str(), "write_file" | "edit_file") {
                    session.tools.last_file_change_applied() && !tool_outcome::is_failure(&result)
                } else if call.name == "run_command" {
                    !session.tools.last_command_changed_files().is_empty()
                } else {
                    false
                };
                if let Some(v) = &mut verification {
                    v.record(
                        call,
                        &result,
                        files_changed,
                        call.name == "computer_screenshot"
                            && session.tools.last_computer_image().is_some(),
                    );
                }
                if verification.is_some() && files_changed {
                    failures.clear();
                    protocol_retries = 0;
                    previous_observation = None;
                    repeated_observations = 0;
                }
                let receipt = format!("Tool result for {}:\n{result}", call.name);
                let mut receipt_message = Map::new();
                receipt_message.insert("role".to_string(), Value::String("user".to_string()));
                receipt_message.insert(
                    "muse_tool".to_string(),
                    Value::String(call.name.clone()),
                );
                if let Some(path) = &path {
                    receipt_message.insert("muse_path".to_string(), Value::String(path.clone()));
                }
                if call.name == "run_command" {
                    if let Some(command) = call.args.get("command").and_then(|v| v.as_str()) {
                        receipt_message.insert(
                            "muse_command".to_string(),
                            Value::String(command.chars().take(500).collect()),
                        );
                    }
                }
                if call.name.starts_with("computer_") {
                    if let Some(image) = session.tools.last_computer_image() {
                        receipt_message.insert(
                            "content".to_string(),
                            json!([
                                { "type": "text", "text": receipt },
                                { "type": "image_url", "attachment_id": uuid::Uuid::new_v4().simple().to_string(), "image_url": { "url": image } },
                            ]),
                        );
                    } else {
                        receipt_message.insert("content".to_string(), Value::String(receipt));
                    }
                } else {
                    receipt_message.insert("content".to_string(), Value::String(receipt));
                }
                messages.push(Value::Object(receipt_message));
                obs.emit(AgentEvent::new("tool_result", result.clone())
                        .with_tool(call.name.clone(), path.clone()),
                );
                // Stop a repeated observation loop, not productive long tasks.
                let observation = context::observation_key(call, &result);
                repeated_observations = if Some(&observation) == previous_observation.as_ref() {
                    repeated_observations + 1
                } else {
                    1
                };
                previous_observation = Some(observation);
                if !tool_outcome::is_failure(&result)
                    && !tool_outcome::is_running(&result)
                    && repeated_observations >= 3
                {
                    obs.emit(AgentEvent::new("incomplete", lang(session, "Stopped because Muse repeated the same operation and result with no progress. Changes already made are kept; see the Terminal for details.", "Berhenti karena Muse mengulangi operasi dan hasil yang sama tanpa kemajuan. Perubahan yang sudah dilakukan tetap disimpan; lihat Terminal untuk detail.")));
                    return Ok(TurnOutcome::incomplete("Stopped because Muse repeated the same operation and result with no progress."));
                }
                if result.to_lowercase().starts_with("error: user declined") {
                    obs.emit(AgentEvent::new("declined", lang(session, "Stopped the part you had not allowed. Steps already completed are kept.", "Menghentikan bagian yang belum Anda izinkan. Langkah yang sudah selesai tetap disimpan.")));
                    return Ok(TurnOutcome::declined("Stopped the part you had not allowed."));
                }
                if !tool_outcome::is_failure(&result) {
                    if matches!(call.name.as_str(), "write_file" | "edit_file")
                        && result.starts_with("unchanged:")
                    {
                        verified_no_change = true;
                    }
                    let previous_evidence = evidence;
                    let operation_evidence = intent::evidence(&call.name);
                    if operation_evidence & work::COMPUTER_INTERACT != 0 {
                        evidence &= !(work::COMPUTER_ASSERT | work::COMPUTER_SCREENSHOT);
                    }
                    if matches!(call.name.as_str(), "edit_file" | "write_file")
                        && session.tools.last_file_change_applied()
                    {
                        evidence &= !work::COMMAND_VERIFIED;
                    }
                    if !matches!(call.name.as_str(), "edit_file" | "write_file")
                        || session.tools.last_file_change_applied()
                    {
                        evidence |= operation_evidence;
                    }
                    if tool_outcome::exited_successfully(&result) {
                        evidence |= work::COMMAND_VERIFIED;
                    }
                    if call.name == "run_command"
                        && !session.tools.last_command_changed_files().is_empty()
                    {
                        evidence |= work::EDIT;
                    }
                    if evidence != previous_evidence {
                        protocol_retries = 0;
                    }
                }
                pending_tool = None;
                if tool_outcome::is_failure(&result) {
                    let key = format!("{}{}", call.name, Value::Object(call.args.clone()));
                    let count = failures.entry(key).or_insert(0);
                    *count += 1;
                    if failures.len() > 256 {
                        if let Some(first) = failures.keys().next().cloned() {
                            failures.remove(&first);
                        }
                    }
                    if *failures.get(&format!("{}{}", call.name, Value::Object(call.args.clone()))).unwrap_or(&0) >= 2 {
                        obs.emit(AgentEvent::new("incomplete", lang(session, "Stopped because the same operation failed twice. Review the result and adjust the request.", "Berhenti karena operasi yang sama gagal dua kali. Tinjau hasil dan sesuaikan permintaan.")));
                        return Ok(TurnOutcome::incomplete("Stopped because the same operation failed twice."));
                    }
                }
            }
        }
    })
    .await;

    // -- finally (mirrors .NET AgentLoop finally) ---------------------------
    let final_outcome = match result {
        Ok(outcome) => outcome,
        Err(AgentError::Cancelled) => {
            let cancelled_text = match &pending_tool {
                Some(tool) => format!("Tool result for {tool}: cancelled by user; inspect the current workspace before continuing."),
                None => "The user stopped this turn.".to_string(),
            };
            messages.push(json!({ "role": "user", "content": cancelled_text }));
            obs.emit(AgentEvent::new("cancelled", lang(session, "Stopped as requested.", "Dihentikan sesuai permintaan.")));
            TurnOutcome::cancelled()
        }
        Err(err) => {
            obs.emit(AgentEvent::new("error", err.to_string()));
            TurnOutcome::incomplete("Chat turn did not complete.")
        }
    };

    let verified_msg = lang(session, "Verified automated tests and UI.", "Pengujian otomatis dan antarmuka sudah diverifikasi.");
    let interrupted_msg = lang(session, "The turn was interrupted.", "Giliran terputus.");
    let still_needed_msg = lang(session, "Still needed:", "Masih perlu:");
    if let Some(goal) = &mut session.goal {
        if verification.is_some() {
            let missing = verification.as_ref().map(|v| v.missing()).unwrap_or_default();
            let completed = obs.outcome.as_ref().map(|(k, _)| k.as_str()) == Some("completed")
                && missing.is_empty();
            goal.status = if completed {
                "completed".to_string()
            } else if obs.outcome.as_ref().map(|(k, _)| k.as_str()) == Some("waiting_input") {
                "waiting_input".to_string()
            } else {
                "paused".to_string()
            };
            goal.summary = if goal.status == "completed" {
                let test_summary = verification
                    .as_ref()
                    .and_then(|v| v.last_test_summary.clone())
                    .unwrap_or_default();
                format!("{verified_msg}\n{test_summary}")
            } else {
                let text = obs
                    .outcome
                    .as_ref()
                    .map(|(_, t)| t.clone())
                    .unwrap_or_else(|| interrupted_msg.clone());
                format!("{text}\n{still_needed_msg} {}", missing.join("; "))
            };
            goal.updated_at = chrono::Utc::now().to_rfc3339();
        }
    }

    if messages.len() > 1 {
        if let Some(last) = messages.last_mut() {
            if let Value::Object(map) = last {
                map.insert(
                    "muse_turn_outcome".to_string(),
                    json!({
                        "goal": prompt.chars().take(1000).collect::<String>(),
                        "status": obs.outcome.as_ref().map(|(k, _)| k.clone()).unwrap_or_else(|| "interrupted".to_string()),
                        "summary": obs.outcome.as_ref().map(|(_, t)| t.clone()).unwrap_or_else(|| lang(session, "The chat turn was interrupted; check the result before continuing.", "Giliran chat terputus; periksa hasil sebelum melanjutkan.")),
                    }),
                );
            }
        }
        context::compact(&mut messages, 1);
        session.history = messages
            .into_iter()
            .skip(1)
            .map(|m| context::without_image(&m))
            .collect();
    }
    session.last_outcome = obs
        .outcome
        .clone()
        .or_else(|| Some((final_outcome.status.clone(), final_outcome.summary.clone())));

    // The observed outcome wins over the computed one when they disagree
    // (e.g. a turn that ended `waiting_input` inside the loop).
    match &obs.outcome {
        Some((kind, text)) => TurnOutcome {
            ok: kind == "completed",
            status: kind.clone(),
            summary: text.clone(),
        },
        None => final_outcome,
    }
}

/// Direct-tool gate (`.NET AgentEndpoints.DirectTool`): may this call run
/// without an approval round-trip? Returns `false` when the caller must
/// answer the 403 `approval_required` body first.
pub fn direct_tool_may_run(session: &AgentSession, call: &ToolCall, approved: bool) -> bool {
    // E6: screen control is refused in read-only mode at the direct-tool
    // entry point too (the caller gets the 403 `approval_required` body).
    if session.read_only && call.name.starts_with("computer_") {
        return false;
    }
    if !ToolExecutor::needs_confirmation(call) {
        return true;
    }
    if session.turbo && !approval::requires_explicit_approval(call) {
        return true;
    }
    approved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ToolCall;

    fn session_with_read_only(read_only: bool) -> AgentSession {
        let dir = std::env::temp_dir().join(format!(
            "muse-agent-chat-turn-test-{}-{}",
            std::process::id(),
            read_only
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = AgentSession::new(&dir, "auto", "muse-chat", "English", false, None).unwrap();
        s.read_only = read_only;
        s
    }

    #[test]
    fn direct_tool_refuses_computer_calls_in_read_only() {
        let call = ToolCall::new("computer_screenshot", Map::new());
        // Even an explicit approval cannot override read-only (E6).
        assert!(!direct_tool_may_run(&session_with_read_only(true), &call, true));
        assert!(!direct_tool_may_run(&session_with_read_only(true), &call, false));
        // Read-only off: computer calls need no confirmation (existing rule).
        assert!(direct_tool_may_run(&session_with_read_only(false), &call, false));
        // Read-only blocks only computer tools, not other tools.
        let write = ToolCall::new("write_file", Map::new());
        assert!(!direct_tool_may_run(
            &session_with_read_only(true),
            &write,
            false
        ));
        assert!(direct_tool_may_run(
            &session_with_read_only(true),
            &write,
            true
        ));
    }
}
