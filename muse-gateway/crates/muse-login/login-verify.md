# muse-login — verifikasi Windows

Crate ini **belum pernah dijalankan di Windows**. Yang sudah terbukti di VM
Linux ini:

| Cek | Hasil |
|---|---|
| `cargo check` (Linux) | lolos, 0 error 0 warning |
| `cargo check --target x86_64-pc-windows-msvc` | lolos, 0 error 0 warning — **seluruh `src/windows.rs` sudah type-check** melawan `webview2-com` 0.39.1 + `windows` 0.62 |
| `cargo test` (Linux) | 13 unit + 5 integrasi, semua lolos |
| `cargo build` Windows / link | **belum** — butuh MSVC `link.exe` + `webview2loader.lib`, tidak ada di VM ini |

Cross-check Windows bisa jalan di VM ini karena `ureq` di target Windows
memakai `native-tls` (SChannel, seperti `.NET HttpClient`), bukan `rustls`/
`ring` yang build script-nya butuh `lib.exe`. Perilaku POST tidak berubah:
gateway adalah loopback HTTP, TLS tidak dipakai kecuali `gateway_url`
berupa `https`.

## Tema gelap + manifest (3 Nov 2026, dikerjakan di VM, BELUM dilihat di PC)

`src/windows.rs` disamakan dengan `DesktopTheme` WinForms (dark palette):
latar jendela `#141414`, label heading `#ECECEC` Segoe UI 17 bold,
desc/status `#8E8E93` Segoe UI 10, tombol owner-drawn (`BS_OWNERDRAW` +
`WM_DRAWITEM`) meniru `CleanButton` — Retry isi Surface `#202020` +
border `#2E2E2E`, "Another account" Quiet (polos saat idle, isi
SurfaceStrong `#2C2C2C` saat hover/tekan), ring fokus Accent; title bar
gelap via `DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE)`; latar
WebView gelap via `ICoreWebView2Controller2::SetDefaultBackgroundColor`
(best-effort, diabaikan bila Runtime lama); jendela ditampilkan SEBELUM
`init_webview` (gelap langsung terlihat); status "Connected" ditahan
~1 detik (timer, tanpa `Sleep` di UI thread) sebelum jendela menutup.

Manifest Common Controls v6 ada di `crates/muse-gateway/`:
`muse-gateway.manifest` + `muse-gateway.rc` + `build.rs`
(`embed-resource` 3.0.12, `[build-dependencies]` di
`crates/muse-gateway/Cargo.toml`). **Saat install di PC, salin juga
`muse-gateway.manifest` ke samping exe sebagai
`muse-gateway.exe.manifest`** — ini penutup bila build di PC ternyata
tanpa resource compiler (`rc.exe`), sehingga manifest tidak ikut
ter-embed. Bila manifest ter-embed, file sidecar diabaikan Windows.

Verifikasi VM (3 Nov): `cargo check -p muse-login --target
x86_64-pc-windows-msvc` lolos 0 error 0 warning; `cargo check
--workspace` (Linux) lolos; `cargo test -p muse-login` 13 unit + 5
integrasi lolos; `cargo test --workspace` lolos kecuali 1 kegagalan
pre-existing `browser_driver_lifecycle_and_missing_cookies`
(muse-browser, Chrome "Hatch WebGL domain policy" di VM).
`cargo check -p muse-gateway --target x86_64-pc-windows-msvc` TIDAK
bisa jalan di VM ini — build script `ring`/`zstd-sys` (dependensi
reqwest/rustls yang sudah ada) butuh `lib.exe` MSVC; ini keterbatasan
VM, bukan regresi dari perubahan ini. Jalur `build.rs` manifest sendiri
terverifikasi terpisah di crate kecil: `cargo check --target
x86_64-pc-windows-msvc` lolos (embed-resource 3.0.12, tanpa `llvm-rc`
→ build tetap sukses, manifest sidecar yang menutup).

