//! Tipe data driver — port minimal dari `Drivers.cs` + `Store.cs` (.NET).
//!
//! [`Account`] di sini adalah definisi LOKAL minimal (hanya field yang dipakai
//! driver). Kontrak §6 mendefinisikan bentuk penuh `accounts.json`; saat
//! integrasi, `muse-store` menyediakan tipe kanonis dan tipe ini diganti.
//! Field dan nama JSON-nya sudah disamakan dengan .NET agar ser/de tetap
//! kompatibel.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Gambar input untuk chat / media (`.NET: InputImage(Data, Mime)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputImage {
    pub data: Vec<u8>,
    pub mime: String,
}

impl InputImage {
    pub fn new(data: Vec<u8>, mime: impl Into<String>) -> Self {
        Self {
            data,
            mime: mime.into(),
        }
    }

    pub fn png(data: Vec<u8>) -> Self {
        Self::new(data, "image/png")
    }
}

/// Request chat — port `ChatRequest` (`Drivers.cs`).
/// Hanya field yang dipakai driver browser yang dibawa; `Cancel` diwakili
/// oleh pembatalan stream (drop receiver), bukan field.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    /// Prompt user (teks penuh).
    pub prompt: String,
    /// Nama model wire (mis. `muse-chat`); sudah di-resolve oleh gateway.
    pub model: String,
    /// Gambar terlampir.
    pub images: Vec<InputImage>,
    /// Bahasa balasan, mis. `"English"` / `"Bahasa Indonesia"`.
    /// Default .NET: `"English"`.
    pub reply_language: String,
    /// ID percakapan stabil (untuk jalur local-tools + hot tab).
    pub conversation_hint: Option<String>,
    /// Riwayat turn `(role, text)` percakapan ini.
    pub turns: Vec<(String, String)>,
    /// Jalur local-tools (agent loop): balasan harus stabil-utuh, bukan delta.
    pub local_tools: bool,
    /// ID batch gambar (dedup upload antar ronde tool).
    pub image_batch_id: Option<String>,
}

impl ChatRequest {
    pub fn new(prompt: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: model.into(),
            reply_language: "English".to_string(),
            ..Default::default()
        }
    }
}

/// Satu event stream chat — bentuk SSE agen (kontrak §7):
/// `{"event":"assistant_delta","text":"..."}` dan `{"event":"done",...}`.
/// Driver hanya menghasilkan dua varian ini; gateway yang membungkusnya
/// menjadi SSE beserta field `done` (`ok`/`status`/`session_id`/`summary`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    /// Potongan balasan asisten (mengalir).
    AssistantDelta(String),
    /// Balasan selesai (lengkap). Untuk jalur local-tools, teks lengkap
    /// dikirim sebagai SATU `AssistantDelta` sebelum `Done`.
    Done,
}

impl ChatEvent {
    /// Nama event wire (`assistant_delta` / `done`).
    pub fn name(&self) -> &'static str {
        match self {
            ChatEvent::AssistantDelta(_) => "assistant_delta",
            ChatEvent::Done => "done",
        }
    }
}

/// Hasil generate media — port `MediaResult` (`Drivers.cs`).
#[derive(Debug, Clone)]
pub struct MediaResult {
    pub data: Vec<u8>,
    pub mime: String,
    /// `"image"` atau `"video"`.
    pub kind: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub revised_prompt: Option<String>,
}

/// Request generate image — port `ImageRequest` (`Drivers.cs`).
#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub prompt: String,
    pub model: String,
    pub size: Option<String>,
    pub n: usize,
    pub references: Vec<InputImage>,
}

impl ImageRequest {
    pub fn new(prompt: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: model.into(),
            size: None,
            n: 1,
            references: Vec::new(),
        }
    }
}

/// Request generate video — port `VideoRequest` (`Drivers.cs`).
#[derive(Debug, Clone)]
pub struct VideoRequest {
    pub prompt: String,
    pub model: String,
    pub size: Option<String>,
    pub duration: Option<u32>,
    pub first_frame: Option<InputImage>,
}

impl VideoRequest {
    pub fn new(prompt: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: model.into(),
            size: None,
            duration: None,
            first_frame: None,
        }
    }
}

/// Hasil renew sesi — port `SessionInfo` (`Drivers.cs`).
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// `true` bila `/api/session` mengembalikan `status == "assigned"`.
    pub ok: bool,
    /// Cookie hasil gabungan (lama + `Set-Cookie` baru).
    pub cookies: HashMap<String, String>,
    /// Unix expiry per nama cookie.
    pub cookie_expires: HashMap<String, i64>,
    /// Meta debug: `session_status`, `vm_id`, `vm_state`, `vm_wake_http`,
    /// `vm_state_after`, `vm_wake_ok` (+ `driver` untuk mock).
    pub meta: HashMap<String, Option<String>>,
}

/// Kapabilitas driver — port `DriverCapabilities` (`Drivers.cs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverCapabilities {
    pub chat: bool,
    pub chat_images: bool,
    pub image: bool,
    pub image_edit: bool,
    pub video: bool,
    pub renew_session: bool,
    pub quota: bool,
}

impl DriverCapabilities {
    pub const NONE: Self = Self {
        chat: false,
        chat_images: false,
        image: false,
        image_edit: false,
        video: false,
        renew_session: false,
        quota: false,
    };

    pub const ALL: Self = Self {
        chat: true,
        chat_images: true,
        image: true,
        image_edit: true,
        video: true,
        renew_session: true,
        quota: true,
    };
}

/// Akun — definisi LOKAL minimal, field sesuai `Store.cs` (`Account`).
/// HANYA field yang dipakai driver; `muse-store` akan menggantikannya
/// dengan definisi penuh saat integrasi (lihat `TRADEOFFS.md`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Account {
    /// `acc_` + 10 hex (kontrak §6).
    pub id: String,
    pub label: String,
    pub enabled: bool,
    /// `active` / `cooling` / `invalid`.
    pub status: String,
    /// Cookie penuh nama → nilai (disimpan PENUH di `accounts.json`).
    pub cookies: HashMap<String, String>,
    /// Unix expiry per nama cookie.
    pub cookie_expires: HashMap<String, i64>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub cooldown_until: Option<String>,
    pub last_error: Option<String>,
    pub success_count: u64,
    pub fail_count: u64,
    /// Meta bebas; driver browser memakai kunci `hot_page` (lihat
    /// `BrowserDriver` .NET: `LoadHot`/`SaveHot`).
    #[serde(default)]
    pub meta: serde_json::Map<String, serde_json::Value>,
}

impl Account {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            enabled: true,
            status: "active".to_string(),
            ..Default::default()
        }
    }
}

/// Cookie hasil refresh sesi (dikembalikan driver ke gateway agar
/// `accounts.json` diperbarui) — lihat `MuseDriver::refreshed_session`.
#[derive(Debug, Clone)]
pub struct SessionRefresh {
    pub account_id: String,
    pub cookies: HashMap<String, String>,
    pub cookie_expires: HashMap<String, i64>,
}
