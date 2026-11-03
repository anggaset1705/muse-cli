//! `muse-store` — persistence layer of the Muse Rust backend.
//!
//! A 1:1 port of the .NET `Muse2Api` storage code (`Store.cs`,
//! `Muse2Api.Agent/Skills.cs`, `ChatSessions.cs`, and the preferences half
//! of `AgentEndpoints.cs`), keeping the same file formats, field meanings
//! and state transitions so data written by the .NET gateway stays readable
//! and vice versa. See `CONTRACTS.md` §6 and §11.
//!
//! # Format note (read this before changing serialization)
//!
//! The .NET code serializes with default `System.Text.Json` options —
//! PascalCase property names and numeric enums (`"Id"`, `"Status": 0`)
//! — and the coordinator's correction (CONTRACTS.md §6, 2 Nov 2026)
//! requires Rust to WRITE that form too, so files stay readable by .NET.
//! This crate writes PascalCase + numeric enums and reads both spellings
//! (snake_case aliases + string-or-number enums), so older Rust-written
//! files still load.
//!
//! # What is NOT in this crate (deliberately)
//!
//! * `AccountPool.AcquireAsync`'s timed wait loop and the `Lease` RAII
//!   guard — async, belongs to the gateway. `AccountPool::acquire` /
//!   `::release` are the synchronous building blocks.
//! * `TaskManager`'s async runner (`Submit`/`Run`/`StopAsync`) — execution
//!   belongs to `muse-agent`; [`tasks::TaskStore`] keeps the records.
//! * DPAPI encryption of `accounts.json` on Windows — belongs to
//!   `muse-core` per CONTRACTS.md §11. See [`protect::Protector`]; the
//!   default is the plaintext protector (the non-Windows .NET behaviour).

pub mod account;
pub mod chat_sessions;
pub mod error;
pub mod json;
pub mod keys;
pub mod media;
pub mod preferences;
pub mod protect;
pub mod quota;
pub mod request_log;
pub mod skills;
pub mod tasks;

pub use account::{Account, AccountPool, AccountStatus, AccountStore, ReleaseOutcome};
pub use chat_sessions::{AgentEvent, ChatSession, ChatSessionStore, GoalRun};
pub use error::StoreError;
pub use quota::{QuotaPhase, QuotaSnapshot};
pub use keys::{ApiKey, KeyStore};
pub use media::MediaStore;
pub use preferences::{Preferences, PreferencesStore};
pub use request_log::{RequestLog, RequestRecord};
pub use skills::{SkillDefinition, SkillStore};
pub use tasks::{BgTask, TaskStore};
