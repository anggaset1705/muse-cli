//! Decrypt .NET-produced DPAPI blobs (api_key + accounts.json) on Windows.
//! Reads paths from env: DOTNET_API_KEY_FILE, DOTNET_ACCOUNTS_FILE.
//! NEVER prints credential values: asserts only success, the `m2a-` prefix
//! and the account count. Ignored on non-Windows (no DPAPI there).

use muse_core::dpapi;

#[test]
#[ignore = "Windows-only: needs .NET-produced DPAPI files; run with DOTNET_API_KEY_FILE set"]
fn dotnet_api_key_decrypts() {
    let path = std::env::var("DOTNET_API_KEY_FILE").expect("DOTNET_API_KEY_FILE not set");
    let plain =
        dpapi::read_file(std::path::Path::new(&path)).expect("read_file api_key failed");
    assert!(
        plain.starts_with("m2a-"),
        "api_key did not decrypt to an m2a- value"
    );
    assert_eq!(plain.len(), 40, "unexpected api_key length");
}

#[test]
#[ignore = "Windows-only: needs .NET-produced DPAPI files; run with DOTNET_ACCOUNTS_FILE set"]
fn dotnet_accounts_decrypt() {
    let path =
        std::env::var("DOTNET_ACCOUNTS_FILE").expect("DOTNET_ACCOUNTS_FILE not set");
    let plain =
        dpapi::read_file(std::path::Path::new(&path)).expect("read_file accounts.json failed");
    let v: serde_json::Value =
        serde_json::from_str(&plain).expect("accounts.json is not JSON after decrypt");
    let arr = v.as_array().expect("accounts.json root is not an array");
    assert!(!arr.is_empty(), "accounts.json decrypted to an empty array");
    // Count only — never print account contents.
    println!("accounts decrypted: count={}", arr.len());
}
