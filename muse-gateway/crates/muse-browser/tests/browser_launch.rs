//! Integration test: buktikan CDP/chromiumoxide benar-benar jalan di VM ini.
//!
//! - Launch Chromium headless (executable dari `MUSE2API_CHROMIUM_PATH`
//!   atau `/opt/meta-chromium/chrome`), buka halaman `data:` yang meniru
//!   DOM muse.ai, jalankan skrip `Dom::PAGE_STATE` / `Dom::CHAT_STATE` /
//!   `FILL_INPUT` / `CLICK_SEND` dan baca hasilnya.
//! - Buka https://muse.ai (tanpa login) dan buktikan navigasi + eval jalan;
//!   tanpa cookie sesi, halaman login yang muncul adalah hasil yang
//!   diharapkan — chat asli tetap butuh akun login (tidak ada di VM).
//! - Lifecycle `BrowserDriver` (startup/health/shutdown) + error auth
//!   untuk akun tanpa cookie (tanpa launch browser).
//!
//! Bila Chromium tidak ditemukan di VM, test SKIP dengan jelas (bukan
//! pura-pura lolos): mereka mencetak `SKIP: ...` dan return Ok.

use std::path::PathBuf;

use chromiumoxide::Browser;
use chromiumoxide::browser::HeadlessMode;
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use chromiumoxide::cdp::browser_protocol::storage::SetCookiesParams as StorageSetCookiesParams;
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams, DisposeBrowserContextParams,
};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use muse_browser::dom;
use muse_browser::{BrowserDriver, DriverSettings, MuseDriver};
use muse_browser::types::{Account, ChatRequest};

fn find_chromium() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MUSE2API_CHROMIUM_PATH") {
        if !p.is_empty() && PathBuf::from(&p).exists() {
            return Some(PathBuf::from(p));
        }
    }
    for p in [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/opt/meta-chromium/chrome",
    ] {
        if PathBuf::from(p).exists() {
            return Some(PathBuf::from(p));
        }
    }
    None
}

async fn launch() -> Option<(Browser, tokio::task::JoinHandle<()>)> {
    let exe = match find_chromium() {
        Some(e) => e,
        None => {
            println!("SKIP: no Chromium executable found in this VM (checked MUSE2API_CHROMIUM_PATH, /usr/bin/google-chrome*, /usr/bin/chromium*, /opt/meta-chromium/chrome)");
            return None;
        }
    };
    // Egress VM ini lewat proxy (env HTTPS_PROXY/HTTP_PROXY); Chrome
    // tidak membaca env proxy otomatis, jadi teruskan sebagai
    // --proxy-server. Tanpa ini, navigasi internet gagal dengan
    // ERR_TUNNEL_CONNECTION_FAILED (curl berhasil karena memakai env).
    // Chrome baru MENOLAK kredensial di URL --proxy-server
    // (ERR_NO_SUPPORTED_PROXIES), jadi userinfo dibuang — proxy VM ini
    // menerima koneksi tanpa auth dari IP VM (terbukti curl 200).
    // Nilai mentah (ber-token) TIDAK pernah dicetak/disimpen.
    let mut builder = chromiumoxide::BrowserConfig::builder()
        .chrome_executable(exe.clone())
        .headless_mode(HeadlessMode::New)
        .no_sandbox(); // test jalan sebagai root di VM
    if let Some(proxy) = proxy_for_chrome() {
        builder = builder.arg(format!("--proxy-server={proxy}"));
    }
    let config = match builder
        .user_data_dir(std::env::temp_dir().join(format!(
            "muse-browser-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-background-networking")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: invalid browser config for {}: {e}", exe.display());
            return None;
        }
    };
    let (browser, mut handler) = match Browser::launch(config).await {
        Ok(b) => b,
        Err(e) => {
            // Launch gagal (mis. /opt/meta-chromium/chrome menolak start
            // tanpa WebGL policy dari browser-daemon) — laporkan, jangan
            // pura-pura skip karena "tidak ada Chromium".
            println!(
                "SKIP: Chromium launch failed for {}: {e}",
                exe.display()
            );
            return None;
        }
    };
    let task = tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(_) = handler.next().await {}
    });
    Some((browser, task))
}

/// Proxy untuk Chrome: dari env, dengan userinfo dibuang.
/// Chrome baru menolak `user:pass@` di `--proxy-server`.
fn proxy_for_chrome() -> Option<String> {
    let raw = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .ok()?;
    if raw.is_empty() {
        return None;
    }
    let scheme = raw.split("://").next().unwrap_or("http");
    let after_scheme = raw.split("://").nth(1).unwrap_or(raw.as_str());
    let hostport = after_scheme.rsplit('@').next().unwrap_or(after_scheme);
    Some(format!("{scheme}://{hostport}"))
}

async fn eval_json(
    page: &chromiumoxide::Page,
    script: &str,
) -> Option<serde_json::Value> {
    let params = EvaluateParams::builder()
        .expression(script)
        .return_by_value(true)
        .await_promise(false)
        .build()
        .ok()?;
    page.evaluate_expression(params)
        .await
        .ok()?
        .value()
        .cloned()
}

