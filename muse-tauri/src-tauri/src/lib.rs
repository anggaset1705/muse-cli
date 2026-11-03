use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use serde::Serialize;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

// ---------------------------------------------------------------------------
// Backend sidecar — Rust muse-gateway only.
// Typed, minimal, no free shell.
// ---------------------------------------------------------------------------
//
// The gateway (Rust workspace `<repo>/muse-gateway`) is launched as a
// child process, never through a shell. It binds loopback only
// (MUSE2API_HOST=127.0.0.1) on a free port picked here, and is health-checked
// against its public GET /healthz endpoint. No secret is ever hardcoded:
// the gateway resolves its own API key from its own data dir / env.
//
// Accepted backend locations (first match wins when no explicit path given):
//   1. env MUSE_BACKEND_PATH — a muse-gateway executable
//   2. <app exe dir>/muse-gateway(.exe)                 (packaged release)
//   3. <repo>/muse-gateway/target/release/muse-gateway(.exe) (dev layout)
// An explicit `backend_path` argument must exist on disk and be a
// muse-gateway executable; anything else is rejected.

/// File name of the gateway binary on this platform.
fn gateway_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "muse-gateway.exe"
    } else {
        "muse-gateway"
    }
}
const HEALTH_PATH: &str = "/healthz";

/// Hide the console window of a spawned child on Windows. The sidecar
/// (muse-gateway.exe), the login helper and the Terminal shells are all
/// console-subsystem programs: spawned without this flag each of them pops
/// a black cmd window on the user's desktop when the app starts (or when a
/// terminal command runs). CREATE_NO_WINDOW (0x08000000) keeps them
/// invisible; stdout/stderr stay piped/nulled as before. No-op elsewhere.
#[cfg(target_os = "windows")]
fn hide_console(cmd: &mut Command) {
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(target_os = "windows"))]
fn hide_console(_cmd: &mut Command) {}

#[derive(Debug, Default, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BackendStatus {
    /// A child process is spawned and has not been stopped.
    pub running: bool,
    /// The child answered GET /healthz with HTTP 200 (best effort).
    pub healthy: bool,
    pub base_url: Option<String>,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    /// How the backend was launched, e.g. "muse-gateway.exe (driver: mock)".
    pub launch_mode: Option<String>,
    /// Why the backend last failed, if it did (see `backend_status`).
    /// None while a backend is running or no failure was recorded.
    pub error: Option<String>,
}

struct RunningBackend {
    child: Child,
    port: u16,
    launch_mode: String,
    /// Driver the child was started with ("mock" or "browser") — used to
    /// word the exit error when the child dies.
    driver: String,
    /// Bearer key handed to the gateway via MUSE2API_API_KEY at spawn.
    /// The gateway's own api_key FILE cannot be read from here: on Windows
    /// it is DPAPI-encrypted (muse-dpapi-v1:), so the app generates the key
    /// itself and is the only holder of the plaintext. Loopback-local only.
    api_key: String,
}

#[derive(Default)]
struct BackendStateInner {
    backend: Option<RunningBackend>,
    /// Reason the backend last failed (child exited on its own). Cleared
    /// on a successful spawn and on an explicit stop — a stop is not a
    /// failure.
    last_error: Option<String>,
}

#[derive(Default)]
struct BackendState(Mutex<BackendStateInner>);

/// A running in-app sign-in window (`muse-gateway login` child process).
/// Tracked only so the UI can tell whether it is still open; the child
/// exits on its own when the user finishes signing in or closes it.
/// The launch mode rides along: a still-running RESTORE-mode window must
/// never swallow a later ADD-account request (or vice versa) — see
/// `start_login`, which kills + relaunches on a mode mismatch instead
/// of returning `already_open`.
#[derive(Default)]
struct LoginState(Mutex<Option<LoginChild>>);

/// A running login child plus the mode it was launched in.
struct LoginChild {
    child: Child,
    clear_session: bool,
}

/// C8 — close guard: the editor reports its dirty state here (via
/// `set_editor_dirty`); `on_window_event` intercepts CloseRequested while
/// dirty and asks the frontend (which owns the i18n confirm text) whether
/// to proceed. The frontend answers with `confirm_app_close`, which sets
/// `close_allowed` and destroys the main window.
#[derive(Default)]
struct CloseState(Mutex<CloseGuard>);

#[derive(Default)]
struct CloseGuard {
    editor_dirty: bool,
    close_allowed: bool,
}

/// C8 — the editor calls this (fire-and-forget) whenever its dirty state
/// changes, so the Rust close interceptor knows whether to block.
#[tauri::command]
fn set_editor_dirty(state: tauri::State<'_, CloseState>, dirty: bool) {
    if let Ok(mut g) = state.0.lock() {
        g.editor_dirty = dirty;
    }
}

