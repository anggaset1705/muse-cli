//! `muse-core`: shared core types for the Muse Rust backend.
//!
//! Ported 1:1 from .NET `Muse2Api` — `Models.cs` (`ModelRegistry`, `Prompt`),
//! `Errors.cs` (`MuseException`, `UpstreamException`), `Settings.cs`
//! (`Settings`), `PrivateDataProtection.cs` (DPAPI). See `CONTRACTS.md` §11.
//! This crate is standalone: it depends on no other workspace crate.

pub mod dpapi;
pub mod errors;
pub mod models;
pub mod settings;

mod prompt_templates;

pub use dpapi::{is_encrypted as dpapi_is_encrypted, protect as dpapi_protect, unprotect as dpapi_unprotect};
pub use errors::MuseError;
pub use models::{FlatPrompt, ModelRegistry, ModelSpec, Prompt, ALIASES, MODELS};
pub use settings::{generate_api_key, Settings};
