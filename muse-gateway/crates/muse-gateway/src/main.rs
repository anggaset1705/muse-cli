// Muse gateway — Rust backend for the Muse CLI desktop app.
//
// Sidecar binary for the Tauri desktop app: binds loopback only, port from
// `MUSE2API_PORT` (via `Settings`), never writes outside `DATA_DIR`.
// The parent (Tauri) owns the process lifetime — no parent-death handling
// here.
//
// Subcommands:
//   muse-gateway            run the HTTP server
//   muse-gateway login [--label <label>] [--label=<label>]
//                       open the Muse sign-in window (muse-login) against
//                       the running gateway; non-Windows prints a clear
//                       NotImplemented message and exits 1.

mod adapters;
mod auth;
mod cors;
mod errors;
mod routes;
mod state;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;

use muse_agent::session::AgentSessionManager;
use muse_browser::DriverFactory;
use muse_core::Settings;
use muse_screen::DesktopComputer;
use muse_store::{
    AccountPool, ChatSessionStore, KeyStore, MediaStore, RequestLog, SkillStore, TaskStore,
};

use crate::adapters::{AgentDriverAdapter, ScreenDriverAdapter};
use state::AppState;

fn exit_with(message: &str, code: i32) -> ! {
    eprintln!("muse-gateway: {message}");
    std::process::exit(code);
}

/// Append one line to `<data_dir>/gateway.log` (best effort, never fails
/// the start). Cacat A (3 Nov 2026): stdout/stderr gateway dibuang Tauri,
/// jadi tanpa file ini kegagalan load `accounts.json` tidak meninggalkan
/// jejak — pool kosong lalu `persist` menimpa akun lama diam-diam.
/// Format: `[gateway 2026-11-03T11:51:00Z] <line>`.
fn append_gateway_log(data_dir: &std::path::Path, line: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(data_dir);
    let path = data_dir.join("gateway.log");
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[gateway {ts}] {line}");
    }
}

/// One-line, no-newline rendering of an error for log lines.
fn one_line(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let short: String = flat.chars().take(200).collect();
    if flat.chars().count() > 200 {
        format!("{short}…")
    } else {
        short
    }
}

/// Parsed `muse-gateway login` arguments (extracted from `cmd_login` so
/// the flag handling is unit-testable — the 3 Nov 2026 add-account
/// incident was a flag that arrived as `false` at the child, and the
/// only way to tell "caller never passed it" from "parser dropped it"
/// is a logged `argv` plus a parser covered by tests).
#[derive(Debug, Clone, PartialEq, Eq)]
struct LoginArgs {
    label: String,
    clear_session: bool,
}

/// Parse `muse-gateway login` args. Accepted forms:
/// `--label <label>`, `--label=<label>`, `--clear-session`,
/// `--clear-session=true|false`. Anything else is an error (exit 2) —
/// a silently ignored flag is exactly the failure mode of 3 Nov 2026,
/// so unknown `--clear-session=...` spellings must fail loudly instead
/// of launching a normal-mode window.
fn parse_login_args(args: &[String]) -> Result<LoginArgs, String> {
    let mut label = "default".to_string();
    let mut clear_session = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(v) = arg.strip_prefix("--label=") {
            label = v.to_string();
        } else if arg == "--label" {
            i += 1;
            if i >= args.len() {
                return Err("--label needs a value".to_string());
            }
            label = args[i].clone();
        } else if arg == "--clear-session" {
            clear_session = true;
        } else if let Some(v) = arg.strip_prefix("--clear-session=") {
            match v {
                "true" | "1" => clear_session = true,
                "false" | "0" => clear_session = false,
                other => {
                    return Err(format!(
                        "--clear-session={other} is not true/false; use bare --clear-session"
                    ))
                }
            }
        } else {
            return Err(format!("unknown argument '{arg}'"));
        }
        i += 1;
    }
    Ok(LoginArgs {
        label,
        clear_session,
    })
}

/// Fingerprint of `accounts.json` as raw bytes (size + first 16 hex of
/// the SHA-256): distinguishes "file was `[]` / missing" from "file
/// held 1 account but was read as 0" without ever logging cookie
/// values (a hash reveals nothing about the contents). `missing` when
/// the file does not exist.
fn accounts_fingerprint(path: &std::path::Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let hash = muse_store::error::sha256_hex_upper(&bytes);
            format!("bytes={} sha256={}", bytes.len(), &hash[..16.min(hash.len())])
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing".to_string(),
        Err(e) => format!("unreadable: {}", one_line(&e.to_string())),
    }
}