/// C8 — called by the frontend after the user confirmed discarding (or
/// when there was nothing dirty): allow the close and destroy the window.
/// The backend sidecar is still killed by the ExitRequested handler.
#[tauri::command]
fn confirm_app_close(app: tauri::AppHandle, state: tauri::State<'_, CloseState>) -> Result<(), String> {
    if let Ok(mut g) = state.0.lock() {
        g.close_allowed = true;
    }
    match app.get_webview_window("main") {
        Some(w) => w.destroy().map_err(|e| e.to_string()),
        None => Err("main window not found".to_string()),
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct LoginStatus {
    pub launched: bool,
    pub already_open: bool,
    pub pid: Option<u32>,
}

/// Pick a free loopback port by binding 127.0.0.1:0 and releasing it.
/// (Small TOCTOU window; the gateway fails loudly if the port is taken and
/// the caller can retry `start_backend`.)
fn pick_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("cannot pick a free port: {e}"))?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

/// Best-effort HTTP GET /healthz over a raw TCP connection (std only).
/// Returns true only on an HTTP 200 status line. The connect has a 2s
/// timeout (a bare `TcpStream::connect` can block indefinitely on a
/// black-holed loopback address); read/write timeouts are 2s as well.
fn probe_health(port: u16) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = format!("127.0.0.1:{port}").to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let req = format!("GET {HEALTH_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 256];
    let Ok(n) = stream.read(&mut buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200")
}

/// Word the failure of a backend child that exited on its own.
fn exit_error_message(driver: &str, status: std::process::ExitStatus) -> String {
    if driver == "browser" {
        format!(
            "backend exited during startup (code {status}, driver: browser). \
             The browser driver needs Chromium (Playwright) — install it and try again."
        )
    } else {
        format!("backend exited during startup (code {status}, driver: {driver}). Try Start backend again.")
    }
}

/// Status of the current backend, reaping a dead child.
///
/// A child that exited on its own is reaped here (state becomes None) and
/// the reason is recorded in `last_error`, so the next `backend_status`
/// poll — and the next `start_backend` — can report it instead of
/// silently showing "not running". A live child always reports
/// `error: None` (a stale failure is cleared on spawn).
fn status_of(state: &mut BackendStateInner) -> BackendStatus {
    match state.backend.as_mut() {
        None => BackendStatus {
            error: state.last_error.clone(),
            ..BackendStatus::default()
        },
        Some(b) => {
            let base_url = Some(format!("http://127.0.0.1:{}", b.port));
            let port = Some(b.port);
            let pid = Some(b.child.id());
            let launch_mode = Some(b.launch_mode.clone());
            match b.child.try_wait() {
                Ok(None) => BackendStatus {
                    running: true,
                    healthy: probe_health(b.port),
                    base_url,
                    port,
                    pid,
                    launch_mode,
                    error: None,
                },
                Ok(Some(status)) => {
                    let msg = exit_error_message(&b.driver, status);
                    state.last_error = Some(msg.clone());
                    state.backend = None;
                    BackendStatus {
                        running: false,
                        healthy: false,
                        base_url,
                        port,
                        pid,
                        launch_mode,
                        error: Some(msg),
                    }
                }
                Err(_) => {
                    // Status unknown — treat as dead, same as before.
                    let msg = format!(
                        "backend exited during startup (status unknown, driver: {}). Try Start backend again.",
                        b.driver
                    );
                    state.last_error = Some(msg.clone());
                    state.backend = None;
                    BackendStatus {
                        running: false,
                        healthy: false,
                        base_url,
                        port,
                        pid,
                        launch_mode,
                        error: Some(msg),
                    }
                }
            }
        }
    }
}

/// Dev-layout gateway binary: `<repo>/muse-gateway/target/release/`
/// `muse-gateway(.exe)`, found by walking up from the app exe
/// (`<repo>/muse-tauri/src-tauri/target/<profile>/<app>`).
fn dev_gateway_path(exe_dir: &Path) -> Option<PathBuf> {
    let root = exe_dir.ancestors().nth(4)?;
    Some(
        root.join("muse-gateway")
            .join("target")
            .join("release")
            .join(gateway_file_name()),
    )
}

fn candidate_backend_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("MUSE_BACKEND_PATH") {
        if !p.is_empty() {
            out.push(PathBuf::from(p));
        }
    }
    // Packaged release: the Rust gateway lives next to the app binary.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join(gateway_file_name()));
            // Dev layout on this machine (new repo layout).
            if let Some(dev) = dev_gateway_path(dir) {
                out.push(dev);
            }
        }
    }
    out
}

