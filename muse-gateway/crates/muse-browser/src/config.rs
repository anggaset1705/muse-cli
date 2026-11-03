//! Pengaturan driver — pemilik kanonisnya `muse-core::Settings`
//! (CONTRACTS.md §11; integrasi 2 Nov 2026 menghapus struct
//! `DriverSettings` lokal yang menduplikasi `Settings`).
//!
//! Modul ini hanya menyimpan alias nama lama (dipakai `BrowserDriver`,
//! `DriverFactory`, dan `BrowserConfig`) plus dua helper kecil yang
//! spesifik driver. Semua nilai tetap dibaca dari env `MUSE2API_*`
//! dengan default .NET yang sama, lewat `muse_core::Settings::load()`.

/// Alias kompatibilitas: pengaturan driver = `muse-core::Settings`.
/// Bukan definisi kedua — satu pemilik kanonik di `muse-core`.
pub type DriverSettings = muse_core::Settings;

/// `Settings.ProfileDir`: `BROWSER_PROFILE_DIR` atau
/// `<data>/browser-profile` (sama seperti `Settings::profile_dir` di
/// `muse-core`; dipertahankan sebagai fungsi bebas agar pemanggil lama
/// yang memakai nama ini tetap jalan).
pub fn profile_dir(settings: &muse_core::Settings) -> std::path::PathBuf {
    settings.profile_dir()
}

/// URL thread baru: `<site>/thread/new` (`Upstream.NewThreadUrl`).
pub fn new_thread_url(settings: &muse_core::Settings) -> String {
    format!("{}/thread/new", settings.site_url.trim_end_matches('/'))
}