Yang masih harus dicek di PC (tambah ke langkah 2 di atas): jendela
tampil gelap sejak muncul (bukan putih), tombol "Another account"
tampil sebagai teks polos tanpa border dan berubah isi saat hover,
title bar gelap, area WebView tidak putih saat loading, dan "Connected"
terlihat ~1 detik sebelum jendela menutup.

## Checklist verifikasi di PC Windows Bree

Jalankan dari PowerShell di `C:\` (atau di mana pun repo berada), dengan
gateway .NET berjalan di `http://127.0.0.1:18610` dan kunci admin dari file
`api_key` (atau `MUSE2API_ADMIN_KEY`).

> Catatan: crate ini belum punya `main`/binary — verifikasi butuh penguji
> kecil (lihat langkah 1) atau integrasi ke gateway Rust oleh koordinator.
> Jangan login sebagai Bree ke akun lain; pakai akun Muse Bree sendiri.

### 1. Build Windows (link)

```powershell
cd ~\workspace\muse-rust-backend\crates\muse-login
cargo build --target x86_64-pc-windows-msvc
```

Harapan: selesai tanpa error. Bila gagal di `webview2loader.lib`, pasang
WebView2 Runtime / SDK NuGet `Microsoft.Web.WebView2` dan ulangi.

### 2. Uji penguji kecil (belum ada — buat saat integrasi)

Contoh `examples/login_smoke.rs` (belum ditulis):

```rust
fn main() {
    let r = muse_login::show_login(
        "http://127.0.0.1:18610",
        "<ADMIN_KEY>",
        "Personal account",
        std::path::Path::new("<DATA_ROOT>"), // mis. %LocalAppData%\Muse2Api.WinForms
        &[],
    );
    println!("{r:?}");
}
```

Jalankan, lalu:

1. Jendela 1060x800 muncul, judul "Connect your Muse account",
   heading "Sign in to your Muse account", tombol "Another account" di
   kanan bawah, tombol "Retry" tersembunyi.
2. Halaman `https://muse.ai` tampil. Login sebagai Bree di halaman itu
   (jangan tempel cookie manual).
3. Harapan: setelah login, status berubah "Checking and restoring your
   saved Muse session…" → "Signed in. Saving the connection…" →
   "Connected. You can start working on your project." Lalu jendela
   menutup sendiri dan program mencetak `Ok(LoginResult { account_id:
   "acc_..." })`.
4. Cek di gateway: `GET /admin/accounts` (header `Authorization: Bearer
   <ADMIN_KEY>`) menampilkan akun baru dengan `label` "Personal account"
   dan `missing_cookies: []`.
5. Tutup jendela sebelum login → program mencetak
   `Err(LoginError::Cancelled)` dan **tidak** ada akun baru di gateway.
6. Klik "Another account" setelah login → cookie terhapus, halaman
   kembali ke form login muse.ai, status "Sign in to another account on
   the Muse page."
7. Login kedua dengan `saved_cookies` dari akun langkah 3 (baca dari
   `accounts.json` di data dir): WebView harus langsung terverifikasi
   tanpa mengetik kredensial (cookie dipulihkan), dan tidak ada jendela
   login kedua.
8. Matikan gateway, ulangi login → status "Signed in, but the connection
   could not be saved. Click Retry…", tombol "Retry" muncul. Nyalakan
   gateway, klik "Retry" → akun tersimpan tanpa login ulang.
9. Cek profil WebView2: folder `<DATA_ROOT>\WebView2` terbentuk dan
   berisi data Edge WebView2; **bukan** profil browser pribadi
   (`%LocalAppData%\Microsoft\Edge` tidak berubah).

### 3. Uji regresi yang sudah lolos di Linux

```powershell
cargo test
```

Harapan: 13 unit + 5 integrasi lolos (termasuk fake-gateway:
`POST /admin/accounts` dengan header Bearer + body
`{label, cookies, reuse_session: true}`).

### 4. Perbaikan 1 — status menunggu-redirect + log runtime (3 Nov 2026)

Dikerjakan di VM, BELUM dilihat di PC. Yang berubah:

