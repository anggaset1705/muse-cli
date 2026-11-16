# muse-browser

Driver browser untuk backend Muse (Rust) — port dari .NET `Muse2Api`
(`~/workspace/muse-cli-en-id/src/Muse2Api/`, hanya dibaca):

| .NET | Rust di crate ini |
|---|---|
| `Drivers.cs` (`IMuseDriver`, `MockDriver`, `HttpDriver`, `DriverFactory`) | `driver.rs`, `mock.rs`, `http.rs` |
| `BrowserDriver.cs` (Playwright) | `browser.rs` (CDP via `chromiumoxide` 0.7) |
| `BrowserSessionStore.cs` | `session_store.rs` |
| `Upstream.cs` | `upstream.rs` |
| `Dom.cs` | `dom.rs` |
| `Prompt.FollowupText` / `LocalContinuation` (`Models.cs`) | `prompt.rs` |
| `Settings.cs` (field driver) | `config.rs` (`DriverSettings`) |
| `Errors.cs` | `error.rs` (`MuseError`) |

## Build mandiri

```sh
cargo check
cargo test
```

`Cargo.toml` crate ini WAJIB berisi tabel `[workspace]` kosong di akhir
agar bisa `cargo check`/`cargo test` mandiri; koordinator yang menghapusnya
saat integrasi ke workspace root. Jangan edit `Cargo.toml` root.

## Pemakaian

```rust
use muse_browser::{DriverFactory, DriverSettings, MuseDriver};
use muse_browser::types::{Account, ChatRequest};
use futures_util::StreamExt;

let settings = DriverSettings::from_env(); // MUSE2API_* (DRIVER=mock default)
let driver = DriverFactory::create(settings)?;
driver.startup().await?;
let mut stream = driver.chat_stream(Account::new("acc_xxx"), ChatRequest::new("halo", "muse-chat"));
while let Some(item) = stream.next().await {
    match item {
        Ok(muse_browser::types::ChatEvent::AssistantDelta(t)) => print!("{t}"),
        Ok(muse_browser::types::ChatEvent::Done) => break,
        Err(e) => eprintln!("error: {e}"),
    }
}
driver.shutdown().await?;
```

Driver yang tersedia: `mock` (offline, deterministik), `browser`
(Chromium via CDP), `http` (chat 501, renew via HTTP). Nama driver dari
`MUSE2API_DRIVER`; nama tak dikenal → error `unknown driver '<nama>'`
(persis .NET).

## Perbedaan perilaku vs .NET

Lihat `TRADEOFFS.md` — terutama:

- `Account` di crate ini adalah definisi lokal minimal; saat integrasi
  diganti tipe kanonis `muse-store`.
- Chat mengembalikan stream `ChatEvent` (`assistant_delta` / `done`),
  bukan delta string mentah seperti .NET.
- Cookie yang diperbarui driver selama chat tidak bermutasi balik ke
  `Account` pemanggil — gateway memanggil
  `MuseDriver::refreshed_session(account_id)` setelah stream selesai.
- Satu tab per akun (bukan pool multi-tab); `AccountMaxConcurrency > 1`
  tetap diserialisasi.
- `LocalConversationStore` (URL thread per `conversation_hint`) belum ada
  — jalur local-tools hanya reuse tab live.
- File storage-state di Windows yang ditulis .NET (DPAPI) tidak bisa
  dibaca di sini; di Linux (plaintext) kompatibel.

## Test

- Unit test: mock driver (balasan persis `Drivers.cs:57`), factory,
  `Dom::parse_usage_text`, session store, prompt, upstream, browser
  helper (URL/media/scoring).
- Integration test (`tests/browser_launch.rs`): launch Chromium headless
  di VM, navigasi halaman `data:` tiruan DOM muse.ai + `https://muse.ai`
  tanpa login, eval skrip `Dom`. Bila Chromium tidak ditemukan, test SKIP
  dengan jelas (bukan pura-pura lolos). Chromium di VM ini:
  `/usr/bin/google-chrome` (stock Chrome 156, diinstal 2 Nov 2026
  untuk pengujian — `/opt/meta-chromium/chrome` yang sudah ada
  TIDAK bisa start standalone: binary Hatch itu menolak start tanpa
  WebGL domain policy dari browser-daemon) atau set
  `MUSE2API_CHROMIUM_PATH`.