/// Render raw argv for the `init` log line (args with spaces quoted),
/// so the next add-account attempt shows exactly what the child
/// received — `argv=[login --label "Account 2" --clear-session]` vs
/// `argv=[login --label "Account 2"]` settles "caller never passed it"
/// vs "parser dropped it" without any guesswork.
fn format_argv(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.chars().any(|c| c.is_whitespace() || c == '"') {
                format!("\"{}\"", a.replace('"', "'"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `muse-gateway login [--label ...] [--clear-session]`: open the Muse
/// sign-in window against the gateway that is already running on loopback.
/// The admin key comes from settings (DPAPI / env, same as the server path);
/// the data root mirrors .NET `GatewayApplication`
/// (`%LocalAppData%\Muse2Api.WinForms` on Windows, `MUSE_DESKTOP_PROFILE_ROOT`
/// override anywhere). `--clear-session` (LoginForm.cs `clearExistingSession`)
/// drops the saved WebView session before navigating, so a different account
/// can sign in; the gateway accounts themselves are untouched.
fn cmd_login(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("usage: muse-gateway login [--label <label>] [--clear-session]");
        return 0;
    }
    let LoginArgs {
        label,
        clear_session,
    } = match parse_login_args(args) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("muse-gateway login: {e}");
            return 2;
        }
    };

    let settings = Settings::load();
    if let Err(e) = settings.validate() {
        eprintln!("muse-gateway login: invalid settings: {e}");
        return 2;
    }
    let settings = match settings.resolve_api_key() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("muse-gateway login: cannot resolve API key: {e}");
            return 2;
        }
    };
    let gateway_url = format!("http://127.0.0.1:{}", settings.port);
    let admin_key = settings.effective_admin_key().to_string();
    let data_root = std::env::var("MUSE_DESKTOP_PROFILE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                std::env::var("LOCALAPPDATA")
                    .map(|p| std::path::PathBuf::from(p).join("Muse2Api.WinForms"))
                    .unwrap_or_else(|_| settings.profile_dir())
            } else {
                settings.profile_dir()
            }
        });
    // Saved cookies of the first account in accounts.json, so a returning
    // session can be reused by the sign-in window (`.NET Gateway.Login`).
    // Skipped for --clear-session: the window starts blank on purpose.
    // Cacat A (3 Nov 2026): hasil load dicatat di baris `init` login.log
    // (path file + ok/gagal + alasan + jumlah akun), bukan hanya jumlah
    // cookie — `saved_cookies=0` saja tidak membedakan "belum ada akun"
    // dari "file gagal dibaca". Bila load gagal, store sudah membackup
    // file ke `accounts.json.corrupt-*` (muse-store) dan jendela tetap
    // jalan tanpa restore (aman: tidak ada yang ditimpa dari sini).
    let accounts_path = settings.accounts_file();
    let (saved_cookies, accounts_load): (Vec<muse_login::SavedCookie>, String) = if clear_session
    {
        (Vec::new(), "skipped (clear-session)".to_string())
    } else {
        let mut store = muse_store::AccountStore::new(accounts_path.clone());
        match store.load() {
            Ok(()) => {
                let count = store.all().len();
                let cookies = store
                    .all()
                    .into_iter()
                    .next()
                    .map(|acc| {
                        acc.cookies
                            .iter()
                            .filter_map(|(name, value)| {
                                value.as_str().map(|v| muse_login::SavedCookie {
                                    name: name.clone(),
                                    value: v.to_string(),
                                    expires_unix: acc
                                        .cookie_expires
                                        .get(name)
                                        .and_then(|e| e.as_f64())
                                        .map(|f| f as i64),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (cookies, format!("ok accounts={count}"))
            }
            Err(e) => (
                Vec::new(),
                format!(
                    "failed: {} (backup: {})",
                    one_line(&e.to_string()),
                    store
                        .load_failure_backup()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
            ),
        }
    };
    // Runtime log (Perbaikan 1, 3 Nov 2026): stage transitions of this
    // login run go to stderr — the Tauri app redirects the child's
    // stdout+stderr to <data_dir>/login.log, so they survive the window
    // closing; run by hand they go to the terminal. Counts only, never
    // cookie/token values. The later stages (cookies-complete,
    // waiting-redirect, session-check-injected, session-message,
    // gateway-post) are logged from muse-login, same process/stderr.
    // The outcome itself is already recorded below ("login complete" on
    // stdout, "muse-gateway login: …" on stderr), so it is not repeated
    // here.
    //
    // Cacat B (3 Nov 2026): the `init` line now also carries the raw
    // `argv` (was `--clear-session` even passed?), the submitted
    // `label` (auto "Account 2" vs a hand-typed label), and the file
    // fingerprint (bytes + sha256 — was the file `[]`/missing, or did
    // it hold 1 account that was read as 0?). The fingerprint is read
    // even when the load is skipped (clear-session), so a later
    // comparison still knows what the file contained.
    muse_login::login_log::log_stage(
        "init",
        &format!(
            "ok, port={}, argv=[login {}], label={}, accounts_file={}, accounts_fingerprint=[{}], accounts_load=[{}], saved_cookies={}, clear_session={}",
            settings.port,
            format_argv(args),
            label,
            accounts_path.display(),
            accounts_fingerprint(&accounts_path),
            accounts_load,
            saved_cookies.len(),
            clear_session
        ),
    );
    match muse_login::show_login(
        &gateway_url,
        &admin_key,
        &label,
        &data_root,
        &saved_cookies,
        clear_session,
    ) {
        Ok(result) => {
            println!("login complete: account {}", result.account_id);
            0
        }
        Err(e) => {
            eprintln!("muse-gateway login: {e}");
            1
        }
    }
}

#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(|s| s.as_str()) == Some("login") {
        std::process::exit(cmd_login(&argv[1..]));
    }
    if argv.first().map(|s| s.as_str()) == Some("--help") || argv.first().map(|s| s.as_str()) == Some("-h") {
        println!("usage: muse-gateway [login [--label <label>] [--clear-session]]");
        return;
    }
    if !argv.is_empty() {
        exit_with(&format!("unknown argument '{}'", argv[0]), 2);
    }

    let settings = Settings::load();
    if let Err(e) = settings.validate() {
        exit_with(&format!("invalid settings: {e}"), 2);
    }
    let settings = match settings.resolve_api_key() {
        Ok(s) => s,
        Err(e) => exit_with(&format!("cannot resolve API key: {e}"), 2),
    };
    if let Err(e) = settings.ensure_dirs() {
        exit_with(&format!("cannot create data dirs: {e}"), 2);
    }

    // Loopback-only enforcement (sidecar contract): refuse any non-loopback
    // host outright instead of silently binding something else.
    let host = settings.host.trim().to_lowercase();
    if host != "127.0.0.1" && host != "::1" && host != "localhost" {
        exit_with(
            &format!(
                "refusing non-loopback host '{host}' (muse-gateway is a loopback-only sidecar)"
            ),
            2,
        );
    }

    let driver: Arc<dyn muse_browser::MuseDriver> = match DriverFactory::create(settings.clone()) {
        Ok(d) => Arc::from(d),
        Err(e) => exit_with(&format!("unknown driver: {e}"), 2),
    };

    // Stores (all under DATA_DIR; a failed request log falls back to
    // :memory: with a stderr warning, like .NET's in-memory fallback).
    let mut pool_inner = AccountPool::new(
        settings.accounts_file(),
        &settings.pool_strategy,
        settings.account_max_concurrency as usize,
        settings.account_cooldown,
        settings.pool_acquire_timeout,
    );
    // Cacat A (3 Nov 2026) — FAIL START, jangan lanjut dengan pool kosong:
    // `accounts.json` ADA tapi gagal dimuat (corrupt / DPAPI / I/O).
    // Perilaku lama ("starting with an empty pool") adalah jalur wipe-nya:
    // POST /admin/accounts berikutnya append akun baru lalu `persist`
    // menimpa file hanya dengan akun itu — akun lama hilang tanpa jejak.
    // Sekarang: file sudah dibackup ke `accounts.json.corrupt-*` oleh
    // `AccountStore::load`, kegagalan dicatat di `<data_dir>/gateway.log`
    // (stdout gateway dibuang Tauri), dan start DIGAGALKAN dengan pesan
    // jelas. Perbaiki / hapus file rusak dulu, baru start lagi. Sebagai
    // pertahanan kedua, `AccountStore::persist` juga menolak menulis
    // selama `load_failed` — jadi kalaupun ada jalur lain yang start
    // dengan pool kosong, file tidak pernah tertimpa.
    if let Err(e) = pool_inner.load() {
        let backup = pool_inner
            .store
            .load_failure_backup()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        append_gateway_log(
            &settings.data_dir,
            &format!(
                "start-failed: cannot load accounts from {}: {}; backup: {}",
                settings.accounts_file().display(),
                one_line(&e.to_string()),
                backup
            ),
        );
        exit_with(
            &format!(
                "cannot load accounts from {}: {e}; refusing to start with an empty pool (the old account would be wiped by the next save). Original file backed up to {backup}; fix or remove it, then start again",
                settings.accounts_file().display()
            ),
            1,
        );
    }
    append_gateway_log(
        &settings.data_dir,
        &format!(
            "start: data_dir={} accounts_file={} accounts={} accounts_fingerprint=[{}] port={}",
            settings.data_dir.display(),
            settings.accounts_file().display(),
            pool_inner.store.all().len(),
            accounts_fingerprint(&settings.accounts_file()),
            settings.port
        ),
    );
    // Cacat B (3 Nov 2026): loading 0 accounts from an EXISTING file
    // is suspicious (12:27:07: file read as 0 while the previous start
    // read 1) — flag it loudly so the next occurrence is noticed at
    // start, not after an account has already been replaced. A missing
    // file (first run) is normal and stays silent.
    if settings.accounts_file().exists() && pool_inner.store.all().is_empty() {
        append_gateway_log(
            &settings.data_dir,
            &format!(
                "start-warning: accounts file exists but holds 0 accounts (fingerprint [{}]); \
                 if a second gateway/process shares this data dir, its empty pool can overwrite the file — check for orphaned gateways before saving a login",
                accounts_fingerprint(&settings.accounts_file())
            ),
        );
    }
    let pool = Arc::new(Mutex::new(pool_inner));
    let http_client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut keys = KeyStore::new(settings.keys_file());
    if let Err(e) = keys.load() {
        eprintln!("muse-gateway: cannot load keys: {e}; starting with no stored keys");
    }
    let mut tasks = TaskStore::new(settings.tasks_file());
    if let Err(e) = tasks.load() {
        eprintln!("muse-gateway: cannot load tasks: {e}; starting with no tasks");
    }
    let requests = match RequestLog::open(settings.requests_file()) {
        Ok(log) => Arc::new(log),
        Err(e) => {
            eprintln!("muse-gateway: cannot open request log: {e}; using :memory:");
            Arc::new(
                RequestLog::open(":memory:")
                    .unwrap_or_else(|_| panic!("muse-gateway: cannot open :memory: request log")),
            )
        }
    };
    let media = match MediaStore::new(settings.media_dir()) {
        Ok(m) => Arc::new(m),
        Err(e) => exit_with(&format!("cannot open media store: {e}"), 2),
    };
    let skills = match SkillStore::new(settings.data_dir.join("skills")) {
        Ok(s) => Arc::new(s),
        Err(e) => exit_with(&format!("cannot open skill store: {e}"), 2),
    };
    // `.NET AgentEndpoints.cs:494`: the chat session store is rooted at
    // the manager's SessionsRoot (`<data>/agent-sessions`), NOT at the
    // data dir itself — otherwise persisted sessions land in
    // `<data>/<SHA256>/` and `restore_persisted_sessions` (which scans
    // `<data>/agent-sessions/**`) never finds them.
    let chat_sessions = Arc::new(ChatSessionStore::new(
        settings.data_dir.join("agent-sessions"),
    ));
    let sessions = AgentSessionManager::new(Some(settings.data_dir.as_path()));

    let screen = Arc::new(DesktopComputer::new());
    let agent_driver: Arc<dyn muse_agent::driver::MuseDriver> = Arc::new(
        AgentDriverAdapter::new(
            driver.clone(),
            Arc::clone(&pool),
            settings.max_failover,
            http_client.clone(),
        ),
    );
    let screen_driver: Arc<dyn muse_agent::screen::ScreenDriver> =
        Arc::new(ScreenDriverAdapter::new(screen.clone()));

    let state = Arc::new(AppState {
        settings: settings.clone(),
        driver: driver.clone(),
        agent_driver,
        pool: Arc::clone(&pool),
        keys: Arc::new(Mutex::new(keys)),
        tasks: Arc::new(Mutex::new(tasks)),
        requests: requests.clone(),
        media,
        skills,
        chat_sessions,
        sessions: tokio::sync::Mutex::new(sessions),
        checked_out: Mutex::new(HashMap::new()),
        deleted_while_running: Mutex::new(std::collections::HashSet::new()),
        screen,
        screen_driver,
        http_client,
        task_cancels: Mutex::new(HashMap::new()),
    });

    // Restore persisted agent sessions before the driver starts.
    crate::adapters::restore_persisted_sessions(&state).await;

    if let Err(e) = driver.startup().await {
        exit_with(&format!("driver startup failed: {e}"), 1);
    }

    // One-shot housekeeping at start (`.NET GatewayLifetime` prunes on a
    // timer; the gateway only prunes once at startup — see VERIFY.md).
    if let Ok(pruned) = requests.prune(settings.request_log_retention_days) {
        if pruned > 0 {
            eprintln!("muse-gateway: pruned {pruned} request log rows");
        }
    }
    let media_pruned = state.media.prune_retention_days(settings.media_retention_days);
    if media_pruned > 0 {
        eprintln!("muse-gateway: pruned {media_pruned} media files");
    }

    let app = routes::router(Arc::clone(&state))
        .layer(axum::middleware::from_fn(crate::cors::loopback_only));

    let ip: std::net::IpAddr = if host == "::1" {
        std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
    } else {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    };
    let addr = SocketAddr::new(ip, settings.port);
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => exit_with(&format!("cannot bind {addr}: {e}"), 2),
    };
    println!(
        "muse-gateway 0.9.1 listening on http://{addr} (driver: {})",
        settings.driver
    );
    if let Err(e) = axum::serve(listener, app).await {
        exit_with(&format!("server error: {e}"), 1);
    }

    // Graceful shutdown (`.NET GatewayLifetime.StopAsync`): stop the driver
    // and flush the key store.
    let _ = driver.shutdown().await;
    if let Ok(mut keys) = state.keys.lock() {
        let _ = keys.flush();
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // Cacat B (3 Nov 2026): the add-account flag arrived as
    // `clear_session=false` at the child twice. The Rust parser must
    // accept every spelling a caller can produce — and fail loudly on
    // the ones it cannot — so a dropped flag can never again look like
    // a normal-mode launch.
    #[test]
    fn parse_login_args_clear_session_forms() {
        // Bare flag (what Tauri passes).
        assert_eq!(
            parse_login_args(&args(&["--clear-session"])).unwrap(),
            LoginArgs {
                label: "default".to_string(),
                clear_session: true,
            }
        );
        // Explicit true/false spellings.
        assert!(parse_login_args(&args(&["--clear-session=true"]))
            .unwrap()
            .clear_session);
        assert!(!parse_login_args(&args(&["--clear-session=false"]))
            .unwrap()
            .clear_session);
        // Order-independent with --label (both spellings).
        let parsed =
            parse_login_args(&args(&["--label", "Account 2", "--clear-session"])).unwrap();
        assert_eq!(parsed.label, "Account 2");
        assert!(parsed.clear_session);
        let parsed = parse_login_args(&args(&["--clear-session", "--label=Akun 3"])).unwrap();
        assert_eq!(parsed.label, "Akun 3");
        assert!(parsed.clear_session);
        // No flag at all = normal mode (label still parsed).
        let parsed = parse_login_args(&args(&["--label", "Personal"])).unwrap();
        assert_eq!(parsed.label, "Personal");
        assert!(!parsed.clear_session);
    }

    #[test]
    fn parse_login_args_rejects_bad_spellings_loudly() {
        // A misspelled flag value must fail, not launch normal-mode.
        assert!(parse_login_args(&args(&["--clear-session=maybe"])).is_err());
        assert!(parse_login_args(&args(&["--clear-session="])).is_err());
        assert!(parse_login_args(&args(&["--unknown"])).is_err());
        assert!(parse_login_args(&args(&["--label"])).is_err());
    }

    #[test]
    fn format_argv_quotes_spaced_args() {
        assert_eq!(
            format_argv(&args(&["--label", "Account 2", "--clear-session"])),
            "--label \"Account 2\" --clear-session"
        );
        assert_eq!(format_argv(&args(&[])), "");
    }

    #[test]
    fn accounts_fingerprint_distinguishes_empty_from_one_account() {
        let dir = std::env::temp_dir()
            .join(format!("muse-gw-fingerprint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("accounts.json");
        assert_eq!(accounts_fingerprint(&file), "missing");
        std::fs::write(&file, "[]").unwrap();
        let empty = accounts_fingerprint(&file);
        assert!(empty.starts_with("bytes=2 sha256="), "{empty}");
        std::fs::write(&file, "[{\"Id\": \"acc_0123456789\"}]").unwrap();
        let one = accounts_fingerprint(&file);
        assert_ne!(empty, one, "different contents must fingerprint differently");
        assert!(one.starts_with("bytes=26 sha256="), "{one}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
