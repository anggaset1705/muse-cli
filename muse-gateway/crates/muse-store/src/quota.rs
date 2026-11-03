//! Quota waterfall (3 Nov 2026, disetujui Bree): pemilihan akun 3 fase.
//!
//! * Fase 1 — kuota mingguan: akun yang mingguan-nya masih tersisa.
//! * Fase 2 — extra kuota (token tambahan): mingguan semua akun habis,
//!   pakai extra akun 1, habis, baru extra akun 2, dst.
//! * Fase 3 — out of credit: semua akun habis mingguan + extra.
//!
//! Klasifikasi memakai snapshot kuota terakhir yang diketahui
//! (`meta["quota"]` tiap akun, ditulis `GET /admin/accounts/{id}/quota`).
//! Acquire TIDAK PERNAH memicu baca kuota live — baca live menggerakkan
//! tab browser agen (s.d. ~12 dtk, bisa menandai tab stale), jadi snapshot
//! hanya diperbarui lewat pemicu manual yang sudah ada (widget Penggunaan:
//! sekali saat backend healthy + tombol Refresh; tombol Quota di Settings).
//!
//! Batas jujur: gateway TIDAK bisa memaksa sisi Muse memakai mingguan vs
//! token di DALAM satu akun — yang dikendalikan hanya akun mana yang
//! mendapat request. Waterfall ini bekerja di level pemilihan akun; jalur
//! reaktif (429 / petunjuk kuota → cooldown) tetap jadi jaring pengaman.

use chrono::{DateTime, Datelike, NaiveDate, Weekday};
use serde_json::{Map, Value};

/// "Habis" mingguan = `weekly_used_pct` sudah mencapai 100%.
pub const WEEKLY_EXHAUSTED_PCT: f64 = 100.0;

/// Snapshot mingguan-habis tanpa tanggal reset yang bisa diparse hanya
/// dipercaya selama ini; sesudahnya snapshot dianggap basi (siklus mingguan
/// hampir pasti sudah berganti) dan akun kembali diperlakukan optimistis
/// (fase mingguan + jaring reaktif).
pub const SNAPSHOT_STALE_SECS: f64 = 7.0 * 86400.0;

/// Fase waterfall satu akun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaPhase {
    /// Mingguan masih tersisa (atau tidak diketahui — optimistis).
    Weekly,
    /// Mingguan habis, tapi token tambahan masih tersisa.
    Extra,
    /// Mingguan habis dan tidak ada token tambahan.
    Exhausted,
}

/// Snapshot kuota terakhir yang diketahui, apa adanya dari
/// `meta["quota"]` (ditulis endpoint quota; lihat `UsageInfo::to_map` di
/// muse-browser).
#[derive(Debug, Clone, Default)]
pub struct QuotaSnapshot {
    pub weekly_used_pct: Option<f64>,
    pub resets_at: Option<String>,
    pub extra_left: Option<String>,
    pub checked_at: Option<f64>,
    pub found: bool,
}

