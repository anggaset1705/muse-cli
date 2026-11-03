//! Driver browser Chromium via CDP ([`chromiumoxide`]) — port
//! `BrowserDriver.cs` (.NET, Microsoft.Playwright).
//!
//! Desain (sama seperti .NET): SATU proses Chromium; tiap akun mendapat
//! konteks browser incognito sendiri (cookie jar terisolasi) via
//! `Target.createBrowserContext`, sehingga ganti akun tidak perlu
//! membersihkan cookie. Satu tab per akun, dipakai ulang antar request.
//! Serialisasi per akun dijamin oleh kunci per akun (default
//! `MUSE2API_ACCOUNT_MAX_CONCURRENCY=1`, sama seperti .NET).
//!
//! Perbedaan vs .NET yang disengaja — lihat `TRADEOFFS.md` untuk daftar
//! lengkap; ringkasnya:
//! - Playwright tidak punya SDK Rust; CDP dipakai langsung. Tidak ada
//!   `BrowserContext` persisten Playwright: konteks dibuat via CDP dan
//!   state disimpan sebagai file `storageState` Playwright
//!   (`BrowserSessionStore`).
//! - TIDAK ada pool multi-tab per akun (`.NET: Checkout`): satu tab per
//!   akun + kunci per akun. `AccountMaxConcurrency > 1` tetap diserialisasi
//!   (perilaku aman, lebih lambat bila >1).
//! - Tidak ada `user_data_dir` persisten: profil Chromium ephemeral
//!   (sama seperti .NET yang tidak pernah meneruskan `ProfileDir` ke
//!   Playwright); persistensi sesi HANYA via file storage-state.
//! - `LocalConversationStore` (URL thread per `conversation_hint`) belum
//!   ada di crate ini — itu milik `muse-store`; jalur local-tools hanya
//!   memakai tab live. Lihat `TRADEOFFS.md`.
//! - `hot_page` (`.NET: account.Meta["hot_page"]`) hanya disimpan di memori
//!   tab, bukan ditulis ke `accounts.json` (gateway yang memiliki akun).
//! - Deteksi 429: selain `ChatState.rateLimited` (DOM), dipasang juga
//!   listener CDP `Network.responseReceived` (`.NET: page.Response`
//!   di `OpenTab`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chromiumoxide::browser::HeadlessMode;
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType,
};
use chromiumoxide::cdp::browser_protocol::network::{
    Cookie, CookieParam, CookieSameSite, EventResponseReceived, TimeSinceEpoch,
};
use chromiumoxide::cdp::browser_protocol::storage::SetCookiesParams as StorageSetCookiesParams;
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams, DisposeBrowserContextParams,
};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::layout::Point;
use chromiumoxide::{Browser, BrowserConfig as ChromiumConfig, Page};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::DriverSettings;
use crate::dom::{self, UsageInfo};
use crate::driver::{ChatStream, MuseDriver};
use crate::error::MuseError;
use muse_core::Prompt;
use crate::session_store::BrowserSessionStore;
use crate::types::{
    Account, ChatEvent, ChatRequest, DriverCapabilities, ImageRequest, InputImage, MediaResult,
    SessionInfo, SessionRefresh, VideoRequest,
};
use crate::upstream::Upstream;

/// Alias pengaturan driver (satu definisi saja — lihat `config.rs`).
pub type BrowserConfig = DriverSettings;

