//! UTC date helpers without a date-time dependency.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Civil date for a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = (if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    }) as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Day count since 1970-01-01 for a civil date (inverse of [`civil_from_days`]).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `yyyy-MM-ddTHH:mm:ssZ`, the format `latest.json` has always used for `pub_date`.
pub fn utc_timestamp(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60
    )
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Parses `YYYY-MM-DD`, rejecting impossible dates such as 2026-02-30.
pub fn parse_date(text: &str) -> Option<(i64, u32, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = &text[range];
        if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let year = i64::from(digits(0..4)?);
    let month = digits(5..7)?;
    let day = digits(8..10)?;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => return None,
    };
    (1..=days_in_month)
        .contains(&day)
        .then_some((year, month, day))
}

/// Seconds since the epoch for `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM[:SS[.fraction]][Z|+HH:MM|-HH:MM]`.
/// A missing zone means UTC. Strict ISO 8601 is all the manifests and the RustSec
/// contract ever carry, so looser spellings are rejected on purpose.
pub fn parse_iso_seconds(text: &str) -> Option<i64> {
    let (date, rest) = match text.split_once(['T', ' ']) {
        Some((date, rest)) => (date, Some(rest)),
        None => (text, None),
    };
    let (year, month, day) = parse_date(date)?;
    let mut seconds = days_from_civil(year, month, day) * 86_400;
    let Some(rest) = rest else {
        return Some(seconds);
    };
    let zone_start = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(zone_start);
    let mut parts = time.split(':');
    let hour: i64 = parts.next()?.parse().ok()?;
    let minute: i64 = parts.next()?.parse().ok()?;
    let second_text = parts.next().unwrap_or("0");
    let whole = second_text.split('.').next()?;
    let second: i64 = whole.parse().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    seconds += hour * 3_600 + minute * 60 + second;
    match zone {
        "" | "Z" => {}
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (zone_hour, zone_minute) = zone[1..].split_once(':')?;
            let offset =
                zone_hour.parse::<i64>().ok()? * 3_600 + zone_minute.parse::<i64>().ok()? * 60;
            seconds -= sign * offset;
        }
    }
    Some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamp_formats_known_instants() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn civil_conversions_round_trip() {
        for days in [-1, 0, 1, 59, 60, 11_016, 19_000, 25_000] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }

    #[test]
    fn parse_date_rejects_impossible_dates() {
        assert_eq!(parse_date("2026-11-07"), Some((2026, 11, 7)));
        assert_eq!(parse_date("2028-02-29"), Some((2028, 2, 29)));
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-4-1",
            "26-04-01",
            "2026-04-31",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn parse_iso_seconds_handles_dates_zones_and_garbage() {
        assert_eq!(parse_iso_seconds("1970-01-02"), Some(86_400));
        assert_eq!(
            parse_iso_seconds("2026-07-06T00:00:00Z"),
            parse_iso_seconds("2026-07-06")
        );
        assert_eq!(
            parse_iso_seconds("2026-07-06T02:00:00+02:00"),
            parse_iso_seconds("2026-07-06T00:00:00Z")
        );
        assert_eq!(
            parse_iso_seconds("2026-07-06T00:00:00.123456-05:00"),
            parse_iso_seconds("2026-07-06T05:00:00Z")
        );
        assert_eq!(parse_iso_seconds("yesterday"), None);
        assert_eq!(parse_iso_seconds("2026-07-06T25:00:00Z"), None);
    }
}
