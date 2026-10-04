//! The wall clock, which only the runtime reads (`update` gets times in
//! messages): the command log's `HH:MM:SS` and a history row's local time.
//! Local time comes from `localtime_r` on Unix; Windows shows UTC.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the epoch, now.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The local zone's offset from UTC at `secs`, in seconds.
#[cfg(unix)]
fn offset_at(secs: i64) -> i64 {
    let t = secs as libc::time_t;
    // SAFETY: `localtime_r` writes into the `tm` we own and reads `t`; both
    // outlive the call.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff
    }
}

#[cfg(not(unix))]
fn offset_at(_secs: i64) -> i64 {
    0
}

/// `HH:MM:SS` of `secs` shifted by `offset`.
fn clock_text(secs: i64, offset: i64) -> String {
    let day = (secs + offset).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, day / 60 % 60, day % 60)
}

/// The command log's time: local `HH:MM:SS` now.
pub fn wall_clock() -> String {
    let now = now_secs();
    clock_text(now, offset_at(now))
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since the epoch of an ISO 8601 UTC time as Seaquel stores it
/// (`2026-10-03T12:03:51.123Z`; fractions and a missing `Z` allowed).
pub fn parse_iso(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { text.get(from..to)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// A stored time as a history row shows it: `HH:MM:SS` when it's today
/// (local), else `MM-DD HH:MM`. Pure: `now` and `offset` are given.
pub fn when_text(secs: i64, now: i64, offset: i64) -> String {
    let local = secs + offset;
    let today = (now + offset).div_euclid(86_400) == local.div_euclid(86_400);
    if today {
        return clock_text(secs, offset);
    }
    let days = local.div_euclid(86_400);
    // civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let rem = local.rem_euclid(86_400);
    format!("{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem / 60 % 60)
}

/// A stored history time, shown in local time; the text as stored when it
/// doesn't parse.
pub fn local_when(stored: &str) -> String {
    match parse_iso(stored) {
        Some(secs) => when_text(secs, now_secs(), offset_at(secs)),
        None => stored.chars().take(16).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times_parse() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-10-03T12:03:51.123Z"), Some(1_791_029_031));
        assert_eq!(parse_iso("2000-02-29 23:59:59"), Some(951_868_799));
        assert_eq!(parse_iso("yesterday"), None);
        assert_eq!(parse_iso("2026-13-03T12:03:51Z"), None);
    }

    #[test]
    fn today_shows_the_time_and_another_day_the_date() {
        let t = parse_iso("2026-10-03T12:03:51Z").unwrap();
        assert_eq!(when_text(t, t + 60, 0), "12:03:51");
        assert_eq!(when_text(t, t + 60, 2 * 3600), "14:03:51");
        assert_eq!(when_text(t, t + 86_400, 0), "10-03 12:03");
        // Local midnight decides "today".
        assert_eq!(when_text(t, t + 12 * 3600, 0), "10-03 12:03");
        assert_eq!(when_text(t, t + 11 * 3600, 0), "12:03:51");
    }

    #[test]
    fn the_wall_clock_is_hh_mm_ss() {
        let text = wall_clock();
        assert_eq!(text.len(), 8, "{text}");
        assert_eq!(&text[2..3], ":");
        assert_eq!(local_when("not a time at all, really"), "not a time at al");
    }
}
