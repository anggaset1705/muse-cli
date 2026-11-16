# TRADEOFFS — muse-browser vs Playwright .NET

Perbandingan jujur crate ini dengan `BrowserDriver.cs` (.NET, Playwright).
Referensi baris .NET dari `~/workspace/muse-cli-en-id/src/Muse2Api/`.

## Yang berhasil di-port (perilaku sama / setara)

- **Mock driver**: balasan persis `Drivers.cs:57` (+ suffix gambar dan
  chunk 8 karakter, `Drivers.cs:60-68`); `GenerateImage` (SolidPng),
  `GenerateVideo` (placeholder mp4), `RenewSession`, `Quota` — semua sama.
  Satu-satunya selisih: pemotongan "200 karakter terakhir" di .NET memakai
  UTF-16 code unit, di Rust memakai `char` (Unicode scalar). Untuk teks
  ASCII/BMP identik; emoji di luar BMP bisa menggeser batas ±beberapa
  karakter.
- **Http driver**: chat 501 dengan pesan persis .NET
  (`Drivers.cs:209-211`); renew via `Upstream.RenewSession`.
- **Factory**: `mock` | `browser` | `http`; nama lain →
  `unknown driver '<nama>'` (persis `Drivers.cs:222`).
- **Dom.cs**: semua selektor + skrip JS dibawa verbatim (`PageState`,
  `ChatState`/`ReadChatState`, `DenyApprovals`, `LastBubbleMedia`,
  `FillInput`, `ClickSend`, `InputEmpty`, `AttachFile`, `FetchAsBase64`,
  `QuotaSettingsEntry/MenuItem/DialogText`) dan `ParseUsageText` (EN/zh)
  1:1. Test unit membuktikan regex parsing; test integrasi membuktikan
  skrip jalan di Chromium asli.
- **Upstream.cs**: konstanta, `CookieHeader`, `MissingSessionCookies`,
  `RenewSession` (GET `/api/session`, merge `Set-Cookie`, POST
  `/api/hatch/vm/wake`, poll sampai `RUNNING`, `ok = status=="assigned"`).
  `connect_attempt_id` memakai UUID v4 acak (bukan `Guid.NewGuid()` —
  format sama `8-4-4-4-12`).
- **BrowserSessionStore.cs**: nama file SHA-256 hex ID akun, hash
  autentikasi (SHA-256 `hatch_sess=…\nhatch_native_auth_device=…`, hex
  upper), validasi bentuk storage-state, tulis atomik. Di Linux,
  file plaintext kompatibel dengan .NET.
- **Alur chat non-local** (`BrowserDriver.cs` `BeginChat`/`ChatStream`):
  thread baru, follow-up via `Prompt.FollowupText`, `OnThread` /
  `LiveContinuation` / `OpenSaved`, attach gambar, kirim (isi input →
  klik kirim ≤5 dtk → Enter), poll 150 ms, delta berbasis prefix,
  stabil 4 poll, timeout `ChatTimeout` / `FirstTokenTimeout` /
  `ChatIdleTimeout` / `ReplyRenderGrace`, deteksi kuota (hint di tail +
  `rateLimited` DOM), deny approval, `SaveHot` in-memory.
- **Media**: `GenerateImage`/`GenerateVideo` (thread baru, `MediaPrompt`,
  `WaitMedia` + grace 8/90 dtk, fallback "please attach…",
  `PickMediaUrl` scoring, `Download` via `FetchAsBase64`).
- **Kuota**: `ReadQuota` (klik entry Settings → menu → baca dialog →
  `ParseUsageText`), cache `QuotaCacheSeconds`, `source=browser`.
- **Renew**: `Upstream.RenewSession` + `InvalidateAccount`.
- **Deteksi 429**: selain DOM (`ChatState.rateLimited`), dipasang listener
  CDP `Network.responseReceived` — padanan `page.Response` .NET
  (`BrowserDriver.cs` ~205-210, di `OpenTab`).

## Yang TIDAK sama / belum bisa