impl QuotaSnapshot {
    pub fn from_meta(meta: &Map<String, Value>) -> Option<QuotaSnapshot> {
        let q = meta.get("quota")?.as_object()?;
        Some(QuotaSnapshot {
            weekly_used_pct: q.get("weekly_used_pct").and_then(|v| v.as_f64()),
            resets_at: q
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            extra_left: q
                .get("extra_left")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            checked_at: q
                .get("checked_at")
                .and_then(|v| v.as_f64())
                .or_else(|| q.get("fetched_at").and_then(|v| v.as_f64())),
            found: q.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }
}

/// Parse `extra_left` ("sisa 31 M token", "1B tokens left", "5,000 left")
/// menjadi jumlah token pembanding. Angka + akhiran besaran (B/M/K, EN/ID);
/// tanpa akhiran, koma/titik dianggap pemisah ribuan ("5,000" = 5000).
/// Dengan akhiran, koma dianggap desimal Indonesia ("1,5 M" = 1,5 juta).
/// `None` bila tidak ada angka sama sekali (teks tanpa angka = tidak
/// diketahui, BUKAN nol — pemanggil yang memutuskan).
pub fn parse_extra_tokens(text: &str) -> Option<f64> {
    let t = text.trim();
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() && !bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let start = i;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b',' || bytes[i] == b'.') {
        i += 1;
    }
    let num_raw = &t[start..i];
    let rest = t[i..].trim_start().to_ascii_lowercase();
    let mult = if rest.starts_with('b')
        || rest.starts_with("miliar")
        || rest.starts_with("billion")
    {
        1e9
    } else if rest.starts_with('m') || rest.starts_with("juta") || rest.starts_with("million") {
        1e6
    } else if rest.starts_with('k') || rest.starts_with("ribu") || rest.starts_with("thousand") {
        1e3
    } else {
        1.0
    };
    let v = if mult != 1.0 {
        num_raw.replace(',', ".").parse::<f64>().ok()?
    } else {
        num_raw.replace([',', '.'], "").parse::<f64>().ok()?
    };
    Some(v * mult)
}

/// Token tambahan yang tersisa menurut snapshot; `None` di `extra_left`
/// (dialog tidak menampilkan seksi token tambahan) = tidak ada extra.
pub fn extra_tokens(snapshot: Option<&QuotaSnapshot>) -> f64 {
    snapshot
        .and_then(|s| s.extra_left.as_deref())
        .and_then(parse_extra_tokens)
        .unwrap_or(0.0)
}

fn month_number(token: &str) -> Option<u32> {
    match token {
        "jan" | "january" | "januari" => Some(1),
        "feb" | "february" | "februari" => Some(2),
        "mar" | "march" | "maret" => Some(3),
        "apr" | "april" => Some(4),
        "may" | "mei" => Some(5),
        "jun" | "june" | "juni" => Some(6),
        "jul" | "july" | "juli" => Some(7),
        "aug" | "august" | "agu" | "agustus" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" | "okt" | "oktober" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" | "des" | "desember" => Some(12),
        _ => None,
    }
}

fn weekday_number(token: &str) -> Option<Weekday> {
    match token {
        "monday" | "senin" => Some(Weekday::Mon),
        "tuesday" | "selasa" => Some(Weekday::Tue),
        "wednesday" | "rabu" => Some(Weekday::Wed),
        "thursday" | "kamis" => Some(Weekday::Thu),
        "friday" | "jumat" | "jum'at" => Some(Weekday::Fri),
        "saturday" | "sabtu" => Some(Weekday::Sat),
        "sunday" | "minggu" => Some(Weekday::Sun),
        _ => None,
    }
}

/// Parse `resets_at` hasil parse dialog ("9 Nov", "9 November", "Nov 9",
/// "Monday", "Senin") menjadi Unix detik (tengah malam UTC di tanggal itu).
///
/// * Tanpa tahun: dialog selalu menyebut reset BERIKUTNYA — bulan-tanggal
///   yang sudah lewat tahun ini digeser ke tahun depan.
/// * Nama hari: kemunculan berikutnya SETELAH `now` (hari yang sama = minggu
///   depan, karena reset hari ini sudah lewat).
/// * Tengah malam UTC adalah perkiraan: tanggal dialog memakai zona waktu
///   akun (Bree: Asia/Jakarta), jadi batas eksklusi bisa meleset < 1 hari —
///   jaring reaktif (429 → cooldown) menutup selisih itu.
/// * `None` bila format tidak dikenali — pemanggil memakai fallback
///   (keluarkan sampai snapshot berikutnya).
pub fn parse_reset_time(text: &str, now: f64) -> Option<f64> {
    let clean = text
        .trim()
        .trim_end_matches(['.', ','])
        .trim()
        .to_ascii_lowercase();
    if clean.is_empty() {
        return None;
    }
    let now_dt = DateTime::from_timestamp(now as i64, 0)?;
    let today = now_dt.date_naive();
    if let Some(wd) = weekday_number(&clean) {
        let mut days =
            (wd.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64)
                .rem_euclid(7);
        if days == 0 {
            days = 7;
        }
        let date = today + chrono::Duration::days(days);
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as f64);
    }
    let tokens: Vec<&str> = clean
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
        .filter(|t| !t.is_empty())
        .collect();
    // "9 Nov" / "9 November" / "Nov 9", dengan/tanpa tahun. Catatan: kedua
    // urutan harus dicoba eksplisit — `?` di dalam arm match pertama akan
    // menggagalkan seluruh fungsi sebelum arm urutan kedua sempat dicoba.
    let (day, month, year) = match tokens.as_slice() {
        [a, b] => {
            if let (Ok(d), Some(m)) = (a.parse::<u32>(), month_number(b)) {
                (d, m, None)
            } else if let (Some(m), Ok(d)) = (month_number(a), b.parse::<u32>()) {
                (d, m, None)
            } else {
                return None;
            }
        }
        [a, b, c] => {
            if let (Ok(d), Some(m), Ok(y)) =
                (a.parse::<u32>(), month_number(b), c.parse::<i32>())
            {
                (d, m, Some(y))
            } else if let (Some(m), Ok(d), Ok(y)) =
                (month_number(a), b.parse::<u32>(), c.parse::<i32>())
            {
                (d, m, Some(y))
            } else {
                return None;
            }
        }
        _ => return None,
    };
    let year = match year {
        Some(y) => y,
        None => {
            let mut y = today.year();
            if let Some(d) = NaiveDate::from_ymd_opt(y, month, day) {
                if d < today {
                    y += 1;
                }
            }
            y
        }
    };
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as f64)
}

