//! Foreground + background process execution — port of `ToolProcess.cs`
//! (`RunProcessAsync`, `RunShellAsync`, `RunCommandAsync`) and
//! `BackgroundProcesses.cs`.
//!
//! Process-tree kill: .NET `ProcessTree` walks descendants with Toolhelp
//! on Windows and `Kill(entireProcessTree: true)` elsewhere. On Linux this
//! port puts every spawned process in its own process group (`setsid` in
//! `pre_exec`) and kills the whole group (`killpg`), which is the
//! equivalent containment. On Windows the child is killed directly;
//! descendant enumeration is NOT ported (see the report).

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify};

use crate::error::{AgentError, AgentResult, CancellationToken};
use crate::shell::{create_invocation, ShellInvocation};

/// Clamp a timeout the way .NET does (`Math.Clamp(seconds, 1, 3600)`).
pub fn clamp_timeout(seconds: i64) -> u64 {
    seconds.clamp(1, 3600) as u64
}

/// Configure a `tokio::process::Command` the way .NET `ProcessInfo` does:
/// no shell execute, piped stdout/stderr, git env vars. On Unix the child
/// becomes a process-group leader so the whole tree can be killed.
fn base_command(file: &str, args: &[String], workdir: &Path, stdin_piped: bool) -> Command {
    let mut cmd = Command::new(file);
    cmd.args(args)
        .current_dir(workdir)
        .stdin(if stdin_piped {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd
}

/// Kill a process tree. On Unix the child is a group leader, so killing
/// the group kills descendants too. On Windows only the child is killed
/// (descendant enumeration is not ported).
async fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
}

/// Output state for one pipe, mirroring .NET `Drain`: the first
/// 16_000 chars are kept verbatim; the last 8_000 chars are kept as tail;
/// when the output is longer, the head is cut to 8_000 chars and the tail
/// follows after a truncation notice.
struct DrainState {
    head: String,
    tail: String,
    total: usize,
}

impl DrainState {
    fn new() -> Self {
        Self {
            head: String::new(),
            tail: String::new(),
            total: 0,
        }
    }

    fn append(&mut self, chunk: &str) {
        let len = chunk.chars().count();
        self.total += len;
        if self.head.chars().count() < 16_000 {
            let room = 16_000 - self.head.chars().count();
            self.head.extend(chunk.chars().take(room));
        }
        self.tail.push_str(chunk);
        if self.tail.chars().count() > 8_000 {
            let skip = self.tail.chars().count() - 8_000;
            self.tail = self.tail.chars().skip(skip).collect();
        }
    }

    fn finish(&self) -> String {
        if self.total > self.head.chars().count() {
            let keep = self.head.chars().take(8_000).collect::<String>();
            format!(
                "{keep}\n[output truncated; {} characters total; last 8000 characters follow]\n{}",
                self.total, self.tail
            )
        } else {
            self.head.clone()
        }
    }
}

/// Drain one pipe into shared state, forwarding each live chunk to the
/// parent through `tx` (`.NET Drain` calls `output(prefix+chunk)` per
/// chunk). The drain task owns only the pipe, the state and the sender,
/// so it stays `Send`; the parent invokes the callback.
async fn drain_to_state<R>(
    mut reader: R,
    state: Arc<Mutex<DrainState>>,
    tx: tokio::sync::mpsc::UnboundedSender<(bool, String)>,
    is_stderr: bool,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut buf = [0u8; 2048];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                state.lock().await.append(&chunk);
                if tx.send((is_stderr, chunk)).is_err() {
                    break;
                }
            }
        }
    }
}

