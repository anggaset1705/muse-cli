//! End-to-end round-trip through the public API: build a whole data dir
//! the way the gateway would, reload every store from disk, and assert the
//! fields match the .NET definitions in Store.cs / Skills.cs /
//! ChatSessions.cs / AgentEndpoints.cs.

use muse_store::{
    Account, AccountStatus, ChatSession, KeyStore, MediaStore, Preferences, PreferencesStore,
    RequestLog, RequestRecord, SkillStore, TaskStore,
};
use serde_json::{Map, Value};
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("muse-store-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn full_data_dir_roundtrip() {
    let root = data_dir();

    // ---- accounts.json ----
    let mut accounts = muse_store::AccountStore::new(root.join("accounts.json"));
    let mut acc = Account::new();
    acc.id = "acc_0123456789".to_string();
    acc.label = "bree-1".to_string();
    acc.cookies = {
        let mut m = Map::new();
        m.insert(
            "hatch_sess".into(),
            Value::String("abcdefghij1234567890".into()),
        );
        m
    };
    accounts.upsert(acc).unwrap();
    let mut accounts2 = muse_store::AccountStore::new(root.join("accounts.json"));
    accounts2.load().unwrap();
    let back = accounts2.get("acc_0123456789").unwrap();
    assert_eq!(back.label, "bree-1");
    assert_eq!(back.status, AccountStatus::Active);
    assert!(back.is_available(1_800_000_000.0));
    // Full cookie value is stored in the file; Public() masks it.
    assert_eq!(back.public()["cookies"]["hatch_sess"], "abcd...7890");

    // ---- keys.json ----
    let mut keys = KeyStore::new(root.join("keys.json"));
    let (key, plaintext) = keys.create("agent", "").unwrap();
    let mut keys2 = KeyStore::new(root.join("keys.json"));
    keys2.load().unwrap();
    assert_eq!(keys2.verify(&plaintext).unwrap().id, key.id);

    // ---- tasks.json ----
    let mut tasks = TaskStore::new(root.join("tasks.json"));
    let task = tasks.create("image", Map::new()).unwrap();
    tasks.set_succeeded(&task.id, Map::new()).unwrap();
    let mut tasks2 = TaskStore::new(root.join("tasks.json"));
    tasks2.load().unwrap();
    assert_eq!(tasks2.get(&task.id).unwrap().status, "succeeded");

    // ---- requests.db ----
    let log = RequestLog::open(root.join("requests.db")).unwrap();
    log.add(&RequestRecord {
        ts: 1_761_972_245.0,
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        model: Some("muse-chat".into()),
        key_id: Some(key.id.clone()),
        key_name: None,
        account_id: Some("acc_0123456789".into()),
        status_code: 200,
        latency_ms: 42,
        stream: false,
        poll: false,
        error: None,
        client_ip: None,
        task_id: None,
    })
    .unwrap();
    drop(log);
    let log2 = RequestLog::open(root.join("requests.db")).unwrap();
    let (rows, total) = log2
        .query(Some(&key.id), None, None, None, None, false, 10, 0)
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].latency_ms, 42);

    // ---- media/ ----
    let media = MediaStore::new(root.join("media")).unwrap();
    let name = media.save(b"fake-png", "image/png", "m").unwrap();
    assert!(media.path_of(&name).is_ok());
    assert_eq!(MediaStore::mime_of(&name), "image/png");

    // ---- skills/ ----
    let skills = SkillStore::new(root.join("skills")).unwrap();
    std::fs::create_dir_all(skills.directory()).unwrap();
    std::fs::write(
        skills.directory().join("fe.md"),
        "---\nname: Frontend\n---\nBuild it.\n",
    )
    .unwrap();
    let skill = skills.get("fe").unwrap();
    assert_eq!(skill.name, "Frontend");
    let (block, applied) = skills.expand("use @fe");
    assert_eq!(applied, vec!["fe"]);
    assert!(block.contains("### Skill @fe — Frontend"));

    // ---- agent-preferences.json ----
    let prefs_store = PreferencesStore::new(root.join("agent-preferences.json"));
    let mut prefs = Preferences::default();
    prefs.reply_language = "Bahasa Indonesia".to_string();
    prefs_store.save(&prefs).unwrap();
    let back = prefs_store.load();
    assert_eq!(back.reply_language, "Bahasa Indonesia");
    assert_eq!(back.view()["ui_language"], "English");

    // ---- agent-sessions/ ----
    let sessions = muse_store::ChatSessionStore::new(root.join("agent-sessions"));
    let ws = root.join("ws").to_string_lossy().into_owned();
    let mut session = ChatSession::new(&ws);
    session.title = "E2E".to_string();
    sessions.save(&mut session).unwrap();
    let loaded = sessions.load(&ws, &session.id).unwrap();
    assert_eq!(loaded.title, "E2E");
    assert_eq!(sessions.list(&ws).len(), 1);
}