/// Validate an explicit backend path: must exist and be a muse-gateway
/// executable (`.exe` on Windows, extensionless elsewhere).
fn validate_backend_path(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Err(format!("backend path does not exist: {}", p.display()));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let ok = if cfg!(target_os = "windows") {
        ext == "exe"
    } else {
        ext.is_empty()
    };
    if !ok {
        return Err(format!(
            "backend path must be the muse-gateway executable ({}): {}",
            gateway_file_name(),
            p.display()
        ));
    }
    Ok(())
}

fn resolve_backend_path(explicit: Option<String>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        let p = p.trim();
        if p.is_empty() {
            return Err("backend_path must not be empty".to_string());
        }
        let path = PathBuf::from(p);
        validate_backend_path(&path)?;
        return Ok(path);
    }
    let candidates = candidate_backend_paths();
    for cand in &candidates {
        if cand.exists() {
            validate_backend_path(cand)?;
            return Ok(cand.clone());
        }
    }
    Err(format!(
        "muse-gateway tidak ditemukan. Salin {} ke samping exe aplikasi, \
         atau set MUSE_BACKEND_PATH ke path muse-gateway. Lokasi yang dicek: {}",
        gateway_file_name(),
        candidates
            .iter()
            .map(|c| c.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Generate a `m2a-<36 hex>` bearer key for the sidecar. Randomness comes
/// from `RandomState` (OS-seeded per process); this is a loopback-local
/// bearer, not a user credential — it only proves the caller is this app.
fn generate_api_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 18];
    for chunk in bytes.chunks_mut(8) {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        h.write_u64(nanos);
        h.write_u32(std::process::id());
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    format!(
        "m2a-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

/// Data dir shared by the sidecar and the sign-in child: deterministic,
/// beside this exe, so the gateway's accounts, sessions and key file
/// always land in <exe dir>/Muse2Api/data instead of depending on
/// whatever cwd the app happened to start with.
fn sidecar_data_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|dir| dir.join("Muse2Api").join("data"))
}

/// I10 — the gateway's data folder (accounts.json, agent sessions, keys).
/// Created if missing so the file manager always has something to open.
/// The frontend's "Buka folder data" button calls this, then opens the
/// returned path with the opener plugin.
#[tauri::command]
fn app_data_dir() -> Result<String, String> {
    let dir = sidecar_data_dir().ok_or_else(|| "cannot find the app folder for the data dir".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create data dir {}: {e}", dir.display()))?;
    Ok(dir.to_string_lossy().to_string())
}

/// Start the Rust muse-gateway sidecar as a child process.
///
/// Returns immediately after spawn (the gateway may still be starting —
/// poll `backend_status` until `healthy` is true). It deliberately does
/// NOT wait for healthy here: a slow gateway (browser driver Chromium
/// start can take ~1 minute) must not hold the state mutex or the
/// command thread. A child that dies at once is NOT an error from this
/// command — the next `backend_status` poll reports it via
/// `BackendStatus.error` (and `last_error`), so the UI can show the
/// reason and offer a retry. Starting twice is a no-op returning the
/// current status. No shell is involved and no secret is accepted or
/// stored here.
#[tauri::command]
fn start_backend(
    state: tauri::State<'_, BackendState>,
    backend_path: Option<String>,
    driver: Option<String>,
) -> Result<BackendStatus, String> {
    let mut guard = state.0.lock().map_err(|e| format!("backend state poisoned: {e}"))?;
    start_backend_inner(&mut guard, backend_path, driver)
}

fn start_backend_inner(
    state: &mut BackendStateInner,
    backend_path: Option<String>,
    driver: Option<String>,
) -> Result<BackendStatus, String> {
    if let Some(b) = state.backend.as_mut() {
        if matches!(b.child.try_wait(), Ok(None)) {
            return Ok(status_of(state));
        }
        // Previous child exited; drop it so a fresh one can start.
        // (Any failure reason it left in last_error is cleared below
        // on successful spawn.)
        state.backend = None;
    }

    let driver = driver
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "mock".to_string());
    if driver != "mock" && driver != "browser" {
        return Err(format!("unknown driver '{driver}' (expected mock or browser)"));
    }

    let backend_path = resolve_backend_path(backend_path)?;
    let port = pick_free_port()?;

    // The only backend is the Rust muse-gateway executable — launched
    // directly, never through a shell or a runtime.
    let mut cmd: Command = Command::new(&backend_path);

    // Loopback only, free port. The gateway reads its own API key itself.
    // The bearer key is generated here and passed in (see RunningBackend).
    // The driver choice is forwarded so real chat uses the browser driver
    // (default mock = echo replies, no real Muse).
    let api_key = generate_api_key();
    if let Some(dir) = sidecar_data_dir() {
        cmd.env("MUSE2API_DATA_DIR", dir);
    }
    cmd.env("MUSE2API_API_KEY", &api_key)
        .env("MUSE2API_DRIVER", &driver)
        .env("MUSE2API_HOST", "127.0.0.1")
        .env("MUSE2API_PORT", port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut cmd);

    let launch_mode = format!("{} (driver: {})", backend_path.display(), driver);
    let child = cmd.spawn().map_err(|e| format!("failed to start backend: {e}"))?;
    state.backend = Some(RunningBackend { child, port, launch_mode, driver, api_key });
    // Spawn succeeded: any stale failure is gone, and the caller polls
    // backend_status until healthy (or until it reports an error).
    state.last_error = None;
    Ok(status_of(state))
}

/// Path of the Rust gateway binary beside this app (only the Rust
/// gateway has a `login` subcommand with a WebView2 window).
fn resolve_gateway_exe() -> Result<PathBuf, String> {
    for cand in candidate_backend_paths() {
        if cand.is_file()
            && cand
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == gateway_file_name())
                .unwrap_or(false)
        {
            return Ok(cand);
        }
    }
    Err(format!(
        "in-app sign-in needs the Rust gateway ({}) next to the app — it was not found. \
         Build it with `cargo build --release -p muse-gateway` in muse-gateway/ and copy it \
         next to the app exe, or set MUSE_BACKEND_PATH.",
        gateway_file_name()
    ))
}

/// Path of the login child's runtime log: `<data_dir>/login.log`
/// (the same data dir the gateway keeps `accounts.json` in, so the file
/// is reachable from the frontend's "Buka folder data" button).
///
/// Design (Perbaikan 1, 3 Nov 2026 — Bree approved ~11:31 WIB):
/// the login child (`muse-gateway login`) writes its stage transitions
/// to stderr (see `muse-login/src/login_log.rs`); `start_login`
/// redirects the child's stdout AND stderr to this file, so the
/// transitions survive the window closing and can be read after a
/// failed login instead of vanishing with the child process.
/// APPEND, not truncate-per-run: every launch appends, so a failed
/// login is still in the file after a retry. If the file grows past
/// 512 KiB it is deleted first (one login run is a few dozen lines, so
/// the cap is never hit in normal use). Each run starts with the
/// child's `[login …] init:` line, which acts as the run separator.
/// If the file cannot be opened the login still starts with nulled
/// stdio (today's behaviour) — a missing log must never block signing
/// in. The file never contains cookie/token values (stage names,
/// statuses, counts and lengths only).
const LOGIN_LOG_MAX_BYTES: u64 = 512 * 1024;
const LOGIN_LOG_NAME: &str = "login.log";

fn open_login_log(data_dir: &Path) -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(data_dir);
    let path = data_dir.join(LOGIN_LOG_NAME);
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > LOGIN_LOG_MAX_BYTES {
            let _ = std::fs::remove_file(&path);
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}

/// Open the Muse sign-in window (`muse-gateway login`) inside the app.
///
/// The login child POSTs the captured session to the gateway that is
/// already running, so this ensures the backend is up first (starting it
/// with the requested driver when needed) and hands the child the same
/// port, bearer key and data dir as the sidecar. Non-blocking: the
/// window runs until the user finishes signing in or closes it; poll
/// `login_status` to know when it is gone.
///
/// `clear_session` (dialog "Other account") passes `--clear-session` to
/// the child: the window drops the saved WebView session before navigating,
/// so a different account can sign in. The gateway accounts are untouched.
///
/// JS ARG NAME (3 Nov 2026 — root cause of 7x `clear_session=false` in
/// login.log): Tauri v2 commands take camelCase args by default, so the
/// frontend must invoke with `clearSession`. A snake_case `clear_session`
/// key is silently dropped by serde (unknown field) and `Option<bool>`
/// defaults to None → false — the child then launches WITHOUT
/// --clear-session and restores the saved session.
#[tauri::command]
fn start_login(
    backend_state: tauri::State<'_, BackendState>,
    login_state: tauri::State<'_, LoginState>,
    label: Option<String>,
    driver: Option<String>,
    clear_session: Option<bool>,
) -> Result<LoginStatus, String> {
    let want_clear = clear_session.unwrap_or(false);
    {
        let mut login = login_state
            .0
            .lock()
            .map_err(|e| format!("login state poisoned: {e}"))?;
        if let Some(entry) = login.as_mut() {
            if matches!(entry.child.try_wait(), Ok(None)) {
                // A sign-in window is still open. Reuse it ONLY when it
                // was launched in the same mode — a restore-mode window
                // showing the already-signed-in Muse page must never
                // swallow an add-account request (the user asked for a
                // FRESH sign-in and would get "langsung masuk" instead).
                // On a mode mismatch, kill the old window and launch the
                // requested mode below, so the mode can never miss.
                if entry.clear_session == want_clear {
                    return Ok(LoginStatus { launched: false, already_open: true, pid: Some(entry.child.id()) });
                }
                let _ = entry.child.kill();
                let _ = entry.child.wait();
            }
            *login = None;
        }
    }

    let (port, api_key) = {
        let mut backend = backend_state
            .0
            .lock()
            .map_err(|e| format!("backend state poisoned: {e}"))?;
        let _status = start_backend_inner(&mut backend, None, driver)?;
        let b = backend
            .backend
            .as_ref()
            .ok_or_else(|| "backend did not start".to_string())?;
        (b.port, b.api_key.clone())
    };

    let gateway_exe = resolve_gateway_exe()?;
    let data_dir = sidecar_data_dir()
        .ok_or_else(|| "cannot find the app folder for the gateway data dir".to_string())?;

    let mut cmd = Command::new(&gateway_exe);
    cmd.arg("login");
    if let Some(l) = label.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        cmd.arg("--label").arg(l);
    }
    if want_clear {
        cmd.arg("--clear-session");
    }
    cmd.env("MUSE2API_PORT", port.to_string())
        .env("MUSE2API_API_KEY", &api_key)
        .env("MUSE2API_DATA_DIR", &data_dir)
        .stdin(Stdio::null());
    // Runtime log (Perbaikan 1): the child's stdout+stderr go to
    // <data_dir>/login.log (append, truncated past 512 KiB) instead of
    // being discarded — see `open_login_log`. If the file cannot be
    // opened the login still starts with nulled stdio.
    match open_login_log(&data_dir) {
        Some(log_file) => match log_file.try_clone() {
            Ok(stdout) => {
                cmd.stdout(stdout);
                cmd.stderr(log_file);
            }
            Err(_) => {
                cmd.stdout(Stdio::null());
                cmd.stderr(log_file);
            }
        },
        None => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
    }
    hide_console(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to open the sign-in window: {e}"))?;
    let pid = child.id();
    let mut login = login_state
        .0
        .lock()
        .map_err(|e| format!("login state poisoned: {e}"))?;
    *login = Some(LoginChild { child, clear_session: want_clear });
    Ok(LoginStatus { launched: true, already_open: false, pid: Some(pid) })
}

/// Whether the in-app sign-in window is still open.
#[tauri::command]
fn login_status(state: tauri::State<'_, LoginState>) -> Result<LoginStatus, String> {
    let mut login = state
        .0
        .lock()
        .map_err(|e| format!("login state poisoned: {e}"))?;
    if let Some(entry) = login.as_mut() {
        if matches!(entry.child.try_wait(), Ok(None)) {
            return Ok(LoginStatus { launched: false, already_open: true, pid: Some(entry.child.id()) });
        }
        *login = None;
    }
    Ok(LoginStatus { launched: false, already_open: false, pid: None })
}

/// Report whether the sidecar backend is running and healthy.
///
/// Also reaps a child that died on its own and records why in
/// `BackendStatus.error`, so the UI can show the failure instead of a
/// bare "not running".
#[tauri::command]
fn backend_status(state: tauri::State<'_, BackendState>) -> Result<BackendStatus, String> {
    let mut guard = state.0.lock().map_err(|e| format!("backend state poisoned: {e}"))?;
    Ok(status_of(&mut guard))
}

/// Stop the sidecar backend (SIGKILL-equivalent, then reap). No-op if none.
/// An explicit stop clears any recorded failure — it is not a failure.
#[tauri::command]
fn stop_backend(state: tauri::State<'_, BackendState>) -> Result<BackendStatus, String> {
    let mut guard = state.0.lock().map_err(|e| format!("backend state poisoned: {e}"))?;
    if let Some(mut b) = guard.backend.take() {
        let _ = b.child.kill();
        let _ = b.child.wait();
    }
    guard.last_error = None;
    Ok(BackendStatus::default())
}

/// Bearer key of the currently running sidecar (generated in start_backend
/// and passed via MUSE2API_API_KEY). The frontend calls this once the
/// backend is healthy and uses the key for all /v1/agent/* calls, so the
/// user never types a key. Errors when no backend was started by this app
/// (e.g. an external/manual gateway) — then the key in Settings applies.
#[tauri::command]
fn backend_api_key(state: tauri::State<'_, BackendState>) -> Result<String, String> {
    let mut guard = state.0.lock().map_err(|e| format!("backend state poisoned: {e}"))?;
    // NB: try_wait() needs &mut, and match-guard bindings are immutable,
    // so check liveness in a plain if-let instead of a match guard.
    if let Some(b) = guard.backend.as_mut() {
        if matches!(b.child.try_wait(), Ok(None)) {
            return Ok(b.api_key.clone());
        }
    }
    Err("no backend running".to_string())
}

/// Strip a Windows verbatim-path prefix (`\\?\`) from a path string.
///
/// On Windows `Path::canonicalize()` returns the verbatim form, e.g.
/// `\\?\C:\Users\B-STORE\Desktop`. That spelling is a valid path, but it
/// is NOT the spelling the user picked — and the frontend keys the
/// conversation list by the raw path string, so writing the verbatim form
/// back into settings orphaned the whole conversation list for that
/// workspace (defect B, 3 Nov 2026: FOX conversations "disappeared" after
/// one send). `\\?\UNC\server\share` becomes the plain UNC `\\server\share`.
/// Anything without the prefix is returned unchanged.
fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = path.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Resolve the workspace folder the agent loop will use.
///
/// - An absolute path is created if missing and returned as-is: it is a
///   real folder the user picked (or a fresh valid one).
/// - Anything else (empty, or a Unix-style "/home/user/workspace" left over
///   in settings on Windows) falls back to "<home>/Muse Workspace",
///   created if missing. On Windows a "/..." path is NOT absolute, so the
///   stale Linux default can never leak through again.
/// - The returned path NEVER carries the Windows verbatim `\\?\` prefix
///   (defect B): canonicalize() output is stripped, and so is the
///   as-typed fallback, so the frontend can compare/store it verbatim.
#[tauri::command]
fn resolve_workspace(requested: Option<String>) -> Result<String, String> {
    if let Some(p) = requested.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(&p);
        if path.is_absolute() {
            std::fs::create_dir_all(&path).map_err(|e| format!("cannot create workspace {p}: {e}"))?;
            return Ok(strip_verbatim_prefix(
                &path
                    .canonicalize()
                    .map(|c| c.to_string_lossy().to_string())
                    .unwrap_or(p),
            ));
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| "cannot find a home folder for the default workspace".to_string())?;
    let dir = PathBuf::from(home).join("Muse Workspace");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create workspace {}: {e}", dir.display()))?;
    Ok(strip_verbatim_prefix(&dir.to_string_lossy()))
}

fn kill_backend(state: &BackendState) {
    if let Ok(mut guard) = state.0.lock() {
        if let Some(mut b) = guard.backend.take() {
            let _ = b.child.kill();
            let _ = b.child.wait();
        }
    }
}

fn kill_login(state: &LoginState) {
    if let Ok(mut guard) = state.0.lock() {
        if let Some(mut entry) = guard.take() {
            let _ = entry.child.kill();
            let _ = entry.child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace files + local terminal.
//
// HONEST SCOPE: these commands are LOCAL to this Tauri app. They read/write
// files only inside a workspace folder the user picks, and run commands with
// that folder as cwd. They are NOT the agent loop — the real agent executor
// lives in the Rust gateway (`/v1/agent/*`), not here — so nothing here
// claims to be the real agent executor.
// The frontend labels every such surface LOCAL / STUB accordingly.
// ---------------------------------------------------------------------------

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TREE_ENTRIES: usize = 1000;

/// Resolve `rel` inside `root`, refusing escapes (`..`, absolute paths,
/// symlinks pointing outside). `must_exist` canonicalises the target itself;
/// otherwise the parent dir is canonicalised and the file name appended.
fn resolve_in_workspace(root: &str, rel: &str, must_exist: bool) -> Result<PathBuf, String> {
    let root_path = Path::new(root);
    let canon_root = root_path
        .canonicalize()
        .map_err(|e| format!("workspace root is not readable: {e}"))?;
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err("path must be relative to the workspace".to_string());
    }
    let joined = canon_root.join(rel_path);
    let resolved = if must_exist {
        joined
            .canonicalize()
            .map_err(|e| format!("path not found in workspace: {e}"))?
    } else {
        let parent = joined.parent().unwrap_or(&canon_root);
        let canon_parent = parent
            .canonicalize()
            .map_err(|e| format!("parent folder not found in workspace: {e}"))?;
        let name = joined
            .file_name()
            .ok_or_else(|| "invalid file name".to_string())?;
        canon_parent.join(name)
    };
    if !resolved.starts_with(&canon_root) {
        return Err("path escapes the workspace".to_string());
    }
    Ok(resolved)
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct FileEntry {
    pub name: String,
    pub rel: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub children: Option<Vec<FileEntry>>,
}

fn skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "bin" | "obj" | "target" | ".venv" | "dist"
    )
}

fn list_dir(root: &PathBuf, dir: &Path, rel_prefix: &str, depth: u8, budget: &mut usize) -> Vec<FileEntry> {
    let mut out: Vec<FileEntry> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut items: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    items.sort_by_key(|p| {
        let is_dir = p.is_dir();
        (std::cmp::Reverse(is_dir), p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default())
    });
    for path in items {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        // Skip dotfiles/dotfolders (except .gitignore) to keep the tree clean.
        if name.starts_with('.') && name != ".gitignore" {
            continue;
        }
        let rel = if rel_prefix.is_empty() { name.clone() } else { format!("{rel_prefix}/{name}") };
        let is_dir = path.is_dir();
        if is_dir && skip_dir(&name) {
            continue;
        }
        let size = if is_dir { None } else { path.metadata().ok().map(|m| m.len()) };
        let children = if is_dir && depth < 3 {
            Some(list_dir(root, &path, &rel, depth + 1, budget))
        } else {
            None
        };
        out.push(FileEntry { name, rel, is_dir, size, children });
    }
    out
}

#[tauri::command]
fn list_workspace_files(root: String, rel: String) -> Result<Vec<FileEntry>, String> {
    let dir = resolve_in_workspace(&root, &rel, true)?;
    if !dir.is_dir() {
        return Err("not a folder".to_string());
    }
    let canon_root = Path::new(&root).canonicalize().map_err(|e| e.to_string())?;
    let mut budget = MAX_TREE_ENTRIES;
    Ok(list_dir(&canon_root, &dir, rel.trim_end_matches('/'), 0, &mut budget))
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct FileContent {
    pub rel: String,
    pub content: String,
    pub size: u64,
}

#[tauri::command]
fn read_workspace_file(root: String, rel: String) -> Result<FileContent, String> {
    let path = resolve_in_workspace(&root, &rel, true)?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("file too large ({} bytes > 2 MB)", meta.len()));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read as text (binary?): {e}"))?;
    Ok(FileContent { rel: rel.to_string(), content, size: meta.len() })
}

#[tauri::command]
fn write_workspace_file(root: String, rel: String, content: String) -> Result<u64, String> {
    if content.len() as u64 > MAX_FILE_BYTES {
        return Err("content too large (> 2 MB)".to_string());
    }
    let path = resolve_in_workspace(&root, &rel, false)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(content.len() as u64)
}

// --- Local terminal: one child per run, output pumped by a reader thread. ---

struct TerminalRun {
    child: Child,
    output: Arc<Mutex<String>>,
}

#[derive(Default)]
struct TerminalState {
    next_id: AtomicU32,
    runs: Mutex<HashMap<u32, TerminalRun>>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct TerminalOutput {
    pub id: u32,
    pub output: String,
    pub running: bool,
}

#[tauri::command]
fn start_terminal(
    state: tauri::State<'_, TerminalState>,
    root: String,
    command: String,
    shell: Option<String>,
) -> Result<u32, String> {
    let cwd = resolve_in_workspace(&root, "", true)?;
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("command is empty".to_string());
    }
    if command.len() > 4000 {
        return Err("command too long".to_string());
    }
    // No free shell: only a named shell with `-c <command>`, cwd = workspace.
    let shell_name = shell.unwrap_or_else(|| {
        if cfg!(windows) { "cmd".to_string() } else { "sh".to_string() }
    });
    let shell_name = shell_name.trim().to_ascii_lowercase();
    let allowed: &[&str] = if cfg!(windows) {
        &["cmd", "powershell", "pwsh"]
    } else {
        &["sh", "bash", "zsh"]
    };
    if !allowed.contains(&shell_name.as_str()) {
        return Err(format!("shell not allowed: {shell_name}"));
    }
    let mut cmd = Command::new(&shell_name);
    if shell_name == "cmd" {
        cmd.arg("/c");
    } else if shell_name == "powershell" || shell_name == "pwsh" {
        cmd.arg("-NoProfile").arg("-Command");
    } else {
        cmd.arg("-c");
    }
    cmd.arg(&command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("failed to start command: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let output = Arc::new(Mutex::new(String::new()));
    let out_clone = Arc::clone(&output);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut g) = out_clone.lock() {
                        g.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if g.len() > 1_000_000 {
                            let excess = g.len() - 1_000_000;
                            g.drain(..excess);
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });
    let out_clone2 = Arc::clone(&output);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut g) = out_clone2.lock() {
                        g.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if g.len() > 1_000_000 {
                            let excess = g.len() - 1_000_000;
                            g.drain(..excess);
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });
    let id = state.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    let mut runs = state.runs.lock().map_err(|e| format!("terminal state poisoned: {e}"))?;
    runs.insert(id, TerminalRun { child, output });
    Ok(id)
}

#[tauri::command]
fn terminal_output(state: tauri::State<'_, TerminalState>, id: u32) -> Result<TerminalOutput, String> {
    let mut runs = state.runs.lock().map_err(|e| format!("terminal state poisoned: {e}"))?;
    let run = runs.get_mut(&id).ok_or_else(|| format!("unknown terminal run {id}"))?;
    let running = matches!(run.child.try_wait(), Ok(None));
    let output = run.output.lock().map(|g| g.clone()).unwrap_or_default();
    Ok(TerminalOutput { id, output, running })
}

#[tauri::command]
fn stop_terminal(state: tauri::State<'_, TerminalState>, id: u32) -> Result<TerminalOutput, String> {
    let mut runs = state.runs.lock().map_err(|e| format!("terminal state poisoned: {e}"))?;
    let mut run = runs.remove(&id).ok_or_else(|| format!("unknown terminal run {id}"))?;
    let _ = run.child.kill();
    let _ = run.child.wait();
    let output = run.output.lock().map(|g| g.clone()).unwrap_or_default();
    Ok(TerminalOutput { id, output, running: false })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(BackendState::default())
        .manage(LoginState::default())
        .manage(TerminalState::default())
        .manage(CloseState::default())
        .invoke_handler(tauri::generate_handler![
            start_backend,
            backend_status,
            stop_backend,
            backend_api_key,
            start_login,
            login_status,
            resolve_workspace,
            app_data_dir,
            set_editor_dirty,
            confirm_app_close,
            list_workspace_files,
            read_workspace_file,
            write_workspace_file,
            start_terminal,
            terminal_output,
            stop_terminal
        ])
        // C8 — intercept window close while the editor is dirty: block the
        // close and let the frontend (which owns the localized confirm
        // text) ask the user; it answers via `confirm_app_close`. When
        // nothing is dirty the close proceeds untouched, so a hung webview
        // can never trap the window.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let block = window
                    .state::<CloseState>()
                    .0
                    .lock()
                    .map(|g| g.editor_dirty && !g.close_allowed)
                    .unwrap_or(false);
                if block {
                    api.prevent_close();
                    let _ = window.emit("app-close-requested", ());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::ExitRequested { .. } = event {
                kill_backend(app.state::<BackendState>().inner());
                kill_login(app.state::<LoginState>().inner());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_verbatim_prefix_plain_drive() {
        // The exact Bree case: canonicalize() on Windows returns this.
        assert_eq!(
            strip_verbatim_prefix("\\\\?\\C:\\Users\\B-STORE\\Desktop"),
            "C:\\Users\\B-STORE\\Desktop"
        );
    }

    #[test]
    fn strip_verbatim_prefix_unc() {
        assert_eq!(
            strip_verbatim_prefix("\\\\?\\UNC\\server\\share\\FOX"),
            "\\\\server\\share\\FOX"
        );
    }

    #[test]
    fn strip_verbatim_prefix_leaves_plain_paths_alone() {
        assert_eq!(
            strip_verbatim_prefix("C:\\Users\\B-STORE\\Desktop"),
            "C:\\Users\\B-STORE\\Desktop"
        );
        assert_eq!(
            strip_verbatim_prefix("\\\\server\\share\\FOX"),
            "\\\\server\\share\\FOX"
        );
        assert_eq!(strip_verbatim_prefix("/home/bree/FOX"), "/home/bree/FOX");
        assert_eq!(strip_verbatim_prefix(""), "");
    }

    #[test]
    fn resolve_workspace_never_returns_verbatim_prefix() {
        // Real folder under the temp dir (canonicalize() runs on Linux
        // here; on Windows it is the call that produces the `\\?\`
        // prefix, which strip_verbatim_prefix must remove).
        let dir = std::env::temp_dir().join("muse-test-workspace-b-defect");
        let requested = dir.to_string_lossy().to_string();
        let out = resolve_workspace(Some(requested)).expect("resolve_workspace");
        assert!(!out.contains("\\\\?\\"), "verbatim prefix leaked: {out}");
        assert!(Path::new(&out).is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_workspace_empty_falls_back_to_home_workspace() {
        let out = resolve_workspace(None).expect("fallback resolve");
        assert!(!out.contains("\\\\?\\"), "verbatim prefix leaked: {out}");
        assert!(out.ends_with("Muse Workspace"), "unexpected fallback: {out}");
    }

    #[test]
    fn resolve_workspace_relative_falls_back_like_before() {
        // A relative path must still fall back, never be returned as-is.
        // (The Unix-style "/home/user/workspace" case only applies on
        // Windows, where a "/..." path is not absolute; on Linux it IS
        // absolute, so it is not tested here.)
        let out = resolve_workspace(Some("relative/workspace".to_string()))
            .expect("fallback resolve");
        assert!(!out.contains("\\\\?\\"), "verbatim prefix leaked: {out}");
        assert!(out.ends_with("Muse Workspace"), "unexpected fallback: {out}");
    }
}
