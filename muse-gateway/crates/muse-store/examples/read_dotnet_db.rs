//! Cross-compat check: open a requests.db written by the .NET gateway and
//! read it back through the Rust `RequestLog`.
//!
//! Usage: `cargo run --example read_dotnet_db -- /path/to/requests.db`
//! (copy the live DB first — opening it in WAL mode may create -wal/-shm
//! files next to it).

use muse_store::RequestLog;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: read_dotnet_db <requests.db>");
    let log = RequestLog::open(&path)?;
    let (rows, total) = log.query(None, None, None, None, None, false, 10, 0)?;
    println!("total rows: {total}");
    for r in &rows {
        println!(
            "ts={} {} {} -> {} ({} ms, key={:?}, account={:?})",
            r.ts, r.method, r.path, r.status_code, r.latency_ms, r.key_id, r.account_id
        );
    }
    println!("stats(24h): {}", log.stats("24h")?);
    println!("key_usage: {}", log.key_usage()?);
    Ok(())
}
