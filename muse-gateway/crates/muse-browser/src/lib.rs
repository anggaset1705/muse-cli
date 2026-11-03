//! `muse-browser` — driver browser untuk backend Muse (Rust).
//!
//! Port dari `src/Muse2Api/Drivers.cs`, `BrowserDriver.cs` (Playwright),
//! `BrowserSessionStore.cs`, `Upstream.cs`, dan `Dom.cs` di repo .NET
//! (`~/workspace/muse-cli-en-id`, hanya dibaca — jangan diubah).
//!
//! Isi crate:
//! - [`MuseDriver`] — trait driver (mirip `IMuseDriver` .NET) + [`DriverFactory`]
//! - [`MockDriver`] — driver offline deterministik (balasan persis .NET)
//! - [`HttpDriver`] — chat 501 (persis .NET), renew sesi via HTTP
//! - [`BrowserDriver`] — driver Chromium via CDP ([`chromiumoxide`])
//! - [`dom`] — selektor + skrip JS muse.ai (port `Dom.cs` 1:1 sebisa mungkin)
//! - [`upstream`] — fakta upstream muse.ai + renew sesi HTTP (port `Upstream.cs`)
//! - [`BrowserSessionStore`] — snapshot state browser per akun (port
//!   `BrowserSessionStore.cs`)
//!
//! Catatan integrasi (2 Nov 2026): tipe [`Account`] di sini adalah definisi
//! lokal minimal (hanya field yang dipakai driver). Tipe kanonis `accounts.json`
//! tinggal di `muse-store::Account`; gateway mengonversi store → browser saat
//! memanggil driver (lihat `muse-gateway/src/adapters.rs`) — tipe lokal ini
//! dipertahankan sebagai tipe wire driver, bukan diganti. `Prompt` dan
//! `Settings` kanonis tinggal di `muse-core` (modul `prompt.rs` lokal dan
//! struct `DriverSettings` lokal sudah dihapus). Lihat `TRADEOFFS.md`.

pub mod browser;
pub mod config;
pub mod dom;
pub mod driver;
pub mod error;
pub mod http;
pub mod mock;
pub mod session_store;
pub mod types;
pub mod upstream;

pub use browser::{BrowserConfig, BrowserDriver};
pub use config::DriverSettings;
pub use driver::{ChatStream, DriverFactory, MuseDriver};
pub use error::MuseError;
pub use http::HttpDriver;
pub use mock::MockDriver;
pub use session_store::BrowserSessionStore;
pub use types::{
    Account, ChatEvent, ChatRequest, DriverCapabilities, ImageRequest, InputImage, MediaResult,
    SessionInfo, VideoRequest,
};
pub use upstream::Upstream;