/// Halaman tiruan DOM muse.ai (cukup untuk skrip Dom.cs).
const MOCK_PAGE: &str = r#"<!doctype html><html><head><title>Muse mock</title></head><body>
<div class="group/msg ms-auto items-end"><div>halo</div></div>
<div class="hatch-agent-bubble-bg">Halo, ini balasan mock.</div>
<textarea></textarea>
<button aria-label="Send message">Send</button>
</body></html>"#;

#[tokio::test]
async fn launch_navigate_and_read_dom_on_mock_page() {
    let Some((browser, handler)) = launch().await else {
        return; // alasan sudah dicetak launch()
    };
    let exe = find_chromium().unwrap();
    println!("Chromium under test: {}", exe.display());
    let page = browser
        .new_page(format!(
            "data:text/html;base64,{}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                MOCK_PAGE.as_bytes()
            )
        ))
        .await
        .expect("new_page");
    // PageState: ready=true (textarea ada), hasInput=true.
    let state = eval_json(&page, dom::PAGE_STATE)
        .await
        .expect("PageState eval");
    println!("PageState on mock page: {state}");
    assert_eq!(state.get("ready").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(state.get("hasInput").and_then(|v| v.as_bool()), Some(true));
    // ChatState: 1 bubble, lastText terbaca.
    let chat = eval_json(&page, &dom::chat_state())
        .await
        .expect("ChatState eval");
    println!("ChatState on mock page: {chat}");
    assert_eq!(
        chat.get("agentCount").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        chat.get("lastText").and_then(|v| v.as_str()),
        Some("Halo, ini balasan mock.")
    );
    // FillInput + ClickSend.
    let filled = eval_json(&page, &dom::fill_input("tes kirim"))
        .await
        .expect("FillInput eval");
    assert_eq!(
        filled.get("ok").and_then(|v| v.as_bool()),
        Some(true)
    );
    let clicked = eval_json(&page, dom::CLICK_SEND).await.expect("ClickSend eval");
    assert_eq!(clicked.as_str(), Some("clicked"));
    let empty = eval_json(&page, dom::INPUT_EMPTY)
        .await
        .expect("InputEmpty eval");
    assert_eq!(empty.as_bool(), Some(false), "input terisi setelah fill");
    let mut b = browser;
    let _ = b.close().await;
    handler.abort();
    println!("OK: launch + navigasi data-URL + eval DOM (PageState/ChatState/FillInput/ClickSend) jalan di VM");
}

#[tokio::test]
async fn navigate_real_muse_ai_without_login() {
    let Some((browser, handler)) = launch().await else {
        return; // alasan sudah dicetak launch()
    };
    let page = browser
        .new_page("https://muse.ai/thread/new")
        .await
        .expect("new_page muse.ai");
    // Tunggu sebentar agar halaman sempat load (tanpa Playwright auto-wait).
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let state = eval_json(&page, dom::PAGE_STATE).await;
    println!("PageState on https://muse.ai/thread/new (no login): {state:?}");
    match state {
        Some(s) => {
            let head = s.get("head").and_then(|v| v.as_str()).unwrap_or("");
            println!("head[0..800]: {}", &head[..head.len().min(400)]);
            // Halaman error Chrome (tunnel/proxy mati) BUKAN bukti navigasi
            // — laporkan sebagai skip, jangan klaim berhasil.
            if head.contains("can’t be reached") || head.contains("ERR_") {
                println!("SKIP: muse.ai tidak termuat di Chrome (halaman error: {head}) — cek proxy/MUSE2API_BROWSER_PROXY; batas dicatat di TRADEOFFS.md");
                let mut b = browser;
                let _ = b.close().await;
                handler.abort();
                return;
            }
            // Tanpa cookie sesi, yang diharapkan: halaman login.
            let login = dom::LOGIN_HINTS.iter().any(|h| head.to_lowercase().contains(h));
            let ready = s.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
            assert!(
                login,
                "tanpa login, halaman muse.ai seharusnya menampilkan login; head={head}"
            );
            assert!(!ready, "tanpa login, halaman muse.ai seharusnya ready=false");
            println!("OK: navigasi muse.ai termuat; halaman login terlihat, ready=false");
        }
        None => {
            println!("SKIP: eval PageState di muse.ai gagal (halaman belum load / jaringan) — bukan bukti chat; batas dicatat di TRADEOFFS.md");
        }
    }
    let mut b = browser;
    let _ = b.close().await;
    handler.abort();
}

/// Server HTTP lokal minimal (127.0.0.1, port acak) untuk membuktikan
/// cookie level-context sampai ke page — tanpa internet, tanpa akun,
/// tanpa kirim pesan apa pun.
async fn start_local_server() -> (tokio::task::JoinHandle<()>, u16) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local test server");
    let port = listener.local_addr().expect("local addr").port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = "<!doctype html><html><head><title>local</title></head><body>ok</body></html>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    (task, port)
}

