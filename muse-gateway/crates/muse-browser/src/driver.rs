//! Trait driver + factory — port `IMuseDriver` + `DriverFactory`
//! (`Drivers.cs`).
//!
//! Perbedaan dari .NET yang disengaja (terdokumentasi):
//! - `ChatStream` .NET menerima `Account` (class, mutable bersama) dan
//!   mengembalikan `IAsyncEnumerable<string>` (delta teks mentah). Di Rust,
//!   stream mengembalikan [`ChatEvent`] (`assistant_delta` / `done`, kontrak
//!   §11) dan menerima `Account` **by value** (snapshot; gateway meng-clone).
//!   Cookie yang diperbarui driver selama chat TIDAK bermutasi balik ke
//!   `Account` pemanggil — driver menyimpannya ke [`BrowserSessionStore`]
//!   dan mengeksposnya lewat [`MuseDriver::refreshed_session`]; gateway
//!   yang memanggil itu setelah stream selesai untuk memperbarui
//!   `accounts.json`. Lihat `TRADEOFFS.md`.
//! - Pembatalan: drop stream (receiver ditutup) → tugas chat di-abort.
//!   Tidak ada `CancellationToken` yang diteruskan ke dalam stream.
//! - `Health()` mengembalikan `serde_json::Value` (map), sama isinya dengan
//!   .NET (`driver`, `ok`, + field khusus driver).

use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures_util::Stream;

use crate::browser::BrowserDriver;
use crate::config::DriverSettings;
use crate::error::MuseError;
use crate::http::HttpDriver;
use crate::mock::MockDriver;
use crate::types::{
    Account, ChatEvent, ChatRequest, DriverCapabilities, ImageRequest, MediaResult, SessionInfo,
    SessionRefresh, VideoRequest,
};

/// Stream event chat: delta teks atau error. `Done` dikirim sebagai event
/// terakhir oleh driver (bukan oleh stream itu sendiri yang habis).
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatEvent, MuseError>> + Send>>;

/// Port `IMuseDriver` (`Drivers.cs`).
#[async_trait]
pub trait MuseDriver: Send + Sync {
    /// Nama driver wire: `mock` / `browser` / `http`.
    fn name(&self) -> &'static str;

    /// Apakah driver butuh akun dari pool (`.NET: RequiresAccount`).
    fn requires_account(&self) -> bool;

    fn capabilities(&self) -> DriverCapabilities;

    /// `.NET: StartupAsync()` — default no-op.
    async fn startup(&self) -> Result<(), MuseError> {
        Ok(())
    }

    /// `.NET: ShutdownAsync()` — default no-op.
    async fn shutdown(&self) -> Result<(), MuseError> {
        Ok(())
    }

    /// `.NET: InvalidateAccount(accountId)` — buang tab/konteks + cache
    /// akun (dipanggil setelah renew / saat akun dicurigai basi).
    async fn invalidate_account(&self, _account_id: &str) -> Result<(), MuseError> {
        Ok(())
    }

    /// `.NET: ChatStream(Account, ChatRequest)` — stream event chat.
    /// Implementasi WAJIB mengakhiri stream dengan `ChatEvent::Done` bila
    /// berhasil, atau satu `Err` bila gagal (jangan dua-duanya).
    fn chat_stream(&self, account: Account, req: ChatRequest) -> ChatStream;

    /// `.NET: GenerateImage` — default `NotImplemented`, seperti .NET.
    async fn generate_image(
        &self,
        _account: Account,
        _req: ImageRequest,
    ) -> Result<Vec<MediaResult>, MuseError> {
        Err(MuseError::not_implemented(format!(
            "driver '{}' does not support image generation",
            self.name()
        )))
    }

    /// `.NET: GenerateVideo` — default `NotImplemented`, seperti .NET.
    async fn generate_video(
        &self,
        _account: Account,
        _req: VideoRequest,
    ) -> Result<MediaResult, MuseError> {
        Err(MuseError::not_implemented(format!(
            "driver '{}' does not support video generation",
            self.name()
        )))
    }

    /// `.NET: RenewSession` — default `NotImplemented`, seperti .NET.
    async fn renew_session(&self, _account: Account) -> Result<SessionInfo, MuseError> {
        Err(MuseError::not_implemented(format!(
            "driver '{}' does not support session renewal",
            self.name()
        )))
    }

    /// `.NET: Quota` — default `NotImplemented`, seperti .NET.
    async fn quota(
        &self,
        _account: Account,
    ) -> Result<HashMap<String, serde_json::Value>, MuseError> {
        Err(MuseError::not_implemented(format!(
            "driver '{}' does not support quota lookup",
            self.name()
        )))
    }

    /// `.NET: Health()` — default `{driver, ok:true}`.
    async fn health(&self) -> Result<serde_json::Value, MuseError> {
        Ok(serde_json::json!({ "driver": self.name(), "ok": true }))
    }

    /// Cookie sesi yang diperbarui driver selama chat terakhir akun ini
    /// (untuk ditulis gateway ke `accounts.json`). `None` bila tidak ada
    /// pembaruan (mock/http mengembalikan `None`).
    ///
    /// Tidak ada padanan langsung di .NET: di sana `BrowserDriver`
    /// memutasi `account.Cookies` secara langsung (class). Di Rust,
    /// gateway memanggil ini setelah stream chat selesai.
    fn refreshed_session(&self, _account_id: &str) -> Option<SessionRefresh> {
        None
    }
}

/// Port `DriverFactory.Create(Settings)` (`Drivers.cs`).
pub struct DriverFactory;

impl DriverFactory {
    /// `settings.Driver`: `"browser"` | `"http"` | `"mock"`, selain itu
    /// error persis .NET: `unknown driver '<name>'`.
    pub fn create(settings: DriverSettings) -> Result<Box<dyn MuseDriver>, MuseError> {
        match settings.driver.as_str() {
            "browser" => Ok(Box::new(BrowserDriver::new(settings))),
            "http" => Ok(Box::new(HttpDriver::new())),
            "mock" => Ok(Box::new(MockDriver::new())),
            other => Err(MuseError::invalid(format!("unknown driver '{other}'"))),
        }
    }

    /// Seperti [`DriverFactory::create`] tapi dari nama saja (dipakai test
    /// + gateway yang belum punya `DriverSettings` lengkap).
    pub fn create_by_name(name: &str) -> Result<Box<dyn MuseDriver>, MuseError> {
        let mut settings = DriverSettings::load();
        settings.driver = name.to_string();
        Self::create(settings)
    }
}