// ---------------------------------------------------------------------------
// JSON hasil eval JS (Dom.cs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PageStateJson {
    ready: bool,
    has_input: bool,
    path: String,
    head: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct AttachmentJson {
    tid: String,
    kind: String,
    src: String,
    w: i64,
    h: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ChatStateJson {
    agent_count: usize,
    last_text: String,
    reply_key: String,
    reply_keys: Vec<String>,
    user_key: String,
    reply_after_user: bool,
    rate_limited: bool,
    error: String,
    local_request_id: String,
    local_reply_bound: bool,
    local_reply_text: String,
    generating: bool,
    approval: bool,
    attachments: Vec<AttachmentJson>,
    tail: String,
}

// ---------------------------------------------------------------------------
// State tab + driver
// ---------------------------------------------------------------------------

/// Port `BrowserTab` (`BrowserDriver.cs`).
struct TabState {
    page: Page,
    context_id: BrowserContextId,
    hint: Option<String>,
    turns: Vec<(String, String)>,
    image_count: usize,
    thread_url: String,
    local: bool,
    rate_limited: Arc<AtomicBool>,
    listener_task: Option<JoinHandle<()>>,
    busy: bool,
    stale: bool,
    quota_note: String,
}

impl TabState {
    fn new(page: Page, context_id: BrowserContextId, rate_limited: Arc<AtomicBool>) -> Self {
        Self {
            page,
            context_id,
            hint: None,
            turns: Vec::new(),
            image_count: 0,
            thread_url: String::new(),
            local: false,
            rate_limited,
            listener_task: None,
            busy: false,
            stale: false,
            quota_note: String::new(),
        }
    }

    /// Port `BrowserTab.HasState` — dipakai saat pool multi-tab ditambah
    /// (lihat TRADEOFFS.md butir 2); saat ini satu tab per akun.
    #[allow(dead_code)]
    fn has_state(&self) -> bool {
        !self.turns.is_empty()
            || !self.thread_url.is_empty()
            || (self.local && self.hint.as_deref().map(|h| !h.is_empty()).unwrap_or(false))
    }
}

struct SharedState {
    // Arc karena chromiumoxide::Browser tidak Clone; Drop-nya membunuh
    // child Chromium (kill_on_drop), jadi drop Arc terakhir aman.
    browser: Option<Arc<Browser>>,
    handler: Option<JoinHandle<()>>,
    tabs: HashMap<String, Arc<Mutex<TabState>>>,
    context_auth: HashMap<String, String>,
    reset_contexts: HashSet<String>,
    /// Profil Chromium ephemeral unik per launch (dibersihkan saat shutdown).
    profile_dir: Option<PathBuf>,
}

struct Inner {
    settings: DriverSettings,
    sessions: BrowserSessionStore,
    state: Mutex<SharedState>,
    account_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    refreshed: Mutex<HashMap<String, SessionRefresh>>,
    quota_cache: Mutex<HashMap<String, (Instant, HashMap<String, serde_json::Value>)>>,
}

/// Driver browser Chromium (CDP). Clone murah (Arc di dalam).
#[derive(Clone)]
pub struct BrowserDriver {
    inner: Arc<Inner>,
}

impl BrowserDriver {
    pub fn new(settings: DriverSettings) -> Self {
        let sessions = BrowserSessionStore::new(settings.data_dir.clone());
        Self {
            inner: Arc::new(Inner {
                settings,
                sessions,
                state: Mutex::new(SharedState {
                    browser: None,
                    handler: None,
                    tabs: HashMap::new(),
                    context_auth: HashMap::new(),
                    reset_contexts: HashSet::new(),
                    profile_dir: None,
                }),
                account_locks: Mutex::new(HashMap::new()),
                refreshed: Mutex::new(HashMap::new()),
                quota_cache: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Cari executable Chromium: `MUSE2API_CHROMIUM_PATH` dulu, lalu nama
    /// umum di PATH, lalu `/opt/meta-chromium/chrome` (Chromium Hatch
    /// Meta di VM ini).
    ///
    /// PERHATIAN: `/opt/meta-chromium/chrome` TIDAK bisa start standalone
    /// — binary Hatch itu menolak start tanpa WebGL domain policy dari
    /// browser-daemon (`chrome_main_delegate.cc:1212`, "Hatch WebGL domain
    /// policy is required for browser startup"; README-nya:
    /// "Normal startup requires the image-owned WebGL policy supplied by
    /// browser-daemon"). Kandidat itu ditaruh PALING AKHIR; di VM ini
    /// Chrome stock (`/usr/bin/google-chrome`, diinstal 2 Nov 2026
    /// untuk pengujian) yang terpilih. Di PC Windows Bree,
    /// `MUSE2API_CHROMIUM_PATH` harus menunjuk Chromium Playwright
    /// (`chromium-1248`) atau Chrome stock.
    /// `None` = biarkan chromiumoxide mendeteksi sendiri (jalur Playwright
    /// `playwright.ps1 install chromium` tidak ada di Rust).
    fn resolve_chromium_path(settings: &DriverSettings) -> Option<PathBuf> {
        if !settings.chromium_path.is_empty() {
            let p = PathBuf::from(&settings.chromium_path);
            if p.exists() {
                return Some(p);
            }
            // Tetap teruskan apa adanya — chromiumoxide akan memberi error
            // yang jelas bila path tidak ada (lebih baik daripada diam-diam
            // memakai browser lain).
            return Some(p);
        }
        for name in [
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
        ] {
            for dir in ["/usr/bin", "/usr/local/bin", "/opt/google/chrome"] {
                let p = PathBuf::from(dir).join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        let meta = PathBuf::from("/opt/meta-chromium/chrome");
        if meta.exists() {
            return Some(meta);
        }
        None
    }

    // -- lifecycle ---------------------------------------------------------

    /// Pick a free loopback port by binding 127.0.0.1:0 and releasing it.
    /// Same small-TOCTOU pattern as `pick_free_port` in the Tauri sidecar;
    /// the Chromium launch fails loudly if the port is taken meanwhile.
    fn pick_free_port() -> Result<u16, MuseError> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| {
            MuseError::internal(format!("cannot pick a free CDP port: {e}"))
        })?;
        listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|e| MuseError::internal(format!("cannot read free CDP port: {e}")))
    }

    async fn ensure_started(&self) -> Result<(), MuseError> {
        let mut state = self.inner.state.lock().await;
        if state.browser.is_some() {
            return Ok(());
        }
        let settings = &self.inner.settings;
        // CDP port: 0 (default) = pick a free loopback port per launch, so
        // an orphaned Chrome from a force-killed gateway cannot squat a
        // fixed port and break the next launch. An explicit
        // MUSE2API_CDP_PORT is used as-is (diagnostics).
        let cdp_port = if settings.cdp_port == 0 {
            Self::pick_free_port()?
        } else {
            settings.cdp_port
        };
        let mut builder = ChromiumConfig::builder();
        builder = builder
            .headless_mode(if settings.headless {
                HeadlessMode::New
            } else {
                HeadlessMode::False
            })
            .window_size(1440, 2200)
            .viewport(Viewport {
                width: 1440,
                height: 2200,
                device_scale_factor: None,
                emulating_mobile: false,
                is_landscape: false,
                has_touch: false,
            })
            .port(cdp_port)
            .launch_timeout(Duration::from_secs(60));
        // Arg launch .NET (BrowserDriver.StartupAsync).
        for arg in [
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-dev-shm-usage",
            "--disable-background-networking",
            "--autoplay-policy=no-user-gesture-required",
            "--window-size=1440,2200",
        ] {
            builder = builder.arg(arg);
        }
        // VM ini jalan sebagai root — Chrome menolak start tanpa
        // --no-sandbox ("Running as root without --no-sandbox is not
        // supported"). Di PC Bree (Windows, user biasa) sandbox tetap
        // nyala; .NET Playwright tidak pernah mematikan sandbox.
        if cfg!(unix) && is_root() {
            builder = builder.no_sandbox();
        }
        if !settings.browser_proxy.is_empty() {
            builder = builder.arg(format!("--proxy-server={}", settings.browser_proxy));
        }
        if let Some(exe) = Self::resolve_chromium_path(settings) {
            builder = builder.chrome_executable(exe);
        }
        // Profil ephemeral UNIK per launch. chromiumoxide default-nya satu
        // dir tetap (/tmp/chromiumoxide-runner) — dua launch bersamaan
        // akan saling mengunci SingletonLock dan gagal. .NET Playwright
        // juga memakai profil temp baru per launch; persistensi sesi
        // tetap via file storage-state (bukan profil).
        let profile_dir = std::env::temp_dir().join(format!(
            "muse-browser-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        builder = builder.user_data_dir(profile_dir.clone());
        let config = builder.build().map_err(|e| {
            MuseError::browser_not_installed(format!("invalid browser config: {e}"))
        })?;
        let (browser, mut handler) = Browser::launch(config).await.map_err(|e| {
            MuseError::browser_not_installed(format!(
                "Could not launch Chromium via CDP. Set MUSE2API_CHROMIUM_PATH to a Chromium/Chrome executable. Original error: {e}"
            ))
        })?;
        // Handler HARUS terus dipolling (seperti contoh chromiumoxide).
        let handler_task = tokio::spawn(async move {
            while let Some(_) = handler.next().await {}
        });
        state.browser = Some(Arc::new(browser));
        state.handler = Some(handler_task);
        state.profile_dir = Some(profile_dir);
        Ok(())
    }

    async fn browser_handle(&self) -> Result<Arc<Browser>, MuseError> {
        self.ensure_started().await?;
        let state = self.inner.state.lock().await;
        state
            .browser
            .clone()
            .ok_or_else(|| MuseError::internal("browser is not started"))
    }

    async fn account_lock(&self, account_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.inner.account_locks.lock().await;
        locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    // -- context + tab -----------------------------------------------------

    /// Port `GetContext` + `OpenTab` (.NET): konteks incognito per akun +
    /// satu page + cookie akun + listener 429.
    async fn get_or_create_tab(
        &self,
        account: &Account,
    ) -> Result<Arc<Mutex<TabState>>, MuseError> {
        // Tab lama yang sudah ditandai stale dibuang dulu (port Checkin .NET
        // yang menutup tab Stale saat check-in).
        let existing = {
            let state = self.inner.state.lock().await;
            state.tabs.get(&account.id).cloned()
        };
        if let Some(tab) = existing {
            let stale = tab.try_lock().map(|t| t.stale).unwrap_or(false);
            if !stale {
                return Ok(tab);
            }
            self.close_tab_locked(&account.id, tab).await;
        }
        let missing = Upstream::missing_session_cookies(&account.cookies);
        if !missing.is_empty() {
            return Err(MuseError::upstream_auth(format!(
                "account is missing session cookies: {}",
                missing.join(", ")
            )));
        }
        let browser = self.browser_handle().await?;
        let ctx = browser
            .execute(CreateBrowserContextParams::default())
            .await
            .map_err(|e| MuseError::internal(format!("create browser context: {e}")))?
            .result
            .browser_context_id;

        // Cookie: storage-state tersimpan dulu, lalu cookie akun menimpa
        // (urutan .NET: NewContextAsync(StorageState) lalu SetCookies).
        // Cookie dipasang di LEVEL CONTEXT via CDP `Storage.setCookies`
        // dengan `browserContextId` — persis `context.AddCookiesAsync`
        // .NET — SEBELUM page dibuat. JANGAN `page.set_cookies` pada
        // page `about:blank`: chromiumoxide menolaknya ("Blank page can
        // not have cookie", `validate_cookie_url` di page.rs).
        let mut cookies: Vec<CookieParam> = Vec::new();
        if let Some(state_json) = self.inner.sessions.load(account) {
            cookies.extend(storage_state_cookies(&state_json));
        }
        cookies.extend(account_cookies(account));
        if !cookies.is_empty() {
            let params = StorageSetCookiesParams::builder()
                .cookies(cookies)
                .browser_context_id(ctx.clone())
                .build()
                .map_err(|e| MuseError::internal(format!("set cookies: {e}")))?;
            if let Err(e) = browser.execute(params).await {
                // Konteks kosong jangan dibiarkan bocor bila set-cookie
                // gagal (mis. cookie tidak valid) — tab tidak jadi dibuat.
                let _ = browser
                    .execute(DisposeBrowserContextParams::new(ctx))
                    .await;
                return Err(MuseError::internal(format!("set cookies: {e}")));
            }
        }

        let page = browser
            .new_page(
                CreateTargetParams::builder()
                    .browser_context_id(ctx.clone())
                    .url("about:blank")
                    .build()
                    .map_err(|e| MuseError::internal(format!("create target: {e}")))?,
            )
            .await
            .map_err(|e| MuseError::internal(format!("new page: {e}")))?;

        let rate_limited = Arc::new(AtomicBool::new(false));
        // Port `page.Response` .NET (OpenTab): 429 dari muse.ai /api/*.
        let listener_task = spawn_rate_limit_listener(page.clone(), rate_limited.clone()).await;
        let mut tab = TabState::new(page, ctx.clone(), rate_limited);
        tab.listener_task = listener_task;
        let tab = Arc::new(Mutex::new(tab));
        let mut state = self.inner.state.lock().await;
        // Balapan: tab lain mungkin sudah dibuat selagi await di atas.
        if let Some(existing) = state.tabs.get(&account.id) {
            return Ok(existing.clone());
        }
        state
            .context_auth
            .insert(account.id.clone(), BrowserSessionStore::authentication_hash(account));
        state.tabs.insert(account.id.clone(), tab.clone());
        Ok(tab)
    }

    async fn close_tab_locked(&self, account_id: &str, tab: Arc<Mutex<TabState>>) {
        let (page, ctx, listener) = {
            let mut t = tab.lock().await;
            if let Some(h) = t.listener_task.take() {
                h.abort();
            }
            (t.page.clone(), t.context_id.clone(), ())
        };
        let _ = listener;
        let _ = page.close().await;
        if let Ok(browser) = self.browser_handle().await {
            let _ = browser
                .execute(DisposeBrowserContextParams::new(ctx))
                .await;
        }
        let mut state = self.inner.state.lock().await;
        state.tabs.remove(account_id);
        state.context_auth.remove(account_id);
    }

    // -- eval --------------------------------------------------------------

    /// Port `Eval` (.NET): evaluasi JS, timeout 10 dtk, gagal → `None`
    /// (bukan error).
    async fn eval(page: &Page, script: &str, await_promise: bool) -> Option<serde_json::Value> {
        let params = EvaluateParams::builder()
            .expression(script)
            .return_by_value(true)
            .await_promise(await_promise)
            .build()
            .ok()?;
        let fut = page.evaluate_expression(params);
        match tokio::time::timeout(Duration::from_secs(10), fut).await {
            Ok(Ok(result)) => result.value().cloned(),
            _ => None,
        }
    }

    async fn page_state(page: &Page) -> Option<PageStateJson> {
        let v = Self::eval(page, dom::PAGE_STATE, false).await?;
        serde_json::from_value(v).ok()
    }

    async fn chat_state(page: &Page, local_request_id: Option<&str>) -> Option<ChatStateJson> {
        let script = dom::read_chat_state(local_request_id);
        let v = Self::eval(page, &script, false).await?;
        serde_json::from_value(v).ok()
    }

    // -- navigasi ----------------------------------------------------------

    /// Port `NewThread` (.NET).
    async fn new_thread(&self, tab: &mut TabState) -> Result<(), MuseError> {
        forget(tab);
        tab.page
            .goto(crate::config::new_thread_url(&self.inner.settings))
            .await
            .map_err(|e| MuseError::internal(format!("goto new thread: {e}")))?;
        let deadline = Instant::now() + Duration::from_secs_f64(self.inner.settings.page_ready_timeout);
        let mut state = None;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
            state = Self::page_state(&tab.page).await;
            if state.as_ref().map(|s| s.ready).unwrap_or(false) {
                return Ok(());
            }
        }
        let head = state
            .as_ref()
            .map(|s| s.head.to_lowercase())
            .unwrap_or_default();
        if dom::LOGIN_HINTS.iter().any(|h| head.contains(h)) {
            return Err(MuseError::upstream_auth(
                "muse.ai redirected to login; cookies are no longer valid",
            ));
        }
        Err(MuseError::upstream_timeout("muse.ai page did not become ready"))
    }

    /// Port `OnThread` (.NET).
    async fn on_thread(&self, tab: &TabState) -> Result<bool, MuseError> {
        let href = tab
            .page
            .url()
            .await
            .map_err(|e| MuseError::internal(format!("page url: {e}")))?
            .unwrap_or_default();
        if !is_thread_url(&href) {
            return Ok(false);
        }
        if !tab.thread_url.is_empty()
            && url_path(&href).trim_end_matches('/')
                != url_path(&tab.thread_url).trim_end_matches('/')
        {
            return Ok(false);
        }
        let page = Self::page_state(&tab.page).await;
        if !page.as_ref().map(|p| p.has_input).unwrap_or(false) {
            return Ok(false);
        }
        if page.as_ref().map(|p| p.head.contains("Connecting...")).unwrap_or(false) {
            return Ok(false);
        }
        let state = self.chat_state_checked(tab).await?;
        if state.generating {
            return Ok(false);
        }
        if dom::STALL_HINTS
            .iter()
            .any(|h| state.tail.contains(h))
        {
            return Ok(false);
        }
        Ok(tab.local || state.agent_count < HOT_BUBBLE_LIMIT)
    }

    /// Port `LiveContinuation` (.NET).
    async fn live_continuation(&self, tab: &TabState) -> Result<bool, MuseError> {
        let last_user = tab
            .turns
            .iter()
            .rev()
            .find(|(r, _)| r == "user")
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        if last_user.is_empty() {
            return Ok(false);
        }
        let page = Self::page_state(&tab.page).await;
        if !page.as_ref().map(|p| p.has_input).unwrap_or(false) {
            return Ok(false);
        }
        let state = self.chat_state_checked(tab).await?;
        let head: String = last_user.chars().take(80).collect();
        let present = Self::eval(&tab.page, &dom::body_includes(&head), false).await;
        if state.generating {
            return Ok(false);
        }
        if dom::STALL_HINTS
            .iter()
            .any(|h| state.tail.contains(h))
        {
            return Ok(false);
        }
        if state.agent_count >= HOT_BUBBLE_LIMIT {
            return Ok(false);
        }
        Ok(present
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Port `OpenSaved` (.NET).
    async fn open_saved(&self, tab: &mut TabState) -> Result<(), MuseError> {
        let url = tab.thread_url.clone();
        tab.page
            .goto(url)
            .await
            .map_err(|e| MuseError::internal(format!("goto saved thread: {e}")))?;
        let deadline = Instant::now() + Duration::from_secs_f64(self.inner.settings.page_ready_timeout);
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if self.on_thread(tab).await? {
                return Ok(());
            }
        }
        Err(MuseError::upstream_timeout(
            "saved conversation did not become ready",
        ))
    }

    /// Port `ChatState` + pemeriksaan error/kuota (.NET `ChatState(tab)`).
    async fn chat_state_checked(&self, tab: &TabState) -> Result<ChatStateJson, MuseError> {
        self.chat_state_checked_req(tab, None).await
    }

    async fn chat_state_checked_req(
        &self,
        tab: &TabState,
        local_request_id: Option<&str>,
    ) -> Result<ChatStateJson, MuseError> {
        if tab.rate_limited.load(Ordering::Relaxed) {
            return Err(MuseError::upstream_quota(
                "Muse returned rate limit (HTTP 429). Conversation retained; no automatic retry or replacement chat.",
            ));
        }
        let state = Self::chat_state(&tab.page, local_request_id)
            .await
            .ok_or_else(|| {
                MuseError::upstream_response_unavailable(
                    "Muse web state unavailable; stopped waiting. Conversation retained.",
                )
            })?;
        self.check_chat_state(tab, &state).await?;
        Ok(state)
    }

    /// Pemeriksaan error/kuota dari satu `ChatState` — dipakai ulang oleh
    /// loop chat (yang tidak boleh memanggil `chat_state_checked` dua kali
    /// per putaran). Port bagian `ChatState(tab)` di `BrowserDriver.cs`.
    async fn check_chat_state(
        &self,
        tab: &TabState,
        state: &ChatStateJson,
    ) -> Result<(), MuseError> {
        if state.rate_limited {
            return Err(MuseError::upstream_quota(
                "Muse shows a rate limit notice. Conversation retained; no automatic retry or replacement chat.",
            ));
        }
        if !state.error.trim().is_empty() {
            return Err(MuseError::upstream_response_unavailable(format!(
                "Muse web stopped responding: {}",
                state.error
            )));
        }
        let tail = &state.tail;
        let low = tail.to_lowercase();
        for hint in dom::QUOTA_HINTS {
            if !low.contains(hint) {
                continue;
            }
            let i = low.find(hint).unwrap_or(0);
            let snippet = char_slice(&low, i.saturating_sub(120), i + 120)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let reply = state.last_text.to_lowercase();
            if !state.generating && reply.contains(hint) {
                return Err(MuseError::upstream_quota(format!(
                    "muse.ai reports the account is out of quota: …{snippet}…"
                )));
            }
            if snippet != tab.quota_note {
                // .NET: Console.WriteLine("ignoring quota hint ...")
                eprintln!("ignoring quota hint '{hint}' outside the reply: {snippet}");
            }
        }
        Ok(())
    }

    // -- kirim -------------------------------------------------------------

    /// Port `Attach` (.NET).
    async fn attach(&self, tab: &TabState, images: &[InputImage]) -> Result<(), MuseError> {
        use base64::Engine;
        for (idx, img) in images.iter().enumerate() {
            let ext = img
                .mime
                .split('/')
                .last()
                .unwrap_or("png")
                .replace("jpeg", "jpg");
            let b64 = base64::engine::general_purpose::STANDARD.encode(&img.data);
            let res = Self::eval(
                &tab.page,
                &dom::attach_file(&b64, &img.mime, &format!("input_{idx}.{ext}")),
                false,
            )
            .await;
            let ok = res
                .as_ref()
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !ok {
                let err = res
                    .as_ref()
                    .and_then(|v| v.get("err"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return Err(MuseError::internal(format!("failed to attach image: {err}")));
            }
        }
        if !images.is_empty() {
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        Ok(())
    }

    /// Port `Send` (.NET): isi input → klik kirim (≤5 dtk) → fallback
    /// tombol Enter → cek input kosong.
    async fn send(&self, tab: &TabState, prompt_text: &str) -> Result<(), MuseError> {
        let res = Self::eval(&tab.page, &dom::fill_input(prompt_text), false).await;
        let ok = res
            .as_ref()
            .and_then(|v| v.get("ok"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ok {
            let err = res
                .as_ref()
                .and_then(|v| v.get("err"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Err(MuseError::internal(format!(
                "cannot fill chat input: {err}"
            )));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let clicked = Self::eval(&tab.page, dom::CLICK_SEND, false).await;
            if clicked.as_ref().and_then(|v| v.as_str()) == Some("clicked") {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        press_key(&tab.page, "Enter", "Enter", 13, Some("\r")).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let empty = Self::eval(&tab.page, dom::INPUT_EMPTY, false).await;
        if empty.and_then(|v| v.as_bool()).unwrap_or(false) {
            Ok(())
        } else {
            Err(MuseError::internal(
                "send button not found and Enter did not submit",
            ))
        }
    }

    /// Port `DenyApprovals` (.NET).
    async fn deny_approvals(&self, tab: &TabState) {
        if let Some(denied) = Self::eval(&tab.page, dom::DENY_APPROVALS, false).await {
            if let Some(arr) = denied.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        eprintln!("denied muse.ai {}", s.to_lowercase());
                    }
                }
            }
        }
    }

    // -- chat --------------------------------------------------------------

    /// Satu putaran chat penuh (dipanggil dari task `chat_stream`).
    /// Mengirim `AssistantDelta` lewat `tx`; `Done` dikirim pemanggil.
    async fn run_chat(
        &self,
        account: Account,
        req: ChatRequest,
        tx: tokio::sync::mpsc::Sender<Result<ChatEvent, MuseError>>,
    ) -> Result<(), MuseError> {
        // Serialisasi per akun (seperti AccountMaxConcurrency=1 .NET).
        let lock = self.account_lock(&account.id).await;
        let _guard = lock.lock().await;
        let tab_arc = self.get_or_create_tab(&account).await?;
        let mut tab = tab_arc.lock().await;
        tab.busy = true;
        let result = self.run_chat_locked(&req, &mut tab, &tx).await;
        let finished = result.is_ok();
        // -- finally (.NET ChatStream finally) --
        if req.local_tools {
            tab.thread_url = tab.page.url().await.ok().flatten().unwrap_or_default();
            tab.hint = req.conversation_hint.clone();
            tab.local = true;
            tab.turns.clear();
            if !finished {
                tab.stale = true;
            }
        } else if finished {
            if let Ok(Some(url)) = tab.page.url().await {
                if is_thread_url(&url) {
                    tab.thread_url = url;
                }
            }
            save_hot(&mut tab, &req);
        } else {
            forget(&mut tab);
            tab.stale = true;
        }
        // SaveSession — selalu dicoba, gagal = diam (seperti .NET).
        let refresh = self.save_session(&account, &tab).await;
        tab.busy = false;
        let stale = tab.stale;
        drop(tab);
        if let Some(r) = refresh {
            let mut m = self.inner.refreshed.lock().await;
            m.insert(r.account_id.clone(), r);
        }
        if stale {
            self.close_tab_locked(&account.id, tab_arc).await;
        }
        result
    }

    async fn run_chat_locked(
        &self,
        req: &ChatRequest,
        tab: &mut TabState,
        tx: &tokio::sync::mpsc::Sender<Result<ChatEvent, MuseError>>,
    ) -> Result<(), MuseError> {
        let hint = req.conversation_hint.clone().unwrap_or_default();
        if req.local_tools {
            let request_id = self.begin_local(req, tab).await?;
            return self
                .poll_local(req, tab, &request_id, tx)
                .await;
        }
        // -- BeginChat non-local (.NET) --
        let mut prompt_text = req.prompt.clone();
        let mut images: Vec<InputImage> = req.images.clone();
        let follow = if tab.turns.is_empty() {
            None
        } else {
            Prompt::followup_text(&tab.turns, &req.turns)
        };
        let same_owner = tab.hint.as_deref().unwrap_or("") == hint;
        if let Some(f) = follow {
            if same_owner && req.images.len() >= tab.image_count {
                let on_thread = self.on_thread(tab).await?;
                let live = if !on_thread {
                    self.live_continuation(tab).await?
                } else {
                    true
                };
                if on_thread || live {
                    // lanjut di thread yang sama
                } else if !tab.thread_url.is_empty() {
                    self.open_saved(tab).await?;
                }
                prompt_text = f;
                images = req.images.iter().skip(tab.image_count).cloned().collect();
            } else {
                self.new_thread(tab).await?;
            }
        } else {
            // hot_page dari proses lain tidak bisa dibaca di sini (meta akun
            // dimiliki gateway) — tab in-memory adalah sumber hot state.
            self.new_thread(tab).await?;
        }
        let base_state = self.chat_state_checked(tab).await?;
        let base_count = base_state.agent_count;
        let base_text = base_state.last_text.clone();
        let base_user_key = base_state.user_key.clone();
        let base_reply_keys: HashSet<String> =
            base_state.reply_keys.iter().cloned().collect();
        self.attach(tab, &images).await?;
        self.send(tab, &prompt_text).await?;
        self.poll_chat(tab, base_count, &base_text, &base_user_key, &base_reply_keys, tx)
            .await
    }

    /// Awal jalur local-tools (.NET `BeginChat` cabang `LocalTools`).
    /// Tanpa `LocalConversationStore` (milik muse-store): hanya reuse tab
    /// live; URL tersimpan antar-restart BELUM bisa dipakai.
    /// Mengembalikan request id (`MUSE_LOCAL_REQUEST_ID`) untuk `poll_local`.
    async fn begin_local(
        &self,
        req: &ChatRequest,
        tab: &mut TabState,
    ) -> Result<String, MuseError> {
        let hint = req
            .conversation_hint
            .clone()
            .unwrap_or_default();
        if hint.trim().is_empty() {
            return Err(MuseError::invalid(
                "Local tools require a stable conversation ID.",
            ));
        }
        tab.local = true;
        tab.rate_limited.store(false, Ordering::Relaxed);
        let url = tab.page.url().await.ok().flatten().unwrap_or_default();
        let live = tab.local
            && tab.hint.as_deref() == Some(hint.as_str())
            && is_thread_url(&url);
        if live {
            tab.hint = Some(hint.clone());
            tab.thread_url = url;
            if !self.on_thread(tab).await? {
                self.open_saved(tab).await?;
            }
        } else {
            self.new_thread(tab).await?;
            tab.local = true;
            tab.hint = Some(hint.clone());
        }
        let _state = self.chat_state_checked(tab).await?;
        // Kirim dengan request id (MUSE_LOCAL_REQUEST_ID) — poll_local
        // mengikat balasan ke id ini (seperti .NET submittedRequestId).
        let request_id = crate::upstream::new_uuid_v4().replace('-', "");
        // TODO(muse-store): gambar di sini tidak di-hash/dedup via
        // ImageBatchId + LocalConversationStore (lihat TRADEOFFS.md).
        self.attach(tab, &req.images).await?;
        let prompt_text = Prompt::local_continuation(&req.turns, &req.reply_language);
        self.send(
            tab,
            &format!(
                "MUSE_LOCAL_REQUEST_ID: {request_id}\n(Transport metadata for the app; do not echo it in your answer.)\n\n{prompt_text}"
            ),
        )
        .await?;
        Ok(request_id)
    }

    /// Loop poll chat biasa — port `BrowserDriver.ChatStream` (.NET).
    async fn poll_chat(
        &self,
        tab: &mut TabState,
        base_count: usize,
        base_text: &str,
        base_user_key: &str,
        base_reply_keys: &HashSet<String>,
        tx: &tokio::sync::mpsc::Sender<Result<ChatEvent, MuseError>>,
    ) -> Result<(), MuseError> {
        let settings = &self.inner.settings;
        let started = Instant::now();
        let mut emitted = String::new();
        let mut last: Option<String> = None;
        let mut stable = 0usize;
        let mut got_first = false;
        let mut saw_generating = false;
        let mut stopped_at: Option<f64> = None;
        let mut progress: Option<String> = None;
        let mut progress_at = 0.0f64;
        loop {
            let elapsed = started.elapsed().as_secs_f64();
            if elapsed > settings.chat_timeout {
                return Err(MuseError::upstream_timeout("assistant reply timed out"));
            }
            if !got_first && elapsed > settings.first_token_timeout {
                return Err(MuseError::upstream_response_unavailable(
                    "No distinct assistant reply could be read from Muse web before timeout. Conversation retained; no automatic resend.",
                ));
            }
            tokio::time::sleep(Duration::from_secs_f64(POLL_INTERVAL)).await;
            let st = match Self::chat_state(&tab.page, None).await {
                Some(s) => s,
                None => {
                    return Err(MuseError::upstream_response_unavailable(
                        "Muse web state unavailable; stopped waiting. Conversation retained.",
                    ));
                }
            };
            self.check_chat_state(tab, &st).await?;
            if st.generating {
                saw_generating = true;
                stopped_at = None;
            } else if saw_generating && stopped_at.is_none() {
                stopped_at = Some(elapsed);
            }
            let response_text = st.last_text.clone();
            let snapshot = format!("{}:{response_text}", st.generating);
            if Some(&snapshot) != progress.as_ref() {
                progress = Some(snapshot);
                progress_at = elapsed;
            }
            if got_first && elapsed - progress_at > settings.chat_idle_timeout {
                return Err(MuseError::upstream_response_unavailable(
                    "Muse web stopped making progress; no new response data. Conversation retained; no automatic resend.",
                ));
            }
            if st.approval {
                self.deny_approvals(tab).await;
                continue;
            }
            let text = response_text;
            let current_user_key = st.user_key.clone();
            let reply_key = st.reply_key.clone();
            let new_message = st.reply_after_user
                && !current_user_key.is_empty()
                && current_user_key != base_user_key
                && !reply_key.is_empty()
                && !base_reply_keys.contains(&reply_key);
            let is_new = new_message
                || st.agent_count > base_count
                || (!text.is_empty() && text != base_text);
            if !is_new || text.is_empty() {
                if let Some(idle_at) = stopped_at {
                    if elapsed - idle_at > settings.reply_render_grace {
                        return Err(MuseError::upstream_response_unavailable(
                            "Muse web stopped responding without a completed reply that the app could identify. Conversation retained; no automatic resend.",
                        ));
                    }
                }
                if elapsed > 15.0
                    && dom::STALL_HINTS.iter().any(|h| st.tail.contains(h))
                {
                    return Err(MuseError::upstream_timeout(
                        "upstream workspace is stuck connecting",
                    ));
                }
                continue;
            }
            got_first = true;
            if last.as_deref() != Some(text.as_str()) {
                // DOM re-render bisa memendekkan teks sebentar — tunggu
                // prefix yang sudah dikirim kembali (seperti .NET).
                if !text.starts_with(&emitted) {
                    continue;
                }
                let delta = text[emitted.len()..].to_string();
                emitted = text.clone();
                last = Some(text);
                stable = 0;
                if !delta.is_empty()
                    && tx
                        .clone()
                        .send(Ok(ChatEvent::AssistantDelta(delta)))
                        .await
                        .is_err()
                {
                    return Err(MuseError::internal("chat stream closed by caller"));
                }
            } else if !st.generating {
                stable += 1;
                if stable >= STABLE_POLLS_DONE {
                    return Ok(());
                }
            } else {
                stable = 0;
            }
        }
    }

    /// Loop poll jalur local-tools — port cabang `LocalTools` di
    /// `BrowserDriver.ChatStream` (.NET): tunggu balasan utuh stabil,
    /// kirim SATU delta penuh (Markdown/code fence tidak bisa direkonstruksi
    /// dari delta parsial DOM).
    async fn poll_local(
        &self,
        _req: &ChatRequest,
        tab: &mut TabState,
        request_id: &str,
        tx: &tokio::sync::mpsc::Sender<Result<ChatEvent, MuseError>>,
    ) -> Result<(), MuseError> {
        let settings = &self.inner.settings;
        let started = Instant::now();
        let mut last: Option<String> = None;
        let mut stable = 0usize;
        let mut got_first = false;
        let mut saw_generating = false;
        let mut stopped_at: Option<f64> = None;
        let mut progress: Option<String> = None;
        let mut progress_at = 0.0f64;
        loop {
            let elapsed = started.elapsed().as_secs_f64();
            if elapsed > settings.chat_timeout {
                return Err(MuseError::upstream_timeout("assistant reply timed out"));
            }
            if !got_first && elapsed > settings.first_token_timeout {
                return Err(MuseError::upstream_response_unavailable(
                    "No distinct assistant reply could be read from Muse web before timeout. Conversation retained; no automatic resend.",
                ));
            }
            tokio::time::sleep(Duration::from_secs_f64(POLL_INTERVAL)).await;
            let st = match Self::chat_state(&tab.page, Some(&request_id)).await {
                Some(s) => s,
                None => {
                    return Err(MuseError::upstream_response_unavailable(
                        "Muse web state unavailable; stopped waiting. Conversation retained.",
                    ));
                }
            };
            self.check_chat_state(tab, &st).await?;
            if st.generating {
                saw_generating = true;
                stopped_at = None;
            } else if saw_generating && stopped_at.is_none() {
                stopped_at = Some(elapsed);
            }
            let response_text = st.local_reply_text.clone();
            let snapshot = format!("{}:{response_text}", st.generating);
            if Some(&snapshot) != progress.as_ref() {
                progress = Some(snapshot);
                progress_at = elapsed;
            }
            if got_first && elapsed - progress_at > settings.chat_idle_timeout {
                return Err(MuseError::upstream_response_unavailable(
                    "Muse web stopped making progress; no new response data. Conversation retained; no automatic resend.",
                ));
            }
            if st.approval {
                self.deny_approvals(tab).await;
                continue;
            }
            let is_new = !request_id.is_empty()
                && st.local_request_id == request_id
                && st.local_reply_bound;
            let text = response_text;
            if !is_new || text.is_empty() {
                if let Some(idle_at) = stopped_at {
                    if elapsed - idle_at > settings.reply_render_grace {
                        return Err(MuseError::upstream_response_unavailable(
                            "Muse web stopped responding without a completed reply that the app could identify. Conversation retained; no automatic resend.",
                        ));
                    }
                }
                if elapsed > 15.0
                    && dom::STALL_HINTS.iter().any(|h| st.tail.contains(h))
                {
                    return Err(MuseError::upstream_timeout(
                        "upstream workspace is stuck connecting",
                    ));
                }
                continue;
            }
            got_first = true;
            if last.as_deref() != Some(text.as_str()) {
                last = Some(text);
                stable = 0;
            } else if !st.generating {
                stable += 1;
                if stable >= STABLE_POLLS_DONE {
                    let full = last.clone().unwrap_or_default();
                    if tx
                        .clone()
                        .send(Ok(ChatEvent::AssistantDelta(full)))
                        .await
                        .is_err()
                    {
                        return Err(MuseError::internal("chat stream closed by caller"));
                    }
                    return Ok(());
                }
            } else {
                stable = 0;
            }
        }
    }

    // -- media -------------------------------------------------------------

    /// Port `MediaPrompt` (.NET).
    fn media_prompt(prompt_text: &str, kind: &str, size: Option<&str>, duration: Option<u32>) -> String {
        let mut hints = Vec::new();
        if let Some(size) = size {
            if !size.is_empty() {
                hints.push(format!("aspect ratio {size}"));
            }
        }
        if let Some(d) = duration {
            hints.push(format!("{d} seconds long"));
        }
        let verb = if kind == "image" {
            "Generate an image"
        } else {
            "Generate a video"
        };
        let suffix = if hints.is_empty() {
            String::new()
        } else {
            format!(" ({})", hints.join(", "))
        };
        format!("{verb}{suffix}: {prompt_text}")
    }

    /// Port `WaitMedia` (.NET) — tunggu lampiran media segar di chat.
    async fn wait_media(
        &self,
        tab: &mut TabState,
        base_state: &ChatStateJson,
        kind: &str,
        timeout: f64,
    ) -> Result<HashMap<String, serde_json::Value>, MuseError> {
        let base_atts = base_state.attachments.len();
        let base_count = base_state.agent_count;
        let grace = if kind == "video" { 90.0 } else { 8.0 };
        let started = Instant::now();
        let mut text_done_at: Option<Instant> = None;
        let mut denied = false;
        let mut asked_to_attach = false;
        let mut cur_base_count = base_count;
        loop {
            let elapsed = started.elapsed().as_secs_f64();
            if elapsed >= timeout * 2.5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(600)).await;
            let st = self.chat_state_checked(tab).await?;
            if st.approval {
                self.deny_approvals(tab).await;
                denied = true;
                text_done_at = None;
                continue;
            }
            let atts: Vec<&AttachmentJson> = st.attachments.iter().skip(base_atts).collect();
            let fresh: Vec<&&AttachmentJson> = atts
                .iter()
                .filter(|a| !a.src.is_empty() && a.kind == kind)
                .collect();
            if let Some(last_att) = fresh.last() {
                let mut m = HashMap::new();
                m.insert("src".to_string(), serde_json::json!(last_att.src));
                m.insert("kind".to_string(), serde_json::json!(kind));
                m.insert("w".to_string(), serde_json::json!(last_att.w));
                m.insert("h".to_string(), serde_json::json!(last_att.h));
                return Ok(m);
            }
            let busy = st.generating
                || atts
                    .iter()
                    .any(|a| a.tid.contains(kind));
            let elapsed = started.elapsed().as_secs_f64();
            if elapsed >= timeout && !busy {
                break;
            }
            if atts.iter().any(|a| a.tid.contains(kind)) {
                text_done_at = None;
                continue;
            }
            let text = st.last_text.clone();
            if st.agent_count > cur_base_count && !text.is_empty() && !st.generating {
                if text_done_at.is_none() {
                    text_done_at = Some(Instant::now());
                } else if text_done_at.unwrap().elapsed().as_secs_f64() >= grace {
                    if !asked_to_attach {
                        self.send(
                            tab,
                            &format!(
                                "Please attach the final {kind} file here in the chat{}",
                                if denied {
                                    " instead of uploading it."
                                } else {
                                    ", as a single attachment."
                                }
                            ),
                        )
                        .await?;
                        asked_to_attach = true;
                        text_done_at = None;
                        cur_base_count = st.agent_count;
                        continue;
                    }
                    let probe = Self::eval(&tab.page, dom::LAST_BUBBLE_MEDIA, false).await;
                    let links: Vec<String> = probe
                        .as_ref()
                        .and_then(|v| v.get("links"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(url) = pick_media_url(&links, kind) {
                        let mut m = HashMap::new();
                        m.insert("src".to_string(), serde_json::json!(url));
                        m.insert("kind".to_string(), serde_json::json!(kind));
                        return Ok(m);
                    }
                    return Err(MuseError::upstream_refused(format!(
                        "upstream replied with text only: {}",
                        text.chars().take(200).collect::<String>()
                    )));
                }
            } else {
                text_done_at = None;
            }
        }
        Err(MuseError::upstream_timeout(format!(
            "{kind} generation timed out"
        )))
    }

    /// Port `Download` (.NET) — fetch lampiran via JS di halaman.
    async fn download(
        &self,
        tab: &TabState,
        att: &HashMap<String, serde_json::Value>,
        kind: &str,
    ) -> Result<MediaResult, MuseError> {
        let src = att
            .get("src")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let res = Self::eval(&tab.page, &dom::fetch_as_base64(&src), true).await;
        let ok = res
            .as_ref()
            .and_then(|v| v.get("ok"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ok {
            let err = res
                .as_ref()
                .and_then(|v| v.get("err"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Err(MuseError::internal(format!(
                "failed to download generated {kind}: {err}"
            )));
        }
        let mime = res
            .as_ref()
            .and_then(|v| v.get("mime"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if kind == "video" {
                    "video/mp4".to_string()
                } else {
                    "image/png".to_string()
                }
            });
        let b64 = res
            .as_ref()
            .and_then(|v| v.get("b64"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| MuseError::internal(format!("base64 decode: {e}")))?;
        let w = att
            .get("w")
            .and_then(|v| v.as_i64())
            .filter(|w| *w > 0)
            .map(|w| w as u32);
        let h = att
            .get("h")
            .and_then(|v| v.as_i64())
            .filter(|h| *h > 0)
            .map(|h| h as u32);
        Ok(MediaResult {
            data,
            mime,
            kind: kind.to_string(),
            width: w,
            height: h,
            revised_prompt: None,
        })
    }

    /// Port `Generate` (.NET) — thread baru untuk media.
    async fn generate(
        &self,
        account: &Account,
        tab_arc: Arc<Mutex<TabState>>,
        prompt_text: String,
        images: Vec<InputImage>,
        kind: &str,
        timeout: f64,
    ) -> Result<MediaResult, MuseError> {
        let mut tab = tab_arc.lock().await;
        tab.busy = true;
        let completed;
        let result = async {
            self.new_thread(&mut tab).await?;
            let base_state = self.chat_state_checked(&tab).await?;
            self.attach(&tab, &images).await?;
            self.send(&tab, &prompt_text).await?;
            let att = self
                .wait_media(&mut tab, &base_state, kind, timeout)
                .await?;
            let result = self.download(&tab, &att, kind).await?;
            Ok::<MediaResult, MuseError>(result)
        }
        .await;
        completed = result.is_ok();
        if !completed {
            tab.stale = true;
        }
        tab.busy = false;
        let stale = tab.stale;
        drop(tab);
        if stale {
            self.close_tab_locked(&account.id, tab_arc).await;
        }
        result
    }

    // -- sesi / kuota ------------------------------------------------------

    /// Port `SaveSession` (.NET): baca cookie dari page, simpan yang sesi
    /// + opsional ke akun (refresh map) + file storage-state. Gagal = `None`
    /// (best-effort, seperti .NET yang menelan exception).
    async fn save_session(
        &self,
        account: &Account,
        tab: &TabState,
    ) -> Option<SessionRefresh> {
        let cookies = match tokio::time::timeout(Duration::from_secs(10), tab.page.get_cookies())
            .await
        {
            Ok(Ok(c)) => c,
            _ => return None,
        };
        if Upstream::SESSION_COOKIES.iter().any(|name| {
            !cookies
                .iter()
                .any(|c| &c.name == name && !c.value.is_empty())
        }) {
            return None;
        }
        {
            let state = self.inner.state.lock().await;
            if state.context_auth.get(&account.id)
                != Some(&BrowserSessionStore::authentication_hash(account))
            {
                return None;
            }
        }
        let mut saved_cookies = account.cookies.clone();
        let mut saved_expires = account.cookie_expires.clone();
        for cookie in cookies.iter().filter(|c| {
            Upstream::SESSION_COOKIES.contains(&c.name.as_str())
                || Upstream::OPTIONAL_SESSION_COOKIES.contains(&c.name.as_str())
        }) {
            saved_cookies.insert(cookie.name.clone(), cookie.value.clone());
            if cookie.expires > 0.0 {
                saved_expires.insert(cookie.name.clone(), cookie.expires as i64);
            } else {
                saved_expires.remove(&cookie.name);
            }
        }
        let mut updated = account.clone();
        updated.cookies = saved_cookies.clone();
        updated.cookie_expires = saved_expires.clone();
        {
            let mut state = self.inner.state.lock().await;
            state.context_auth.insert(
                account.id.clone(),
                BrowserSessionStore::authentication_hash(&updated),
            );
        }
        // Storage-state format Playwright (cookies + origins kosong —
        // localStorage TIDAK ditangkap CDP di sini; lihat TRADEOFFS.md).
        let state_json = storage_state_json(&cookies);
        let _ = self.inner.sessions.save(&updated, &state_json);
        Some(SessionRefresh {
            account_id: account.id.clone(),
            cookies: saved_cookies,
            cookie_expires: saved_expires,
        })
    }

    /// Port `ClickJs` (.NET): eval ekspresi titik, klik mouse di sana.
    async fn click_js(&self, tab: &TabState, expr: &str) -> Result<bool, MuseError> {
        let deadline = Instant::now() + Duration::from_secs_f64(QUOTA_CLICK_TIMEOUT);
        loop {
            let pt = Self::eval(&tab.page, expr, false).await;
            if let Some(o) = pt.as_ref().and_then(|v| v.as_object()) {
                let x = o.get("x").and_then(|v| v.as_i64());
                let y = o.get("y").and_then(|v| v.as_i64());
                if let (Some(x), Some(y)) = (x, y) {
                    tab.page
                        .click(Point {
                            x: x as f64,
                            y: y as f64,
                        })
                        .await
                        .map_err(|e| MuseError::internal(format!("click: {e}")))?;
                    return Ok(true);
                }
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Port `Escape` (.NET): tekan Escape 2x.
    async fn escape(&self, tab: &TabState) {
        for _ in 0..2 {
            press_key(&tab.page, "Escape", "Escape", 27, None).await;
            tokio::time::sleep(Duration::from_secs_f64(ESCAPE_PAUSE)).await;
        }
    }

    /// Port `ReadQuota` (.NET).
    async fn read_quota(
        &self,
        tab_arc: Arc<Mutex<TabState>>,
    ) -> Result<HashMap<String, serde_json::Value>, MuseError> {
        let mut tab = tab_arc.lock().await;
        let page = Self::page_state(&tab.page).await;
        if !page.as_ref().map(|p| p.has_input).unwrap_or(false) {
            self.new_thread(&mut tab).await?;
        }
        self.escape(&tab).await;
        let result = async {
            if !self.click_js(&tab, dom::QUOTA_SETTINGS_ENTRY).await? {
                let mut m = UsageInfo::empty().to_map();
                m.insert(
                    "error".to_string(),
                    serde_json::json!("settings entry not found"),
                );
                return Ok::<HashMap<String, serde_json::Value>, MuseError>(m);
            }
            if !self.click_js(&tab, dom::QUOTA_MENU_ITEM).await? {
                let mut m = UsageInfo::empty().to_map();
                m.insert(
                    "error".to_string(),
                    serde_json::json!("settings menu item not found"),
                );
                return Ok(m);
            }
            let deadline = Instant::now() + Duration::from_secs_f64(QUOTA_READ_TIMEOUT);
            loop {
                let dialog = Self::eval(&tab.page, dom::QUOTA_DIALOG_TEXT, false).await;
                let text = dialog
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let parsed = dom::parse_usage_text(Some(&text));
                if parsed.found || Instant::now() >= deadline {
                    return Ok(parsed.to_map());
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        .await;
        // finally (.NET): Escape; bila dialog masih terbuka, tab stale.
        self.escape(&tab).await;
        let dialog = Self::eval(&tab.page, dom::QUOTA_DIALOG_TEXT, false).await;
        if dialog
            .as_ref()
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            tab.stale = true;
        }
        drop(tab);
        // Bila dialog kuota masih terbuka, tab ditandai stale di atas;
        // penutupan dilakukan pemanggil (quota).
        result
    }
}

const POLL_INTERVAL: f64 = 0.15;
const STABLE_POLLS_DONE: usize = 4;
const HOT_BUBBLE_LIMIT: usize = 16;
const QUOTA_CLICK_TIMEOUT: f64 = 8.0;
const QUOTA_READ_TIMEOUT: f64 = 12.0;
const ESCAPE_PAUSE: f64 = 0.1;

// ---------------------------------------------------------------------------
// Trait MuseDriver
// ---------------------------------------------------------------------------

#[async_trait]
impl MuseDriver for BrowserDriver {
    fn name(&self) -> &'static str {
        "browser"
    }

    fn requires_account(&self) -> bool {
        true
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities::ALL
    }

    async fn startup(&self) -> Result<(), MuseError> {
        self.ensure_started().await
    }

    async fn shutdown(&self) -> Result<(), MuseError> {
        let tabs: Vec<Arc<Mutex<TabState>>>;
        let mut state = self.inner.state.lock().await;
        tabs = state.tabs.values().cloned().collect();
        state.tabs.clear();
        state.context_auth.clear();
        state.reset_contexts.clear();
        let browser = state.browser.take();
        let handler = state.handler.take();
        let profile_dir = state.profile_dir.take();
        drop(state);
        for tab in tabs {
            let mut t = tab.lock().await;
            if let Some(h) = t.listener_task.take() {
                h.abort();
            }
            let _ = t.page.clone().close().await;
        }
        if let Some(b) = browser {
            // Tutup graceful bila Arc ini satu-satunya pemilik; bila ada
            // tugas lain yang masih memegang Arc, drop saja — Drop Browser
            // membunuh child Chromium (kill_on_drop), jadi tidak zombie.
            if let Ok(mut b) = Arc::try_unwrap(b) {
                let _ = b.close().await;
            }
        }
        if let Some(h) = handler {
            h.abort();
        }
        if let Some(dir) = profile_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.inner.quota_cache.lock().await.clear();
        Ok(())
    }

    async fn invalidate_account(&self, account_id: &str) -> Result<(), MuseError> {
        let tab = {
            let mut state = self.inner.state.lock().await;
            state.reset_contexts.insert(account_id.to_string());
            state.tabs.get(account_id).cloned()
        };
        if let Some(tab) = tab {
            {
                let mut t = tab.lock().await;
                t.stale = true;
            }
            // Tutup langsung bila tidak sibuk; bila sibuk, run_chat akan
            // menutupnya di finally (stale).
            let idle = tab.try_lock().is_ok();
            if idle {
                self.close_tab_locked(account_id, tab).await;
            }
        } else {
            let mut state = self.inner.state.lock().await;
            state.tabs.remove(account_id);
            state.context_auth.remove(account_id);
            state.reset_contexts.remove(account_id);
        }
        self.inner.quota_cache.lock().await.remove(account_id);
        Ok(())
    }

    fn chat_stream(&self, account: Account, req: ChatRequest) -> ChatStream {
        let this = self.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatEvent, MuseError>>(32);
        let handle = tokio::spawn(async move {
            match this.run_chat(account, req, tx.clone()).await {
                Ok(()) => {
                    let _ = tx.send(Ok(ChatEvent::Done)).await;
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                }
            }
        });
        // Receiver tokio bukan Stream — bungkus dengan unfold.
        let stream = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Box::pin(AbortableStream {
            inner: Box::pin(stream),
            handle: Some(handle),
        })
    }

    async fn generate_image(
        &self,
        account: Account,
        req: ImageRequest,
    ) -> Result<Vec<MediaResult>, MuseError> {
        let prompt_text = Self::media_prompt(&req.prompt, "image", req.size.as_deref(), None);
        let mut results = Vec::new();
        for _ in 0..req.n.max(1) {
            let lock = self.account_lock(&account.id).await;
            let _guard = lock.lock().await;
            let tab_arc = self.get_or_create_tab(&account).await?;
            let r = self
                .generate(
                    &account,
                    tab_arc,
                    prompt_text.clone(),
                    req.references.clone(),
                    "image",
                    self.inner.settings.image_timeout,
                )
                .await?;
            let mut r = r;
            r.revised_prompt = Some(req.prompt.clone());
            results.push(r);
        }
        Ok(results)
    }

    async fn generate_video(
        &self,
        account: Account,
        req: VideoRequest,
    ) -> Result<MediaResult, MuseError> {
        let prompt_text = Self::media_prompt(
            &req.prompt,
            "video",
            req.size.as_deref(),
            req.duration,
        );
        let images = req
            .first_frame
            .map(|f| vec![f])
            .unwrap_or_default();
        let lock = self.account_lock(&account.id).await;
        let _guard = lock.lock().await;
        let tab_arc = self.get_or_create_tab(&account).await?;
        self.generate(
            &account,
            tab_arc,
            prompt_text,
            images,
            "video",
            self.inner.settings.video_timeout,
        )
        .await
    }

    async fn renew_session(&self, account: Account) -> Result<SessionInfo, MuseError> {
        // .NET BrowserDriver.RenewSession: Upstream.RenewSession(cookies,
        // WakeConfirmSeconds) lalu InvalidateAccount.
        let info = Upstream::renew_session(
            &account.cookies,
            self.inner.settings.wake_confirm_seconds,
            2.0,
        )
        .await?;
        self.invalidate_account(&account.id).await?;
        Ok(info)
    }

    async fn quota(
        &self,
        account: Account,
    ) -> Result<HashMap<String, serde_json::Value>, MuseError> {
        {
            let cache = self.inner.quota_cache.lock().await;
            if let Some((at, hit)) = cache.get(&account.id) {
                if at.elapsed().as_secs_f64() < self.inner.settings.quota_cache_seconds {
                    return Ok(hit.clone());
                }
            }
        }
        let lock = self.account_lock(&account.id).await;
        let _guard = lock.lock().await;
        {
            let cache = self.inner.quota_cache.lock().await;
            if let Some((at, hit)) = cache.get(&account.id) {
                if at.elapsed().as_secs_f64() < self.inner.settings.quota_cache_seconds {
                    return Ok(hit.clone());
                }
            }
        }
        let tab_arc = self.get_or_create_tab(&account).await?;
        let parsed = self.read_quota(tab_arc.clone()).await?;
        let mut snapshot = parsed;
        snapshot.insert("source".to_string(), serde_json::json!("browser"));
        snapshot.insert(
            "fetched_at".to_string(),
            serde_json::json!(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0)
            ),
        );
        // Bila dialog kuota masih terbuka, read_quota menandai tab stale —
        // tutup tab agar chat berikutnya mulai bersih.
        let stale = tab_arc.lock().await.stale;
        if stale {
            self.close_tab_locked(&account.id, tab_arc).await;
        }
        self.inner
            .quota_cache
            .lock()
            .await
            .insert(account.id.clone(), (Instant::now(), snapshot.clone()));
        Ok(snapshot)
    }

    async fn health(&self) -> Result<serde_json::Value, MuseError> {
        let state = self.inner.state.lock().await;
        let tabs: Vec<&Arc<Mutex<TabState>>> = state.tabs.values().collect();
        let total = tabs.len();
        // busy tidak bisa dibaca tanpa lock; hitung dari tab yang terkunci
        // saat ini (try_lock gagal = sibuk).
        let mut busy = 0;
        for t in tabs {
            if t.try_lock().is_err() {
                busy += 1;
            }
        }
        Ok(serde_json::json!({
            "driver": "browser",
            "ok": state.browser.is_some(),
            "tabs": total,
            "busy_tabs": busy,
        }))
    }

    fn refreshed_session(&self, account_id: &str) -> Option<SessionRefresh> {
        // Sync read dari Mutex tokio tidak bisa; pakai try_lock (best-effort).
        self.inner
            .refreshed
            .try_lock()
            .ok()?
            .get(account_id)
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// Stream chat yang bisa di-abort (drop receiver → abort task chat)
// ---------------------------------------------------------------------------

struct AbortableStream {
    inner: std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<ChatEvent, MuseError>> + Send>>,
    handle: Option<JoinHandle<()>>,
}

impl futures_util::Stream for AbortableStream {
    type Item = Result<ChatEvent, MuseError>;

    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for AbortableStream {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Helper bebas
// ---------------------------------------------------------------------------

/// `true` bila proses ini jalan sebagai root (uid 0) — dibaca dari
/// `/proc/self/status`, tanpa dep `libc`.
fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<u32>()
                .ok()
        })
        .map(|uid| uid == 0)
        .unwrap_or(false)
}

/// Port `BrowserTab` helpers (.NET): lupakan state hot tab.
fn forget(tab: &mut TabState) {
    tab.hint = None;
    tab.turns = Vec::new();
    tab.image_count = 0;
    tab.thread_url = String::new();
    tab.local = false;
    tab.rate_limited.store(false, Ordering::Relaxed);
}

/// Port `SaveHot` (.NET) — hanya di memori tab (lihat header file).
/// `hot_page` di `account.Meta` TIDAK ditulis: akun dimiliki gateway.
fn save_hot(tab: &mut TabState, req: &ChatRequest) {
    tab.hint = req.conversation_hint.clone();
    tab.turns = req.turns.clone();
    tab.image_count = req.images.len();
}

/// Port `Upstream.IsThreadUrl` via `BrowserDriver.IsThreadUrl` (.NET).
fn is_thread_url(url: &str) -> bool {
    let path = url_path(url).trim_end_matches('/');
    path.starts_with("/thread/") && path != "/thread/new"
}

/// Path URL (tanpa query/fragment), seperti `Uri.AbsolutePath` .NET.
fn url_path(url: &str) -> &str {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let path = match after_scheme.find('/') {
        Some(i) => &after_scheme[i..],
        None => "/",
    };
    let end = path
        .find(|c| c == '?' || c == '#')
        .unwrap_or(path.len());
    &path[..end]
}

/// Iris string per karakter (bukan byte) — aman untuk UTF-8.
/// `.NET` memakai indeks UTF-16; hasil bisa bergeser ±1 karakter untuk
/// teks non-BMP, tapi tidak pernah panic.
fn char_slice(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Cookie akun → `CookieParam` CDP (port `BrowserDriver.SetCookies`).
fn account_cookies(account: &Account) -> Vec<CookieParam> {
    let mut out = Vec::new();
    for (name, value) in &account.cookies {
        if value.is_empty() {
            continue;
        }
        let mut b = CookieParam::builder()
            .name(name.clone())
            .value(value.clone())
            .domain(".muse.ai")
            .path("/")
            .secure(true);
        if let Some(exp) = account.cookie_expires.get(name) {
            if *exp > 0 {
                b = b.expires(TimeSinceEpoch::new(*exp as f64));
            }
        }
        if let Ok(param) = b.build() {
            out.push(param);
        }
    }
    out
}

/// Cookie dari file storage-state Playwright → `CookieParam` CDP.
/// Format Playwright: `{name,value,domain,path,expires,httpOnly,secure,sameSite}`.
fn storage_state_cookies(state_json: &str) -> Vec<CookieParam> {
    let parsed: serde_json::Value = match serde_json::from_str(state_json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let Some(arr) = parsed.get("cookies").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    for c in arr {
        let Some(name) = c.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(value) = c.get("value").and_then(|v| v.as_str()) else {
            continue;
        };
        let mut b = CookieParam::builder()
            .name(name)
            .value(value)
            .domain(
                c.get("domain")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".muse.ai"),
            )
            .path(c.get("path").and_then(|v| v.as_str()).unwrap_or("/"))
            .secure(c.get("secure").and_then(|v| v.as_bool()).unwrap_or(true))
            .http_only(
                c.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false),
            );
        if let Some(exp) = c.get("expires").and_then(|v| v.as_f64()) {
            if exp > 0.0 {
                b = b.expires(TimeSinceEpoch::new(exp));
            }
        }
        if let Some(same) = c.get("sameSite").and_then(|v| v.as_str()) {
            let ss = match same {
                "Strict" => Some(CookieSameSite::Strict),
                "Lax" => Some(CookieSameSite::Lax),
                "None" => Some(CookieSameSite::None),
                _ => None,
            };
            if let Some(ss) = ss {
                b = b.same_site(ss);
            }
        }
        if let Ok(param) = b.build() {
            out.push(param);
        }
    }
    out
}

/// Cookie CDP → JSON storage-state Playwright
/// (`{cookies:[...], origins:[]}`). `origins` selalu kosong: localStorage
/// tidak ditangkap (lihat TRADEOFFS.md).
fn storage_state_json(cookies: &[Cookie]) -> String {
    let arr: Vec<serde_json::Value> = cookies
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "value": c.value,
                "domain": c.domain,
                "path": c.path,
                "expires": if c.expires > 0.0 { c.expires } else { -1.0 },
                "httpOnly": c.http_only,
                "secure": c.secure,
                "sameSite": c.same_site.as_ref().map(|s| s.as_ref()).unwrap_or("Lax"),
            })
        })
        .collect();
    serde_json::json!({ "cookies": arr, "origins": [] }).to_string()
}

/// Port `PickMediaUrl` (.NET) — skor link kandidat media.
fn pick_media_url(links: &[String], kind: &str) -> Option<String> {
    let video_ext = [".mp4", ".webm", ".mov", ".m4v"];
    let image_ext = [".png", ".jpg", ".jpeg", ".webp", ".gif", ".avif"];
    let media_hosts = [
        "videodelivery.net",
        "cloudflarestream.com",
        "cloudflarestorage.com",
        "amazonaws.com",
        "storage.googleapis.com",
        "blob.core.windows.net",
        "musecdn",
        "cdn.muse",
        "muse-cdn",
    ];
    let media_paths = ["/media/", "/download", "/dl/", "/video", "/attachment", "/files/"];
    let score = |u: &str| -> i32 {
        let low = u.to_lowercase();
        let path = low.split('?').next().unwrap_or("").split('#').next().unwrap_or("");
        let exts = if kind == "video" { video_ext.as_slice() } else { image_ext.as_slice() };
        if u.starts_with("blob:") || exts.iter().any(|e| path.ends_with(e)) {
            return 3;
        }
        if media_hosts.iter().any(|h| low.contains(h)) {
            return 2;
        }
        if media_paths.iter().any(|p| low.contains(p)) {
            return 1;
        }
        0
    };
    let mut cands: Vec<&String> = links
        .iter()
        .filter(|u| {
            !u.is_empty()
                && !u.starts_with("mailto:")
                && !u.starts_with("javascript:")
        })
        .collect();
    cands.sort_by(|a, b| score(b).cmp(&score(a)));
    cands
        .first()
        .filter(|c| score(c) > 0)
        .map(|c| (*c).clone())
}

/// Tekan tombol via CDP `Input.dispatchKeyEvent` (keyDown + keyUp).
/// Port `IPage.Keyboard.PressAsync` (.NET) — Playwright menekan tombol
/// fisik; di CDP ini padanannya.
async fn press_key(page: &Page, key: &str, code: &str, vk: i64, text: Option<&str>) {
    for (ty, with_text) in [
        (DispatchKeyEventType::KeyDown, true),
        (DispatchKeyEventType::KeyUp, false),
    ] {
        let mut b = DispatchKeyEventParams::builder()
            .r#type(ty)
            .key(key)
            .code(code)
            .windows_virtual_key_code(vk);
        if with_text {
            if let Some(t) = text {
                b = b.text(t);
            }
        }
        if let Ok(params) = b.build() {
            let _ = page.execute(params).await;
        }
    }
}

/// Listener 429: port `page.Response` di `OpenTab` (.NET) — bila ada
/// respons 429 dari `muse.ai/api/*`, tandai tab rate-limited.
async fn spawn_rate_limit_listener(
    page: Page,
    flag: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    let mut events = match page.event_listener::<EventResponseReceived>().await {
        Ok(e) => e,
        Err(_) => return None,
    };
    Some(tokio::spawn(async move {
        while let Some(ev) = events.next().await {
            if ev.response.status == 429 {
                let url = ev.response.url.to_lowercase();
                if url.contains("muse.ai") && url_path(&url).starts_with("/api/") {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_thread_url_matches_dotnet() {
        assert!(is_thread_url("https://muse.ai/thread/abc123"));
        assert!(is_thread_url("https://muse.ai/thread/abc123?x=1"));
        assert!(!is_thread_url("https://muse.ai/thread/new"));
        assert!(!is_thread_url("https://muse.ai/thread/new/"));
        assert!(!is_thread_url("https://muse.ai/"));
        assert!(!is_thread_url("https://muse.ai/thread/"));
    }

    #[test]
    fn url_path_strips_query_fragment() {
        assert_eq!(url_path("https://muse.ai/thread/abc?x=1#y"), "/thread/abc");
        assert_eq!(url_path("https://muse.ai/"), "/");
        assert_eq!(url_path("about:blank"), "/");
    }

    #[test]
    fn pick_media_url_scores_extension_first() {
        let links = vec![
            "https://example.com/page".to_string(),
            "https://cdn.example.com/v.mp4".to_string(),
            "https://videodelivery.net/abc".to_string(),
        ];
        assert_eq!(
            pick_media_url(&links, "video").as_deref(),
            Some("https://cdn.example.com/v.mp4")
        );
        // Untuk image: .mp4 tidak cocok ekstensi, tapi videodelivery.net
        // skor 2 (media host) — .NET mengembalikan kandidat skor > 0.
        assert_eq!(
            pick_media_url(&links, "image").as_deref(),
            Some("https://videodelivery.net/abc")
        );
        // Tidak ada kandidat berskor -> None.
        let plain = vec!["https://example.com/page".to_string()];
        assert_eq!(pick_media_url(&plain, "video"), None);
    }

    #[test]
    fn media_prompt_matches_dotnet() {
        assert_eq!(
            BrowserDriver::media_prompt("kucing", "image", Some("16:9"), None),
            "Generate an image (aspect ratio 16:9): kucing"
        );
        assert_eq!(
            BrowserDriver::media_prompt("laut", "video", Some("9:16"), Some(10)),
            "Generate a video (aspect ratio 9:16, 10 seconds long): laut"
        );
        assert_eq!(
            BrowserDriver::media_prompt("x", "image", None, None),
            "Generate an image: x"
        );
    }

    #[test]
    fn char_slice_is_char_based_and_safe() {
        assert_eq!(char_slice("halo dunia", 0, 4), "halo");
        assert_eq!(char_slice("🦀🦀🦀", 0, 2), "🦀🦀");
        assert_eq!(char_slice("abc", 5, 9), "");
    }

    #[test]
    fn storage_state_cookies_roundtrip_shape() {
        let json = r#"{"cookies":[{"name":"hatch_sess","value":"v","domain":".muse.ai","path":"/","expires":1893456000,"httpOnly":true,"secure":true,"sameSite":"Lax"}],"origins":[]}"#;
        let params = storage_state_cookies(json);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "hatch_sess");
        assert_eq!(params[0].domain.as_deref(), Some(".muse.ai"));
    }

    #[test]
    fn storage_state_json_shape() {
        let cookies = vec![Cookie {
            name: "hatch_sess".to_string(),
            value: "v".to_string(),
            domain: ".muse.ai".to_string(),
            path: "/".to_string(),
            expires: 1893456000.0,
            size: 10,
            http_only: true,
            secure: true,
            session: false,
            same_site: Some(CookieSameSite::Lax),
            priority: chromiumoxide::cdp::browser_protocol::network::CookiePriority::Medium,
            source_scheme: chromiumoxide::cdp::browser_protocol::network::CookieSourceScheme::Secure,
            source_port: -1,
            partition_key: None,
            partition_key_opaque: None,
        }];
        let json = storage_state_json(&cookies);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["cookies"][0]["name"], "hatch_sess");
        assert_eq!(v["cookies"][0]["sameSite"], "Lax");
        assert_eq!(v["origins"], serde_json::json!([]));
    }
}