- Bila cookie sudah lengkap tapi halaman masih di redirect login
  (Meta/OAuth, bukan `muse.ai`), status kini menampilkan
  "Signed in — waiting for the Muse page…" (bukan diam di "Waiting for
  you to sign in" tanpa Retry). Begitu halaman kembali ke `muse.ai`,
  cek sesi diinjeksikan seperti biasa. Gate keamanan tetap: cek hanya
  diinjeksikan di `muse.ai`.
- Log runtime: tiap jalan login menulis baris tahap ke
  `<data_dir>\login.log` (satu folder dengan `accounts.json`,
  dibuka dari "Buka folder data"): `init`, `cookies-complete`,
  `waiting-redirect` (beserta host redirect), `session-check-injected`,
  `session-message` (`authenticated=true/false`), `gateway-post`
  (ok/gagal + status HTTP), plus `login complete: account …`.
  File di-append tiap jalan (dihapus dulu bila > 512 KiB) dan TIDAK
  berisi nilai cookie/token.

Verifikasi di PC (sesudah build + pasang oleh pekerja lain):

1. Buka jendela login, login seperti biasa → `login.log` terbentuk
   dan berisi baris `init` → `cookies-complete` →
   `session-check-injected` → `session-message: authenticated=true`
   → `gateway-post: ok` → `login complete: account acc_…`.
2. (Bila bisa direproduksi) login yang mampir ke redirect OAuth:
   status "Signed in — waiting for the Muse page…" tampil, lalu
   berubah ke "Checking…" saat halaman kembali ke `muse.ai`;
   `login.log` berisi `waiting-redirect: host=<domain redirect>`.
3. `cargo test -p muse-login` di VM sudah mencakup cabang baru ini
   lewat keputusan murni `refresh::decide_refresh` (logika `windows.rs`
   tidak dikompilasi di Linux).

### 5. Perbaikan darurat — crash RefCell di `WM_CTLCOLORSTATIC` (3 Nov 2026)

Crash nyata di PC Bree, dibuktikan `login.log`: `init` →
`webview-init` → `cookies-complete: required=2 present, total=7` →
`thread 'main' panicked at windows.rs:1056:30: RefCell already
mutably borrowed` → `panic in a function that cannot unwind` →
`aborting`. Penyebab: `set_status` (dulu `&LoginState`) memanggil
`SetWindowTextW` sementara `refresh_inner` memegang `borrow_mut()`;
`SetWindowTextW` synchronously mengirim `WM_CTLCOLORSTATIC` ke
window proc, yang handler-nya `cell.borrow()` — pinjam ganda di
dalam window proc yang tidak bisa unwind → abort, jendela lenyap
tanpa save. Regresi ronde tema gelap 11:01 (sebelumnya tidak ada
handler `WM_CTLCOLORSTATIC` yang meminjam cell).

Perbaikan (`windows.rs`):

- `set_status` / `show_retry` kini menerima HWND polos (bukan
  `&LoginState`) dan HANYA dipanggil tanpa borrow yang dipegang —
  semua call site snapshot HWND dulu (di `refresh_inner`, snapshot
  `status_hwnd` ikut tuple input). Tampilan gelap tidak berubah.
- `set_status` kini juga mencatat tiap perubahan status ke
  `login.log` (`status: <teks>`) — jejak untuk diagnosis login
  berikutnya.
- Handler `wnd_proc` yang membaca cell (`WM_SIZE`,
  `WM_CTLCOLORSTATIC`, `WM_DRAWITEM`) kini memakai `try_borrow()`
  dengan fallback aman (warna muted / font null / lewati layout
  satu frame) — mereka bisa terpanggil re-entrant dari nested pump
  di `wait_for_async_operation` (`GetMessage` + `DispatchMessage`
  saat menunggu async WebView2) maupun sinkron dari `SetWindowTextW`.
- Audit: tidak ada lagi `borrow_mut` yang dipegang melewati
  `set_status`/`show_retry`; `get_cookies` / `execute_script` /
  `post_cookies_to_gateway` / `create_environment` /
  `create_controller` semuanya dipanggil tanpa borrow yang dipegang
  (meski `wait_for_async_operation` memompa pesan saat menunggu).

Verifikasi di PC (sesudah build + pasang):

1. Login ulang di jendela in-app: TIDAK ada crash; `login.log`
   berisi `status: Signed in — waiting for the Muse page…` (bila
   mampir redirect) dan/atau `status: Connected. You can start
   working on your project.` lalu jendela menutup sendiri ~1 detik.
2. `accounts.json` berisi 1 akun sesudah login; pill sambutan
   berubah jadi "✓ Connected".
3. `cargo test -p muse-login` (20 unit + 5 integrasi) dan
   `cargo check -p muse-login --target x86_64-pc-windows-msvc`
   0 error 0 warning di VM; handler Win32 tidak bisa dites di
   Linux — pembuktian runtime hanya di PC Bree.

## Yang masih berisiko (belum terbukti di runtime Windows)

1. **CookieManager ada di `ICoreWebView2_2`, bukan `ICoreWebView2`.**
   Kode memakai `webview.cast::<ICoreWebView2_2>()` lalu
   `CookieManager()` (`src/windows.rs`, `init_webview`). Interface ini
   ada sejak WebView2 SDK 1.0.705 / Edge 87, jadi praktis selalu ada —
   tapi bila `cast()` gagal di PC lama, login gagal total dengan
   `LoginError::WebView`. Alternatif bila terbukti bermasalah: ambil
   `ICoreWebView2Profile` dari `ICoreWebView2_2.Environment()` →
   `Profile()` → `CookieManager()` (API yang sama, jalur berbeda).
2. **`GetCookies` / `WebMessageReceived` / `ExecuteScript` — alur async.**
   Semua ditulis mengikuti pola `wait_for_async_operation` +
   `wait_with_pump` dari `examples/sample.rs` crate `webview2-com`.
   Pola ini memompa message loop secara nested di dalam handler
   `WM_TIMER`. Re-entrancy dijaga flag `refreshing` (mirip `_refreshing`
   di .NET), tapi interaksi `wait_with_pump` (pakai `GetMessageA`)
   dengan loop utama (`GetMessageW`) dan timer 2 detik belum pernah
   dieksekusi — perhatikan apakah jendela freeze atau timer menumpuk
   saat `fetch('/api/session')` lambat.
3. **`CreateCookie` / `AddOrUpdateCookie` saat restore.**
   Dipanggil dengan domain `".muse.ai"`, path `"/"`, `IsSecure = true`,
   `Expires` (Unix detik, `f64`) hanya bila di masa depan — sesuai
   `LoginForm.cs`. Belum terbukti cookie hasil restore benar-benar
   dipakai sesi berikutnya; bila `muse.ai` menolak cookie (mis. atribut
   `SameSite`/`HttpOnly` tidak cocok), langkah 7 di atas akan gagal
   dan user harus login ulang tiap kali. Periksa juga: cookie
   `hatch_sess` asli mungkin `HttpOnly` — `CreateCookie` default
   `IsHttpOnly = false`; .NET juga tidak menyetelnya, jadi ini
   mengikuti .NET apa adanya.
4. **Nonce + host check.** `parse_session_message` menolak host selain
   `muse.ai` dan nonce salah (unit-tested). Tapi `Source` dari event
   args bisa berupa URL redirect (mis. `https://muse.ai/login?...`)
   — host tetap `muse.ai`, aman. Bila Muse memakai subdomain login
   (mis. `auth.muse.ai`), verifikasi akan selalu ditolak; saat itu
   periksa `message.Source` yang sebenarnya di log.
5. **COM STA.** `show_login_impl` memanggil `CoInitializeEx(STA)` dan
   `CoUninitialize` bila ia yang menginisialisasi. Bila pemanggil
   (gateway Rust) sudah menginisialisasi COM sebagai MTA di thread itu,
   hasilnya `LoginError::WebView` (RPC_E_CHANGED_MODE) — panggil
   `show_login` dari thread STA khusus.
6. **POST gateway blocking di UI thread.** `post_cookies_to_gateway`
   (ureq sinkron, timeout 30 dtk) dipanggil dari `refresh_inner` di UI
   thread; jendela membeku sebentar saat POST. .NET melakukannya async.
   Gateway loopback biasanya <100 ms, jadi diterima; bila gateway
   lambat, pindahkan POST ke thread pekerja.
7. **Satu login dalam satu waktu.** Account id diselundupkan keluar lewat
   static `LOGIN_RESULT_SLOT` (Mutex) karena `WM_NCDESTROY` membebaskan
   state sebelum message loop keluar. Dua `show_login` bersamaan akan
   saling menimpa slot — panggil secara berurutan saja.
8. **DPI.** `SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE)`
   dipanggil (seperti sample `webview2-com`); WinForms .NET mengatur
   DPI lewat manifest aplikasi. Bila jendela tampak terlalu kecil/
   besar di layar HiDPI, ini penyebab pertamanya.
9. **String UI hanya Inggris.** `UiStrings.cs` punya varian Indonesia;
   crate ini memakai default Inggris (seperti `.NET` bila UI berbahasa
   Inggris). Bila gateway Rust nanti butuh UI Indonesia, string di
   `src/windows.rs` modul `strings` tinggal diganti.
10. **WebView2 Runtime wajib ada.** Bila PC tidak punya Edge WebView2
    Runtime, `CreateCoreWebView2EnvironmentWithOptions` gagal →
    status "Could not open the sign-in window…" + tombol Retry.
    Instal dari https://developer.microsoft.com/microsoft-edge/webview2/
    (evergreen bootstrapper).

## Penyimpangan dari kontrak (dicatat)

- **Tanda tangan `show_login`**: kontrak §11 menulis
  `show_login(gateway_url, admin_key, label)`. Crate ini memakai
  `show_login(gateway_url, admin_key, label, data_root, saved_cookies)`
  sesuai instruksi tugas: `data_root` diperlukan untuk folder profil
  `<data_root>\WebView2` (LoginForm.cs memakai
  `DesktopDataPaths.Root`), dan `saved_cookies` adalah input restore
  akun tersimpan.
- **Flag `clearExistingSession` milik `LoginForm` tidak diekspos.**
  Aksi yang sama tersedia interaktif lewat tombol "Other account".
- **Label kosong** → `"Personal account"` (default Inggris
  `UiStrings["AccountPersonal"]`).
- **`POST /admin/accounts/import` tidak dipakai** — login memakai
  `POST /admin/accounts` dengan `reuse_session: true`, persis
  `GatewayClient.AddAccountWithCookies` (Client.cs:78-84).

## Referensi .NET yang ditiru (file + baris)

- `src/Muse2Api.WinForms/LoginForm.cs` — seluruh file (153 baris):
  ukuran jendela, profil WebView2, poll 2 dtk, inject JS + nonce,
  cookie wajib/opsional, restore, tombol, Retry, stop saat tutup.
- `src/Muse2Api.WinForms/DesktopDataPaths.cs:7-25` — `Root`:
  `%LocalAppData%\Muse2Api.WinForms` (atau `MUSE_DESKTOP_PROFILE_ROOT`).
- `src/Muse2Api.WinForms/Client.cs:72-84` — `AddAccountWithCookies`:
  body `{label, cookies, reuse_session: true}`, auth Bearer, tanpa proxy.
- `src/Muse2Api/Endpoints.cs:690-703` — `POST /admin/accounts`,
  respons `{account, missing_cookies}`; `:953-1010` `BuildAccount` +
  `ParseCookies` (bentuk `[{name, value, expirationDate?}]`).
- `src/Muse2Api/Upstream.cs:13-14,23-24` — `SessionCookies` /
  `MissingSessionCookies`.
- `src/Muse2Api.WinForms/UiStrings.cs:369,379-392` — string Inggris
  default yang dipakai jendela ini.