/// Run a command in the foreground with a timeout (`.NET RunProcessAsync`).
/// Returns the `exit_code=…\nstdout:\n…\nstderr:\n…` result string, or an
/// `error: …` string on start failure / timeout. Cancellation kills the
/// process tree and returns `Err(Cancelled)`, like .NET.
///
/// Live output chunks are forwarded to `output` (if any) as they arrive;
/// the drain tasks own only the pipes, so the callback never has to be
/// `Send`.
pub async fn run_foreground(
    file: &str,
    args: &[String],
    workdir: &Path,
    timeout_seconds: i64,
    cancel: &CancellationToken,
    mut output: Option<&mut (dyn FnMut(&str) + Send)>,
) -> AgentResult<String> {
    let timeout = clamp_timeout(timeout_seconds);
    cancel.throw_if_cancelled()?;
    let mut cmd = base_command(file, args, workdir, false);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return Ok(format!("error: could not start {file}: {err}")),
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return Ok(format!("error: could not start {file}: stdout was not piped")),
    };
    let stderr = match child.stderr.take() {
        Some(s) => s,
        None => return Ok(format!("error: could not start {file}: stderr was not piped")),
    };
    let (chunk_tx, mut chunk_rx) =
        tokio::sync::mpsc::unbounded_channel::<(bool, String)>();
    let out_state = Arc::new(Mutex::new(DrainState::new()));
    let err_state = Arc::new(Mutex::new(DrainState::new()));
    let stdout_task = tokio::spawn(drain_to_state(
        stdout,
        out_state.clone(),
        chunk_tx.clone(),
        false,
    ));
    let stderr_task = tokio::spawn(drain_to_state(
        stderr,
        err_state.clone(),
        chunk_tx.clone(),
        true,
    ));
    drop(chunk_tx);
    let deadline = tokio::time::sleep(Duration::from_secs(timeout));
    tokio::pin!(deadline);
    let mut timed_out = false;
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let mut cancelled = false;
    loop {
        tokio::select! {
            res = child.wait() => {
                exit_status = res.ok();
                break;
            }
            Some((is_err, chunk)) = chunk_rx.recv() => {
                if let Some(cb) = &mut output {
                    cb(&format!("{}{}", if is_err { "[stderr] " } else { "" }, chunk));
                }
            }
            _ = &mut deadline => {
                timed_out = true;
                break;
            }
            _ = cancel.cancelled() => {
                cancelled = true;
                break;
            }
        }
    }
    if cancelled {
        kill_tree(&mut child).await;
        let _ = child.wait().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let _ = stdout_task.await;
            let _ = stderr_task.await;
        })
        .await;
        return Err(AgentError::Cancelled);
    }
    if timed_out {
        kill_tree(&mut child).await;
        let _ = child.wait().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let _ = stdout_task.await;
            let _ = stderr_task.await;
        })
        .await;
        return Ok(format!(
            "error: command timed out after {timeout}s and was killed"
        ));
    }
    // The process exited; drain any remaining chunks (bounded wait: a
    // grandchild may keep a pipe open), then join the drains.
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some((is_err, chunk)) = chunk_rx.recv().await {
            if let Some(cb) = &mut output {
                cb(&format!("{}{}", if is_err { "[stderr] " } else { "" }, chunk));
            }
        }
    })
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = stdout_task.await;
        let _ = stderr_task.await;
    })
    .await;
    let exit = exit_status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
    let out = out_state.lock().await.finish();
    let err_out = err_state.lock().await.finish();
    Ok(format!("exit_code={exit}\nstdout:\n{out}\nstderr:\n{err_out}"))
}

/// Run a shell command in the foreground (`.NET RunShellAsync`).
pub async fn run_shell(
    command: &str,
    shell: &str,
    workdir: &Path,
    timeout_seconds: i64,
    cancel: &CancellationToken,
    output: Option<&mut (dyn FnMut(&str) + Send)>,
) -> AgentResult<String> {
    let invocation = match create_invocation(command, shell) {
        Ok(inv) => inv,
        Err(err) => return Ok(format!("error: {err}")),
    };
    let result = run_foreground(
        &invocation.file,
        &invocation.args,
        workdir,
        timeout_seconds,
        cancel,
        output,
    )
    .await;
    invocation.cleanup();
    result
}

/// Run `git` with args in the workspace root, 30 s timeout (`.NET GitAsync`).
pub async fn run_git(args: &[&str], root: &Path, cancel: &CancellationToken) -> AgentResult<String> {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    run_foreground("git", &owned, root, 30, cancel, None).await
}

// ---------------------------------------------------------------------------
// Background processes
// ---------------------------------------------------------------------------

/// One running background process (`.NET RunningProcess`).
pub struct RunningProcess {
    pub id: String,
    output: Arc<Mutex<String>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    done: Arc<Notify>,
    timed_out: AtomicBool,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    stop: Arc<Notify>,
}

impl RunningProcess {
    fn status_text(&self, id: &str, running: bool, exit_code: Option<i32>) -> String {
        let mut out = format!("process_id={id}\nrunning={running}\n");
        if let Some(code) = exit_code {
            out += &format!("exit_code={code}\n");
        }
        if self.timed_out.load(Ordering::SeqCst) {
            out += "timed_out=true\n";
        }
        out
    }

    /// `.NET ProcessStatus(id, running)` — needs the locked output text.
    async fn status(&self) -> String {
        let exit_code = *self.exit_code.lock().await;
        let running = exit_code.is_none();
        let text = self.output.lock().await.clone();
        format!(
            "{}{}",
            self.status_text(&self.id, running, exit_code),
            format!("output (latest 32000 chars):\n{text}")
        )
    }
}