1. **Playwright → CDP mentah.** Playwright tidak punya SDK Rust
   (kontrak §12). Konsekuensi:
   - Tidak ada `BrowserContext` Playwright dengan `StorageState` +
     `Viewport` + `UserAgent` terpadu. Konteks dibuat via CDP
     `Target.createBrowserContext` (incognito); viewport di-set di level
     launch; User-Agent tidak di-override (memakai default Chromium).
     `.NET` juga tidak meng-override UA di konteks browser (UA kustom
     hanya dipakai renew HTTP).
   - Tidak ada auto-install browser (`playwright.ps1 install chromium`).
     Executable dicari dari `MUSE2API_CHROMIUM_PATH` → nama umum di PATH
     → `/opt/meta-chromium/chrome` (VM ini) → deteksi chromiumoxide.
     Di PC Windows Bree, `MUSE2API_CHROMIUM_PATH` harus menunjuk
     Chromium Playwright yang sudah terinstal
     (`chromium-1248` + headless shell) atau Chrome yang ada.
   - Tidak ada `WaitUntilState.DOMContentLoaded` / auto-wait Playwright;
     readiness dipolling manual via `PageState` (sama seperti .NET yang
     juga polling `PageState` setelah `GotoAsync`).
   - `page.keyboard` Playwright diganti CDP `Input.dispatchKeyEvent`
     (Enter/Escape). Perilaku di halaman React seharusnya setara, tapi
     belum terbukti melawan muse.ai asli (lihat § "Yang masih harus
     dibuktikan").
2. **Satu tab per akun, bukan pool tab** (`.NET: Checkout`, limit
   `AccountMaxConcurrency`, `PoolAcquireTimeout`). Dengan satu tab +
   kunci per akun, request ke akun yang sama diserialisasi penuh.
   `AccountMaxConcurrency > 1` diabaikan secara diam-diam (aman, tapi
   throughput akun tunggal lebih rendah). `PoolAcquireTimeout` tidak
   berlaku (kunci `tokio::Mutex` tidak bertimeout — request menunggu
   sampai tab bebas; pembatalan via drop stream).
3. **Tidak ada `LocalConversationStore`** (`LocalConversationStore.cs`,
   47 baris — milik `muse-store`, bukan crate ini). Dampak jalur
   local-tools:
   - Reuse percakapan hanya bila tab live masih memegang thread yang
     sama (hint cocok + URL thread). Setelah restart proses / tab
     tertutup, percakapan lokal TIDAK bisa dilanjutkan (URL tersimpan
     tidak ada) — .NET bisa via file store.
   - Dedup gambar via `ImageBatchId` + `ImageHash` tersimpan tidak
     jalan (gambar selalu di-upload ulang tiap ronde).
   - `Save`/`Load` percakapan lokal (URL + status selesai) tidak
     dipanggil — gateway/agent harus mengisi ini saat `muse-store` ada.
   - `LocalContinuation` / `ForLocalExecutor(compact)` sudah di-port
     verbatim ke `prompt.rs` (sementara — hapus saat `muse-core::Prompt`
     tersedia).
4. **`hot_page` tidak persisten.** .NET menyimpan hot state di
   `account.Meta["hot_page"]` dan gateway menulisnya ke `accounts.json`
   (`SaveHot`, `BrowserDriver.cs` ~292-305), sehingga follow-up bisa
   lanjut setelah restart. Di Rust, `Account` dimiliki gateway dan
   driver menerima snapshot by value — hot state hanya hidup di memori
   tab. Setelah restart, tab mulai kosong (chat baru, bukan lanjutan).
   Perlu API gateway↔driver untuk round-trip `hot_page` saat integrasi.
5. **Cookie refresh tidak bermutasi balik.** .NET memutasi
   `account.Cookies` langsung (class) di `SaveSession`
   (`BrowserDriver.cs` ~900-930). Di Rust, driver menyimpan cookie baru
   ke (a) file storage-state dan (b) map internal yang dibaca gateway
   lewat `MuseDriver::refreshed_session(account_id)` — gateway WAJIB
   memanggil ini setelah stream chat selesai, kalau tidak `accounts.json`
   tidak pernah mendapat cookie baru (sesi bisa basi padahal browser
   sudah me-refresh-nya).
6. **Storage-state: `origins` selalu kosong.** `SaveSession` .NET
   menyimpan `context.StorageStateAsync()` lengkap (termasuk
   `localStorage` per origin). CDP di sini hanya membaca cookie via
   `Storage.getCookies`; localStorage tidak ditangkap. Bila muse.ai
   menyimpan sesuatu yang penting di localStorage (bukan hanya cookie),
   sesi hasil restore bisa kurang lengkap. Belum terbukti bermasalah —
   cookie `hatch_sess` + `hatch_native_auth_device` adalah yang
   diverifikasi `LoginForm.cs` (§9 kontrak).
7. **File storage-state Windows (DPAPI) tidak bisa dibaca.** .NET di
   Windows mengenkripsi file dengan DPAPI (`muse-dpapi-v1:`). Crate ini
   mengembalikan `None` untuk file itu (diperlakukan sebagai cache
   rusak → pulih dari cookie akun). Dekripsi adalah tugas `muse-core`
   (kontrak §11); setelah ada, `BrowserSessionStore` harus memakainya
   agar file sesi Windows lama tetap terbaca.
8. **Profil Chromium ephemeral unik per launch.** .NET mendefinisikan
   `Settings.ProfileDir` tapi `BrowserDriver.cs` tidak pernah
   meneruskannya ke Playwright — konteks selalu ephemeral dan
   persistensi hanya via storage-state. Rust menyamakan itu: tiap
   launch memakai `user_data_dir` temp UNIK (dibersihkan saat
   `shutdown()`), bukan `ProfileDir`. Alasan teknis tambahan:
   chromiumoxide default-nya satu dir tetap
   (`/tmp/chromiumoxide-runner`) — dua launch bersamaan saling
   mengunci `SingletonLock` dan gagal; dir unik per launch
   menghilangkan tabrakan itu (ditemukan saat test paralel).
9. **Keepalive tidak ada.** `Settings.KeepaliveEnabled` /
   `KeepaliveInterval` (.NET `BackgroundServices.cs`) adalah layanan
   gateway, bukan driver — di luar crate ini.
10. **Failover antar akun bukan tugas driver.** `.NET: Gateway.cs`
    (retry `MaxFailover` sebelum output keluar) tinggal di gateway;
    driver hanya melempar `MuseError` dengan `retryable` yang benar
    (`ResponseUnavailable`/`Refused` = tidak retryable — upstream
    mungkin sudah menerima request, jangan kirim ulang).
11. **Kuota memakai tab chat yang sama.** .NET `CheckoutIdle` lebih
    memilih tab tanpa state agar kuota tidak mengganggu chat
    (`BrowserDriver.cs` ~958-975). Dengan satu tab per akun, baca kuota
    memakai tab yang ada (bisa menutup dialog Settings dengan Escape;
    bila dialog masih terbuka, tab ditandai stale dan ditutup).

## Yang masih harus dibuktikan di PC Windows Bree

0. **Chrome di VM ini TIDAK bisa memuat halaman internet sama sekali
   (batas lingkungan, bukan cacat driver).** Fakta yang terukur
   2 Nov 2026:
   - `/opt/meta-chromium/chrome` (Hatch Chromium 155) menolak start
     tanpa WebGL domain policy dari browser-daemon
     (`chrome_main_delegate.cc:1212`); README-nya: "Normal startup
     requires the image-owned WebGL policy supplied by browser-daemon".
     Lima format file policy coba-coba (JSON array/objek, teks
     baris) semuanya ditolak ("invalid WebGL domain policy contract");
     tidak ada file policy resmi di image. Jadi binary itu TIDAK
     bisa dipakai untuk pengujian driver di VM.
   - Chrome stock 156 diinstal dari `google-chrome-stable` .deb
     (2 Nov 2026, `/usr/bin/google-chrome`) — launch + CDP jalan,
     tapi Chrome TIDAK bisa memuat halaman eksternal apa pun
     (example.com maupun muse.ai, HTTP maupun HTTPS, via proxy
     maupun langsung, `--single-process` maupun tidak): netlog
     menunjukkan DNS `-105` + CONNECT proxy `ERR_EMPTY_RESPONSE`
     (`-324`); `curl` lewat proxy yang sama (`hatch-egress-proxy:3128`)
     mengembalikan 200 untuk muse.ai dan example.com. Dugaan:
     sandbox/jaringan proses anak Chrome diblokir di cell VM ini.
   - Akibatnya, test `navigate_real_muse_ai_without_login` SKIP
     dengan jujur (halaman error Chrome terdeteksi, bukan diklaim
     berhasil). Bukti load muse.ai yang sebenarnya HANYA bisa
     dilakukan di PC Bree — di sana Chrome/Playwright sudah terbukti
     jalan (chromium-1248 terinstal).
   - Yang TETAP terbukti di VM: launch Chromium + CDP connect +
     navigasi `data:` + eval skrip `Dom` (PageState/ChatState/
     FillInput/ClickSend) + lifecycle driver.

1. **Chat end-to-end melawan muse.ai dengan akun login asli.**
   Di VM ini hanya terbukti: launch Chromium + navigasi + eval DOM di
   halaman tiruan dan di `muse.ai/thread/new` tanpa login (halaman login
   muncul seperti yang diharapkan). Yang belum terbukti: isi input →
   kirim → balasan asisten terbaca via `ChatState` di UI muse.ai yang
   sebenarnya, termasuk `generating` / stabil / approval / kuota.
   Uji di PC: `MUSE2API_DRIVER=browser`, akun dengan cookie
   `hatch_sess` + `hatch_native_auth_device`, chat pendek, amati
   delta mengalir + `Done`.
2. **Chromium Playwright Windows** (`chromium-1248` + headless shell,
   sudah terinstal di PC Bree): set `MUSE2API_CHROMIUM_PATH` ke
   `chrome.exe` Playwright-nya (atau biarkan deteksi bila ada di PATH).
   Headless shell (`headless_shell.exe`) mungkin tidak mendukung semua
   flag/CDP yang dipakai — bila launch gagal, pakai `chrome.exe`
   penuh (bukan headless shell). Verifikasi: `startup()` + `health`
   `ok=true`.
3. **Renew sesi + VM wake** (`Upstream.RenewSession` + `vm/wake`):
   hanya ter-port dari source, belum pernah jalan melawan akun asli
   dari Rust (maupun dari .NET di VM ini). Uji `renew_session` pada
   akun yang VM-nya tidur dan yang `RUNNING`.
4. **Kuota via klik Settings**: selektor `hatch-dock-more` + menu
   Settings + parsing dialog — port verbatim dari `Dom.cs`, tapi
   belum diklik sungguhan di UI muse.ai dari Rust.
5. **Storage-state antar-restart di Windows**: chat → tutup →
   buka lagi → tab harus pulih dari file `browser-sessions/*.json`
   tanpa login ulang. (Di Windows, file ditulis .NET terenkripsi DPAPI
   dan TIDAK terbaca crate ini — butuh butir 7 di atas — jadi uji ini
   harus dilakukan setelah `muse-core` menyediakan DPAPI, atau dengan
   file yang ditulis Rust sendiri.)
6. **Keepalive / failover / pool** adalah tanggung jawab gateway
   (`muse-gateway` + `Gateway.cs`), bukan driver — diuji di sana.

## Keputusan desain yang disengaja (bukan kekurangan)

- Stream chat = `ChatEvent::AssistantDelta(String)` + `Done` terakhir
  (kontrak §11), bukan string mentah .NET — gateway membungkusnya ke
  SSE (`assistant_delta` / `done`, kontrak §7).
- `Account` lokal minimal (lihat `lib.rs`): `muse-store` akan
  menggantikannya dengan tipe kanonis `accounts.json`.
- Pembatalan = drop stream (task chat di-abort). Tidak ada
  `CancellationToken` yang diteruskan ke dalam stream.
- Error memakai `MuseError` (status + `type` + `code` + `retryable`)
  sesuai `Errors.cs`; `health()` mengembalikan JSON yang sama isinya
  dengan .NET (`driver`, `ok`, `tabs`, `busy_tabs`).