/// Regresi bug Bree 2 Nov 2026 ("set cookies: Blank page can not have
/// cookie"): `get_or_create_tab` dulu memanggil `page.set_cookies` pada
/// page `about:blank`, dan chromiumoxide menolaknya di
/// `validate_cookie_url`. Perbaikan: cookie dipasang di LEVEL CONTEXT via
/// CDP `Storage.setCookies` + `browserContextId` SEBELUM page dibuat —
/// persis `context.AddCookiesAsync` di `GetContext` .NET.
///
/// Test ini membuktikan urutan itu di Chromium sungguhan:
/// 1. `Storage.setCookies` level-context berhasil sebelum page ada;
/// 2. jalur lama (`page.set_cookies` di `about:blank`) tetap gagal
///    dengan pesan "Blank page can not have cookie" (penjaga regresi);
/// 3. setelah page dinavigasikan ke server lokal, `document.cookie`
///    berisi cookie level-context tersebut.
#[tokio::test]
async fn context_level_cookies_reach_page_without_blank_page_error() {
    let Some((browser, handler)) = launch().await else {
        return; // alasan sudah dicetak launch()
    };
    let (server, port) = start_local_server().await;
    let ctx = browser
        .execute(CreateBrowserContextParams::default())
        .await
        .expect("create browser context")
        .result
        .browser_context_id;
    // Cookie untuk host server tes (bukan .muse.ai — server tes lokal).
    let cookie = CookieParam::builder()
        .name("muse-test-cookie")
        .value("ctx-cookie-ok")
        .domain("127.0.0.1")
        .path("/")
        .secure(false)
        .http_only(false)
        .build()
        .expect("cookie param");
    // (1) Set di level context SEBELUM page dibuat — harus sukses.
    let params = StorageSetCookiesParams::builder()
        .cookies(vec![cookie.clone()])
        .browser_context_id(ctx.clone())
        .build()
        .expect("storage set-cookies params");
    browser
        .execute(params)
        .await
        .expect("Storage.setCookies with browserContextId");
    println!("OK: Storage.setCookies di level context berhasil (sebelum page dibuat)");
    // Page about:blank, seperti yang dibuat get_or_create_tab.
    let page = browser
        .new_page(
            CreateTargetParams::builder()
                .browser_context_id(ctx.clone())
                .url("about:blank")
                .build()
                .expect("create target params"),
        )
        .await
        .expect("new page");
    // (2) Penjaga regresi: jalur lama HARUS gagal dengan pesan ini.
    let err = page
        .set_cookies(vec![cookie])
        .await
        .expect_err("page.set_cookies di about:blank seharusnya gagal");
    let msg = err.to_string();
    println!("page.set_cookies di about:blank gagal seperti diharapkan: {msg}");
    assert!(
        msg.contains("Blank page can not have cookie"),
        "pesan error berubah? got: {msg}"
    );
    // (3) Navigasi ke server lokal; cookie level-context harus terbaca.
    page.goto(format!("http://127.0.0.1:{port}/"))
        .await
        .expect("goto local server");
    let cookies = eval_json(&page, "document.cookie")
        .await
        .expect("document.cookie eval");
    let cookies = cookies.as_str().unwrap_or("");
    println!("document.cookie setelah navigasi: {cookies}");
    assert!(
        cookies.contains("muse-test-cookie=ctx-cookie-ok"),
        "cookie level-context tidak sampai ke page: {cookies}"
    );
    let _ = browser
        .execute(DisposeBrowserContextParams::new(ctx))
        .await;
    let mut b = browser;
    let _ = b.close().await;
    handler.abort();
    server.abort();
    println!("OK: cookie dipasang di level context sebelum page dibuat, terbaca page setelah navigasi — bug 'Blank page can not have cookie' teratasi");
}

#[tokio::test]
async fn browser_driver_lifecycle_and_missing_cookies() {
    // Akun tanpa cookie sesi -> Auth error, TANPA launch browser.
    let driver = BrowserDriver::new(DriverSettings {
        chromium_path: find_chromium()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        ..DriverSettings::default()
    });
    let account = Account::new("acc_no_cookies");
    let mut stream = driver.chat_stream(account, ChatRequest::new("hi", "muse-chat"));
    use futures_util::StreamExt;
    let first = stream.next().await.expect("stream item");
    let err = first.expect_err("harus error auth");
    assert_eq!(err.code, "upstream_auth_failed");
    assert!(err.message.contains("missing session cookies"));
    println!("OK: akun tanpa cookie -> upstream_auth_failed tanpa launch browser");

    // Lifecycle penuh (hanya bila Chromium ada).
    if find_chromium().is_none() {
        println!("SKIP: lifecycle startup/shutdown — no Chromium in VM");
        return;
    }
    driver.startup().await.expect("startup");
    let health = driver.health().await.expect("health");
    println!("health after startup: {health}");
    assert_eq!(health.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(health.get("driver").and_then(|v| v.as_str()), Some("browser"));
    driver.shutdown().await.expect("shutdown");
    let health = driver.health().await.expect("health after shutdown");
    assert_eq!(health.get("ok").and_then(|v| v.as_bool()), Some(false));
    println!("OK: startup -> health ok=true -> shutdown -> ok=false");
}
