//! Runtime log for the login child process (`muse-gateway login`).
//!
//! Lines go to **stderr**. The Tauri app redirects the login child's
//! stdout + stderr to `<data_dir>/login.log` (see `start_login` in
//! `muse-tauri/src-tauri/src/lib.rs`), so these lines land in that file
//! and survive the window closing — readable after a failed login, next
//! to `accounts.json` via "Buka folder data". Run by hand
//! (`muse-gateway login`), the same lines go to the terminal.
//!
//! What may be logged (hard rule): stage names, statuses, counts and
//! lengths only — **never cookie/token values**. The page host IS logged
//! on the waiting-redirect line: it is a domain (e.g.
//! `login.microsoftonline.com`), not a credential, and it is the one
//! diagnostic fact that tells which redirect the sign-in is stuck on.
//! Error details are capped to one 200-char line (`short()`).
//!
//! Format: `[login 2026-11-03T11:31:00Z] <stage>: <detail>`.
//! Std-only on purpose (no chrono dependency in this crate); the UTC
//! timestamp is formatted from Unix seconds by hand.

use std::time::{SystemTime, UNIX_EPOCH};

/// Format Unix seconds as UTC `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Civil-from-days algorithm (Howard Hinnant's `civil_from_days`);
/// verified against `date -u` for 0, 1762166400 and 1790000000.
pub fn format_unix_ts(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (hour, min, sec) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Current UTC timestamp in the log format; `1970-01-01T00:00:00Z` if
/// the system clock is unavailable (never panics).
pub fn now_ts() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_unix_ts(secs)
}

/// Cap `s` to one log line of at most 200 chars: newlines become spaces,
/// longer text is cut with a trailing `…`. Use for error details, whose
/// source text can carry a gateway body snippet.
pub fn short(s: &str) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    let mut out: String = one_line.chars().take(200).collect();
    if one_line.chars().count() > 200 {
        out.push('…');
    }
    out
}

/// Write one log line to stderr: `[login <utc>] <stage>: <detail>`.
/// `detail` must already be free of secrets (counts/statuses only).
pub fn log_stage(stage: &str, detail: &str) {
    eprintln!("[login {}] {stage}: {detail}", now_ts());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_unix_ts_matches_date_u() {
        // Vectors verified with `date -u -d @<secs>` on the VM.
        assert_eq!(format_unix_ts(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_ts(1_762_166_400), "2025-11-03T10:40:00Z");
        assert_eq!(format_unix_ts(1_790_000_000), "2026-09-21T14:13:20Z");
    }

    #[test]
    fn short_collapses_newlines_and_caps_length() {
        assert_eq!(short("a\nb\rc"), "a b c");
        let long = "x".repeat(250);
        let cut = short(&long);
        assert_eq!(cut.chars().count(), 201); // 200 chars + …
        assert!(cut.ends_with('…'));
        assert_eq!(short("ok"), "ok");
    }
}
