//! Fixture writer: produces a .NET-shaped data directory from Rust so the
//! output files can be inspected with `python3 -m json.tool` / `sqlite3`
//! and compared field-by-field against the Store.cs definitions.
//!
//! Usage: `cargo run --example write_fixture -- /tmp/muse-store-fixture`

use muse_store::{
    Account, AccountStatus, ChatSession, KeyStore, MediaStore, Preferences, PreferencesStore,
    RequestLog, RequestRecord, SkillStore, TaskStore,
};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/muse-store-fixture"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;

    // accounts.json
    let mut accounts = muse_store::AccountStore::new(root.join("accounts.json"));
    let mut acc = Account::new();
    acc.id = "acc_0123456789".to_string();
    acc.label = "bree-1".to_string();
    acc.status = AccountStatus::Active;
    acc.cookies = {
        let mut m = Map::new();
        m.insert(
            "hatch_sess".to_string(),
            Value::String("abcdefghij1234567890".to_string()),
        );
        m.insert(
            "hatch_native_auth_device".to_string(),
            Value::String("device-token-xyz".to_string()),
        );
        m
    };
    acc.cookie_expires = {
        let mut m = Map::new();
        m.insert(
            "hatch_sess".to_string(),
            Value::Number(1_800_000_000.into()),
        );
        m
    };
    acc.created_at = 1_761_972_245.0;
    acc.success_count = 3;
    accounts.upsert(acc)?;
    println!("wrote {}", root.join("accounts.json").display());

    // keys.json
    let mut keys = KeyStore::new(root.join("keys.json"));
    let (key, plaintext) = keys.create("agent", "fixture key")?;
    println!("wrote {} (plaintext {plaintext}, id {})", root.join("keys.json").display(), key.id);

    // tasks.json
    let mut tasks = TaskStore::new(root.join("tasks.json"));
    let task = tasks.create("image", Map::new())?;
    tasks.set_running(&task.id)?;
    tasks.set_progress(&task.id, 50)?;
    tasks.set_succeeded(&task.id, {
        let mut r = Map::new();
        r.insert("url".into(), json!("https://example.invalid/m.png"));
        r
    })?;
    println!("wrote {}", root.join("tasks.json").display());

    // requests.db
    let log = RequestLog::open(root.join("requests.db"))?;
    log.add(&RequestRecord {
        ts: 1_761_972_245.0,
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        model: Some("muse-chat".into()),
        key_id: Some(key.id.clone()),
        key_name: Some("agent".into()),
        account_id: Some("acc_0123456789".into()),
        status_code: 200,
        latency_ms: 123,
        stream: true,
        poll: false,
        error: None,
        client_ip: Some("127.0.0.1".into()),
        task_id: None,
    })?;
    println!("wrote {}", root.join("requests.db").display());

    // media/
    let media = MediaStore::new(root.join("media"))?;
    let name = media.save(b"\x89PNG\r\n\x1a\n", "image/png", "m")?;
    println!("wrote {} ({name})", root.join("media").display());

    // skills/
    let skills = SkillStore::new(root.join("skills"))?;
    std::fs::create_dir_all(skills.directory())?;
    std::fs::write(
        skills.directory().join("fe.md"),
        "---\nname: Frontend\ndescription: Build UIs\n---\nFollow the design system.\n",
    )?;
    println!("wrote {}", skills.directory().display());

    // agent-preferences.json
    let prefs_store = PreferencesStore::new(root.join("agent-preferences.json"));
    let mut prefs = Preferences::default();
    prefs.turbo = true;
    prefs_store.save(&prefs)?;
    println!("wrote {}", root.join("agent-preferences.json").display());

    // agent-sessions/<sha256>/<id>.json
    let sessions = muse_store::ChatSessionStore::new(root.join("agent-sessions"));
    let mut session = ChatSession::new("/tmp/workspace-demo");
    session.title = "Fixture conversation".to_string();
    sessions.save(&mut session)?;
    println!(
        "wrote {}",
        sessions.file_for("/tmp/workspace-demo", &session.id)?.display()
    );

    println!("fixture dir: {}", root.display());
    Ok(())
}