/// Background process registry (`.NET ToolExecutor._processes`).
pub struct ProcessManager {
    processes: Mutex<HashMap<String, Arc<RunningProcess>>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
        }
    }

    /// Start a background shell command (`.NET StartProcessAsync`).
    /// Returns the initial `process_status` string.
    pub async fn start(
        &self,
        command: &str,
        shell: &str,
        workdir: &Path,
        timeout_seconds: i64,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        if command.trim().is_empty() {
            return Ok("error: command must not be empty".to_string());
        }
        {
            let mut map = self.processes.lock().await;
            // Prune finished processes once the registry grows past 50.
            if map.len() > 50 {
                let expired: Vec<String> = map
                    .iter()
                    .filter(|(_, p)| p.exit_code.try_lock().map(|c| c.is_some()).unwrap_or(false))
                    .map(|(k, _)| k.clone())
                    .take(map.len() - 50)
                    .collect();
                for key in expired {
                    map.remove(&key);
                }
            }
            let running = map
                .values()
                .filter(|p| p.exit_code.try_lock().map(|c| c.is_none()).unwrap_or(false))
                .count();
            if running >= 8 {
                return Ok(
                    "error: background process limit reached (8). Stop an existing process first."
                        .to_string(),
                );
            }
        }
        if !workdir.is_dir() {
            return Ok("error: working directory does not exist".to_string());
        }
        let invocation = create_invocation(command, shell)
            .map_err(|e| AgentError::Invalid(e.to_string()))?;
        let mut cmd = base_command(&invocation.file, &invocation.args, workdir, true);
        cancel.throw_if_cancelled()?;
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                invocation.cleanup();
                return Err(AgentError::Message(format!(
                    "could not start {}: {err}",
                    invocation.file
                )));
            }
        };
        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let running = Arc::new(RunningProcess {
            id: id.clone(),
            output: Arc::new(Mutex::new(String::new())),
            exit_code: Arc::new(Mutex::new(None)),
            done: Arc::new(Notify::new()),
            timed_out: AtomicBool::new(false),
            stdin: Mutex::new(stdin),
            stop: Arc::new(Notify::new()),
        });
        let supervisor_state = running.clone();
        let timeout = clamp_timeout(timeout_seconds);
        tokio::spawn(async move {
            let output = supervisor_state.output.clone();
            let append = |text: &str| {
                let output = output.clone();
                let text = text.to_string();
                async move {
                    let mut guard = output.lock().await;
                    guard.push_str(&text);
                    if guard.len() > 32_000 {
                        let skip = guard.len() - 32_000;
                        *guard = guard.chars().skip(skip).collect();
                    }
                }
            };
            let mut drain_tasks = Vec::new();
            if let Some(stdout) = stdout {
                let output = output.clone();
                drain_tasks.push(tokio::spawn(async move {
                    let mut reader = stdout;
                    let mut buf = [0u8; 2048];
                    loop {
                        match reader.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                                let mut guard = output.lock().await;
                                guard.push_str(&chunk);
                                if guard.len() > 32_000 {
                                    let skip = guard.len() - 32_000;
                                    *guard = guard.chars().skip(skip).collect();
                                }
                            }
                        }
                    }
                }));
            }
            if let Some(stderr) = stderr {
                let output = output.clone();
                drain_tasks.push(tokio::spawn(async move {
                    let mut reader = stderr;
                    let mut buf = [0u8; 2048];
                    loop {
                        match reader.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                                let mut guard = output.lock().await;
                                guard.push_str("[stderr] ");
                                guard.push_str(&chunk);
                                if guard.len() > 32_000 {
                                    let skip = guard.len() - 32_000;
                                    *guard = guard.chars().skip(skip).collect();
                                }
                            }
                        }
                    }
                }));
            }
            let deadline = tokio::time::sleep(Duration::from_secs(timeout));
            tokio::pin!(deadline);
            let mut child = child;
            let mut timed_out = false;
            let mut stopped = false;
            tokio::select! {
                _ = child.wait() => {}
                _ = &mut deadline => { timed_out = true; }
                _ = supervisor_state.stop.notified() => { stopped = true; }
            }
            if timed_out || stopped {
                if timed_out {
                    supervisor_state.timed_out.store(true, Ordering::SeqCst);
                }
                kill_tree(&mut child).await;
                let _ = child.wait().await;
                if timed_out {
                    append("\n[background process timed out and was killed]\n").await;
                }
            } else {
                // `child.wait()` above already reaped the process; wait again
                // is a no-op, but keep the shape for clarity.
                let _ = child.wait().await;
            }
            let code = child
                .try_wait()
                .ok()
                .flatten()
                .map(|s| s.code().unwrap_or(-1));
            // `wait()` consumed the status above in the non-timeout path;
            // recover it from the exit code we stored, if any.
            let exit_code = match code {
                Some(c) => Some(c),
                None => None,
            };
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                for task in drain_tasks {
                    let _ = task.await;
                }
            })
            .await;
            // The exit code may have been consumed by the first `wait()` in
            // the select; fall back to the stored status if needed.
            let mut guard = supervisor_state.exit_code.lock().await;
            if guard.is_none() {
                *guard = exit_code;
            }
            drop(guard);
            supervisor_state.done.notify_waiters();
            let _ = pid;
            invocation.cleanup();
        });
        {
            let mut map = self.processes.lock().await;
            map.insert(id.clone(), running.clone());
        }
        // Give the process a moment to fail fast, like .NET's 250 ms delay.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            _ = cancel.cancelled() => {
                running.stop.notify_waiters();
                return Err(AgentError::Cancelled);
            }
        }
        Ok(running.status().await)
    }

    /// `process_status` / `process_input` / `process_stop`
    /// (`.NET ProcessToolAsync`).
    pub async fn tool(
        &self,
        name: &str,
        args: &serde_json::Map<String, serde_json::Value>,
        cancel: &CancellationToken,
    ) -> AgentResult<String> {
        use crate::error::json::{bool_arg, int_arg, str_arg};
        let id = str_arg(args, "process_id", "");
        let running = {
            let map = self.processes.lock().await;
            match map.get(&id) {
                Some(p) => p.clone(),
                None => {
                    return Err(AgentError::Invalid(
                        "Unknown process_id in this workspace session".to_string(),
                    ))
                }
            }
        };
        cancel.throw_if_cancelled()?;
        if name == "process_stop" {
            // The supervisor stores the exit code *before* notifying done,
            // so the exit code is the source of truth: a `done` notification
            // that fired before this call is not missed.
            if running.exit_code.lock().await.is_none() {
                running.stop.notify_waiters();
                let deadline = tokio::time::sleep(Duration::from_secs(5));
                tokio::pin!(deadline);
                loop {
                    if running.exit_code.lock().await.is_some() {
                        break;
                    }
                    tokio::select! {
                        _ = running.done.notified() => { break; }
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                        _ = &mut deadline => { break; }
                        _ = cancel.cancelled() => { return Err(AgentError::Cancelled); }
                    }
                }
            }
            return Ok(format!("stopped=true\n{}", running.status().await));
        }
        if name == "process_input" {
            if running.exit_code.lock().await.is_some() {
                return Ok("error: process has already exited".to_string());
            }
            let input = str_arg(args, "text", "");
            if input.chars().count() > 16_000 {
                return Ok("error: process input is too long".to_string());
            }
            use tokio::io::AsyncWriteExt;
            let mut stdin = running.stdin.lock().await;
            if let Some(stdin) = stdin.as_mut() {
                stdin.write_all(input.as_bytes()).await?;
                if bool_arg(args, "newline", true) {
                    stdin.write_all(b"\n").await?;
                }
                stdin.flush().await?;
            } else {
                return Ok("error: process has already exited".to_string());
            }
            return Ok(running.status().await);
        }
        // process_status: wait up to wait_ms for the process to finish.
        // (Exit code first: the process may already be done.)
        if running.exit_code.lock().await.is_none() {
            let wait_ms = int_arg(args, "wait_ms", 30_000).clamp(0, 60_000) as u64;
            tokio::select! {
                _ = running.done.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(wait_ms)) => {}
                _ = cancel.cancelled() => { return Err(AgentError::Cancelled); }
            }
        }
        cancel.throw_if_cancelled()?;
        Ok(running.status().await)
    }

    /// Stop every background process (`.NET StopBackgroundProcessesAsync`).
    pub async fn stop_all(&self) {
        let processes: Vec<Arc<RunningProcess>> = {
            let mut map = self.processes.lock().await;
            let values: Vec<_> = map.values().cloned().collect();
            map.clear();
            values
        };
        for process in &processes {
            process.stop.notify_waiters();
        }
        for process in &processes {
            if process.exit_code.lock().await.is_some() {
                continue;
            }
            let _ = tokio::time::timeout(Duration::from_secs(5), process.done.notified()).await;
        }
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Re-exported for the executor: run a shell command and report which
/// workspace files changed (`.NET RunCommandAsync` snapshot logic lives in
/// the executor; this helper only runs the shell).
pub fn shell_invocation(command: &str, shell: &str) -> AgentResult<ShellInvocation> {
    create_invocation(command, shell)
}