/// Klasifikasi satu akun ke fase waterfall dari snapshot terakhirnya.
///
/// * Tanpa snapshot / parse gagal / `weekly_used_pct` tidak ada → `Weekly`
///   (optimistis; jaring reaktif tetap berjalan).
/// * Mingguan < 100% → `Weekly`.
/// * Mingguan >= 100% dan tanggal reset sudah lewat → `Weekly` (mingguan
///   sudah berganti).
/// * Mingguan >= 100%, reset di masa depan → `Extra` bila token tambahan
///   tersisa, else `Exhausted`.
/// * Mingguan >= 100%, reset tidak bisa diparse → percaya snapshot hanya
///   selagi segar (< 7 hari); sesudahnya kembali optimistis (`Weekly`).
pub fn classify(snapshot: Option<&QuotaSnapshot>, now: f64) -> QuotaPhase {
    let Some(s) = snapshot else {
        return QuotaPhase::Weekly;
    };
    if !s.found {
        return QuotaPhase::Weekly;
    }
    let Some(pct) = s.weekly_used_pct else {
        return QuotaPhase::Weekly;
    };
    if pct < WEEKLY_EXHAUSTED_PCT {
        return QuotaPhase::Weekly;
    }
    match s.resets_at.as_deref().and_then(|r| parse_reset_time(r, now)) {
        Some(reset) if reset <= now => QuotaPhase::Weekly,
        Some(_) => {
            if extra_tokens(Some(s)) > 0.0 {
                QuotaPhase::Extra
            } else {
                QuotaPhase::Exhausted
            }
        }
        None => {
            let stale = match s.checked_at {
                Some(at) => now - at > SNAPSHOT_STALE_SECS,
                None => false,
            };
            if stale {
                QuotaPhase::Weekly
            } else if extra_tokens(Some(s)) > 0.0 {
                QuotaPhase::Extra
            } else {
                QuotaPhase::Exhausted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    /// Selasa 2026-11-03 12:00 UTC (tanggal nyata saat fitur ini dibangun).
    fn now() -> f64 {
        Utc.with_ymd_and_hms(2026, 11, 3, 12, 0, 0)
            .unwrap()
            .timestamp() as f64
    }

    fn snap(weekly: f64, resets: &str, extra: &str) -> QuotaSnapshot {
        QuotaSnapshot {
            weekly_used_pct: Some(weekly),
            resets_at: Some(resets.to_string()),
            extra_left: Some(extra.to_string()),
            checked_at: Some(now()),
            found: true,
        }
    }

    #[test]
    fn parse_extra_tokens_formats() {
        assert_eq!(parse_extra_tokens("sisa 31 M token"), Some(31_000_000.0));
        assert_eq!(parse_extra_tokens("1B tokens left"), Some(1_000_000_000.0));
        assert_eq!(parse_extra_tokens("5,000 left"), Some(5_000.0));
        assert_eq!(parse_extra_tokens("0 left"), Some(0.0));
        assert_eq!(parse_extra_tokens("1,5 M"), Some(1_500_000.0));
        assert_eq!(parse_extra_tokens("2 K"), Some(2_000.0));
        assert_eq!(parse_extra_tokens("tidak ada"), None);
        assert_eq!(parse_extra_tokens(""), None);
    }

    #[test]
    fn parse_reset_time_dates() {
        let day = |y: i32, m: u32, d: u32| {
            Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap().timestamp() as f64
        };
        assert_eq!(parse_reset_time("9 Nov", now()), Some(day(2026, 11, 9)));
        assert_eq!(
            parse_reset_time("9 November", now()),
            Some(day(2026, 11, 9))
        );
        assert_eq!(parse_reset_time("Nov 9", now()), Some(day(2026, 11, 9)));
        assert_eq!(
            parse_reset_time("9 Nov 2026", now()),
            Some(day(2026, 11, 9))
        );
        // "1 Nov" sudah lewat (hari ini 3 Nov) -> tahun depan.
        assert_eq!(parse_reset_time("1 Nov", now()), Some(day(2027, 11, 1)));
        assert_eq!(parse_reset_time("segera", now()), None);
        assert_eq!(parse_reset_time("", now()), None);
    }

    #[test]
    fn parse_reset_time_weekdays() {
        let day = |y: i32, m: u32, d: u32| {
            Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap().timestamp() as f64
        };
        // Hari ini Selasa 3 Nov 2026; Senin berikut = 9 Nov.
        assert_eq!(parse_reset_time("Monday", now()), Some(day(2026, 11, 9)));
        assert_eq!(parse_reset_time("Senin", now()), Some(day(2026, 11, 9)));
        // Hari yang sama (Selasa) = minggu depan, bukan hari ini.
        assert_eq!(parse_reset_time("Tuesday", now()), Some(day(2026, 11, 10)));
    }

    #[test]
    fn classify_phases() {
        let n = now();
        assert_eq!(classify(None, n), QuotaPhase::Weekly);
        assert_eq!(
            classify(Some(&snap(48.0, "9 Nov", "sisa 31 M token")), n),
            QuotaPhase::Weekly
        );
        assert_eq!(
            classify(Some(&snap(100.0, "9 Nov", "sisa 31 M token")), n),
            QuotaPhase::Extra
        );
        assert_eq!(
            classify(Some(&snap(100.0, "9 Nov", "1B tokens left")), n),
            QuotaPhase::Extra
        );
        assert_eq!(
            classify(Some(&snap(100.0, "9 Nov", "0 left")), n),
            QuotaPhase::Exhausted
        );
        // Reset sudah lewat (1 Nov < 3 Nov, digeser ke 2027 -> masa depan):
        // pakai tanggal yang benar-benar lampau dengan tahun eksplisit.
        assert_eq!(
            classify(Some(&snap(100.0, "1 Nov 2026", "0 left")), n),
            QuotaPhase::Weekly
        );
        // Snapshot basi (> 7 hari) + reset tak terparse -> optimistis.
        let mut stale = snap(100.0, "segera", "0 left");
        stale.checked_at = Some(n - 8.0 * 86400.0);
        assert_eq!(classify(Some(&stale), n), QuotaPhase::Weekly);
        // Snapshot segar + reset tak terparse + extra ada -> Extra.
        assert_eq!(
            classify(Some(&snap(100.0, "segera", "sisa 31 M token")), n),
            QuotaPhase::Extra
        );
        // Snapshot segar + reset tak terparse + tanpa extra -> Exhausted.
        assert_eq!(
            classify(Some(&snap(100.0, "segera", "0 left")), n),
            QuotaPhase::Exhausted
        );
    }

    #[test]
    fn snapshot_of_reads_quota_meta() {
        let mut meta = Map::new();
        let mut q = Map::new();
        q.insert(
            "weekly_used_pct".to_string(),
            Value::from(48.0),
        );
        q.insert(
            "resets_at".to_string(),
            Value::String("9 Nov".to_string()),
        );
        q.insert(
            "extra_left".to_string(),
            Value::String("sisa 31 M token".to_string()),
        );
        q.insert("found".to_string(), Value::Bool(true));
        q.insert("checked_at".to_string(), Value::from(now()));
        meta.insert("quota".to_string(), Value::Object(q));
        let s = QuotaSnapshot::from_meta(&meta).unwrap();
        assert_eq!(s.weekly_used_pct, Some(48.0));
        assert_eq!(s.extra_left.as_deref(), Some("sisa 31 M token"));
        assert_eq!(classify(Some(&s), now()), QuotaPhase::Weekly);
        assert!(QuotaSnapshot::from_meta(&Map::new()).is_none());
    }
}
