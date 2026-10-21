//! Parsing for the ISO-8601 timestamps Komga sends.
//!
//! Komga writes release dates as `YYYY-MM-DD` and every other timestamp as
//! `YYYY-MM-DDTHH:MM:SSZ`. This also accepts fractional seconds and numeric offsets so a
//! server that formats them differently still works. Ito wants seconds since the Unix
//! epoch, which is all this module produces.

/// Seconds since the Unix epoch for an ISO-8601 date or date-time, or `None` if `text` is neither.
///
/// A date without a time is midnight UTC. A time without an offset is read as UTC.
pub fn epoch_seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    let (date, time) = match text.split_once(['T', 't']) {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let (year, month, day) = parse_date(date)?;
    let mut seconds = days_from_civil(year, month, day) * 86_400;
    let mut fraction = 0.0;
    if let Some(time) = time {
        let (clock, offset) = split_offset(time)?;
        let clock = parse_clock(clock)?;
        seconds += clock.hours * 3_600 + clock.minutes * 60 + clock.seconds - offset;
        fraction = clock.fraction;
    }
    Some(seconds as f64 + fraction)
}

struct Clock {
    hours: i64,
    minutes: i64,
    seconds: i64,
    fraction: f64,
}

fn parse_date(date: &str) -> Option<(i64, i64, i64)> {
    let mut parts = date.split('-');
    let year = parse_digits(parts.next()?, 4, 4)?;
    let month = parse_digits(parts.next()?, 1, 2)?;
    let day = parse_digits(parts.next()?, 1, 2)?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    if !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// Splits a time of day from its UTC offset, returning the offset in seconds.
fn split_offset(time: &str) -> Option<(&str, i64)> {
    if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    // A clock never contains `+` or `-`, so the first one starts the offset.
    let Some(at) = time.find(['+', '-']) else {
        return Some((time, 0));
    };
    let (clock, offset) = time.split_at(at);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let digits = offset.get(1..)?;
    if !digits.is_ascii() {
        return None;
    }
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (parse_digits(hours, 2, 2)?, parse_digits(minutes, 2, 2)?),
        None if digits.len() == 4 => (
            parse_digits(&digits[..2], 2, 2)?,
            parse_digits(&digits[2..], 2, 2)?,
        ),
        None if digits.len() == 2 => (parse_digits(digits, 2, 2)?, 0),
        None => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((clock, sign * (hours * 3_600 + minutes * 60)))
}

fn parse_clock(clock: &str) -> Option<Clock> {
    let (whole, fraction) = match clock.split_once(['.', ',']) {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (clock, None),
    };
    let mut parts = whole.split(':');
    let hours = parse_digits(parts.next()?, 2, 2)?;
    let minutes = parse_digits(parts.next()?, 2, 2)?;
    let seconds = match parts.next() {
        Some(seconds) => parse_digits(seconds, 2, 2)?,
        None => 0,
    };
    // 60 is a leap second.
    if parts.next().is_some() || hours > 23 || minutes > 59 || seconds > 60 {
        return None;
    }
    let fraction = match fraction {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            format!("0.{digits}").parse().ok()?
        }
        Some(_) => return None,
        None => 0.0,
    };
    Some(Clock {
        hours,
        minutes,
        seconds,
        fraction,
    })
}

fn parse_digits(text: &str, min_len: usize, max_len: usize) -> Option<i64> {
    let valid =
        (min_len..=max_len).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
    if valid { text.parse().ok() } else { None }
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the given proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_timestamp_shapes_komga_sends() {
        assert_eq!(epoch_seconds("2026-10-21T06:11:15Z"), Some(1_792_563_075.0));
        assert_eq!(epoch_seconds("2020-01-05"), Some(1_578_182_400.0));
    }

    #[test]
    fn dates_are_midnight_utc_and_times_without_offset_are_utc() {
        assert_eq!(epoch_seconds("1970-01-01"), Some(0.0));
        assert_eq!(epoch_seconds("1970-01-01T00:00:00"), Some(0.0));
    }

    #[test]
    fn applies_numeric_offsets() {
        assert_eq!(
            epoch_seconds("2026-10-21T08:11:15+02:00"),
            Some(1_792_563_075.0)
        );
        assert_eq!(
            epoch_seconds("2026-10-21T01:11:15-05:00"),
            Some(1_792_563_075.0)
        );
        assert_eq!(
            epoch_seconds("2026-10-21T08:11:15+0200"),
            Some(1_792_563_075.0)
        );
        assert_eq!(
            epoch_seconds("2026-10-21T08:11:15+02"),
            Some(1_792_563_075.0)
        );
    }

    #[test]
    fn keeps_fractional_seconds() {
        assert_eq!(
            epoch_seconds("2026-10-21T06:11:15.500Z"),
            Some(1_792_563_075.5)
        );
    }

    #[test]
    fn handles_leap_days_and_the_edges_of_the_calendar() {
        assert_eq!(epoch_seconds("2024-02-29"), Some(1_709_164_800.0));
        assert_eq!(epoch_seconds("2000-03-01"), Some(951_868_800.0));
        assert_eq!(epoch_seconds("1969-12-31T23:59:59Z"), Some(-1.0));
        assert_eq!(epoch_seconds("1900-01-01"), Some(-2_208_988_800.0));
        assert_eq!(
            epoch_seconds("9999-12-31T23:59:59Z"),
            Some(253_402_300_799.0)
        );
    }

    #[test]
    fn ignores_surrounding_whitespace() {
        assert_eq!(epoch_seconds("  2020-01-05 \n"), Some(1_578_182_400.0));
    }

    #[test]
    fn rejects_anything_that_is_not_a_real_date_or_time() {
        for bad in [
            "",
            "yesterday",
            "2026",
            "2026-10",
            "2026-10-21-01",
            "2026-13-01",
            "2026-00-10",
            "2026-10-00",
            "2026-02-29", // not a leap year
            "1900-02-29", // divisible by 100 but not 400
            "2026-04-31",
            "2026-10-21T",
            "2026-10-21T25:00:00Z",
            "2026-10-21T06:61:00Z",
            "2026-10-21T06:11:61Z",
            "2026-10-21T06:11:15.Z",
            "2026-10-21T06:11:15+2",
            "2026-10-21T06:11:15+24:00",
            "2026-10-21T06:11:15+02:60",
            "２０２６-10-21",
        ] {
            assert_eq!(epoch_seconds(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn never_panics_on_non_ascii_input() {
        assert_eq!(epoch_seconds("2026-10-21T06:11:15+０２:００"), None);
        assert_eq!(epoch_seconds("2026-10-21T06:11:15é"), None);
        assert_eq!(epoch_seconds("é"), None);
    }
}
