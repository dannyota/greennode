use super::redirect::Retry;
use std::time::{Duration, SystemTime};

pub(crate) enum SendFailure {
    FailedDial,
    PossiblySent,
}
pub(crate) enum Outcome<'a> {
    Send(SendFailure),
    Status(u16, Option<&'a str>),
    Body,
}
pub(crate) struct Config {
    pub(crate) budget: usize,
    pub(crate) interval: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            budget: 3,
            interval: Duration::from_secs(1),
        }
    }
}
pub(crate) fn decide(
    policy: Retry,
    attempt: usize,
    config: &Config,
    outcome: Outcome<'_>,
    now: SystemTime,
    random: u64,
) -> Option<Duration> {
    if matches!(policy, Retry::Once) || attempt >= config.budget {
        return None;
    }
    let hint = match outcome {
        Outcome::Body => return None,
        Outcome::Send(SendFailure::FailedDial) => None,
        Outcome::Send(SendFailure::PossiblySent) if matches!(policy, Retry::Repeatable) => None,
        Outcome::Send(SendFailure::PossiblySent) => return None,
        Outcome::Status(429, hint) => hint,
        Outcome::Status(502..=504, hint) if matches!(policy, Retry::Repeatable) => hint,
        Outcome::Status(_, _) => return None,
    };
    let cap = Duration::from_secs(30);
    if let Some(hint) = hint.and_then(|s| retry_after(s, now)) {
        return Some(hint.min(cap));
    }
    let interval = if config.interval.is_zero() {
        Duration::from_secs(1)
    } else {
        config.interval
    };
    let multiplier = 1u128.checked_shl(u32::try_from(attempt).unwrap_or(u32::MAX));
    let nanos = multiplier
        .and_then(|factor| interval.as_nanos().checked_mul(factor))
        .unwrap_or(u128::MAX)
        .min(cap.as_nanos());
    let half = nanos / 2;
    let jitter = half * u128::from(random) / u128::from(u64::MAX);
    Some(Duration::from_nanos((half + jitter) as u64))
}

fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let digits = value.strip_prefix('+').unwrap_or(value);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        let seconds = digits.bytes().fold(0u64, |n, b| {
            n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
        });
        return (seconds > 0).then(|| Duration::from_secs(seconds));
    }
    let date = parse_http_date(value)?;
    let timestamp = date.unix_timestamp_nanos();
    let absolute = timestamp.unsigned_abs();
    let duration = Duration::new(
        u64::try_from(absolute / 1_000_000_000).ok()?,
        (absolute % 1_000_000_000) as u32,
    );
    let date = if timestamp >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(duration)?
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(duration)?
    };
    date.duration_since(now)
        .ok()
        .filter(|duration| !duration.is_zero())
}

fn signed_zone(value: &str) -> bool {
    let digits = value.strip_prefix('+').or_else(|| value.strip_prefix('-'));
    digits.is_some_and(|digits| {
        !digits.is_empty()
            && digits
                .bytes()
                .try_fold(0u64, |number, digit| {
                    digit.is_ascii_digit().then_some(())?;
                    number.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
                })
                .is_some_and(|number| number <= 23)
    })
}

fn rfc850_zone(value: &str) -> bool {
    if value.starts_with("UTC") {
        return value == "UTC";
    }
    if let Some(suffix) = value.strip_prefix("GMT") {
        return suffix.is_empty() || signed_zone(suffix);
    }
    if matches!(value, "ChST" | "MeST" | "WITA") {
        return true;
    }
    if value.starts_with(['+', '-']) {
        return value.len() >= 3 && signed_zone(value);
    }
    value.bytes().all(|b| b.is_ascii_uppercase())
        && (value.len() == 3 || ((value.len() == 4 || value.len() == 5) && value.ends_with('T')))
}

fn parse_http_date(value: &str) -> Option<time::OffsetDateTime> {
    if value.starts_with(' ')
        || value.ends_with(' ')
        || value.bytes().any(|b| b.is_ascii_whitespace() && b != b' ')
    {
        return None;
    }
    let fields: Vec<_> = value
        .split(' ')
        .filter(|part| !part.is_empty())
        .take(7)
        .collect();
    let (weekday, day, month, year, clock, layout) = match fields.as_slice() {
        [weekday, day, month, year, clock, "GMT"] => (*weekday, *day, *month, *year, *clock, 0),
        [weekday, date, clock, zone] if rfc850_zone(zone) => {
            let mut parts = date.split('-');
            let day = parts.next()?;
            let month = parts.next()?;
            let year = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            (*weekday, day, month, year, *clock, 1)
        }
        [weekday, month, day, clock, year] => (*weekday, *day, *month, *year, *clock, 2),
        _ => return None,
    };
    let weekday = if layout == 2 {
        weekday
    } else {
        weekday.strip_suffix(',')?
    };
    let valid_weekday = if layout == 1 {
        [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ]
        .iter()
        .any(|name| name.eq_ignore_ascii_case(weekday))
    } else {
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .iter()
            .any(|name| name.eq_ignore_ascii_case(weekday))
    };
    if !valid_weekday
        || (layout != 2 && day.len() != 2)
        || (layout == 2 && !(1..=2).contains(&day.len()))
        || year.len() != if layout == 1 { 2 } else { 4 }
    {
        return None;
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|name| name.eq_ignore_ascii_case(month))?
        + 1;
    if !year.bytes().all(|b| b.is_ascii_digit()) || !day.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut year_num: i32 = year.parse().ok()?;
    if layout == 1 {
        year_num += if year_num >= 69 { 1900 } else { 2000 };
    }
    let mut parts = clock.split(':');
    let hour = parts.next()?;
    let minute = parts.next()?;
    let seconds = parts.next()?;
    if parts.next().is_some()
        || !(1..=2).contains(&hour.len())
        || minute.len() != 2
        || !hour.bytes().all(|b| b.is_ascii_digit())
        || !minute.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let (second, fraction) = match seconds.split_once(['.', ',']) {
        Some((second, fraction)) => (second, Some(fraction)),
        None => (seconds, None),
    };
    if second.len() != 2 || !second.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let nanos = if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let digits = fraction.len().min(9);
        let n: u32 = fraction.get(..digits)?.parse().ok()?;
        n * 10u32.pow(9 - u32::try_from(digits).ok()?)
    } else {
        0
    };
    let date = time::Date::from_calendar_date(
        year_num,
        time::Month::try_from(u8::try_from(month).ok()?).ok()?,
        day.parse().ok()?,
    )
    .ok()?;
    let clock = time::Time::from_hms_nano(
        hour.parse().ok()?,
        minute.parse().ok()?,
        second.parse().ok()?,
        nanos,
    )
    .ok()?;
    Some(time::PrimitiveDateTime::new(date, clock).assume_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;
    fn check(policy: Retry, attempt: usize, outcome: Outcome<'_>) -> Option<Duration> {
        decide(policy, attempt, &Config::default(), outcome, UNIX_EPOCH, 0)
    }
    #[test]
    fn once() {
        assert!(check(Retry::Repeatable, 0, Outcome::Status(429, None)).is_some());
        assert!(check(Retry::Once, 0, Outcome::Status(429, None)).is_none());
    }
    #[test]
    fn budget() {
        assert!(check(Retry::Repeatable, 2, Outcome::Status(429, None)).is_some());
        assert!(check(Retry::Repeatable, 3, Outcome::Status(429, None)).is_none());
    }
    #[test]
    fn body() {
        assert!(check(Retry::Repeatable, 0, Outcome::Status(503, None)).is_some());
        assert!(check(Retry::Repeatable, 0, Outcome::Body).is_none());
    }
    #[test]
    fn failed_dial() {
        assert!(
            check(
                Retry::NonIdempotent,
                0,
                Outcome::Send(SendFailure::FailedDial)
            )
            .is_some()
        );
        assert!(
            check(
                Retry::NonIdempotent,
                0,
                Outcome::Send(SendFailure::PossiblySent)
            )
            .is_none()
        );
    }
    #[test]
    fn repeatable_send() {
        assert!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Send(SendFailure::PossiblySent)
            )
            .is_some()
        );
        assert!(
            check(
                Retry::NonIdempotent,
                0,
                Outcome::Send(SendFailure::PossiblySent)
            )
            .is_none()
        );
    }
    #[test]
    fn non_idempotent_status() {
        assert!(check(Retry::NonIdempotent, 0, Outcome::Status(429, None)).is_some());
        assert!(check(Retry::NonIdempotent, 0, Outcome::Status(503, None)).is_none());
    }
    #[test]
    fn repeatable_status() {
        for status in [429, 502, 503, 504] {
            assert!(check(Retry::Repeatable, 0, Outcome::Status(status, None)).is_some());
        }
        assert!(check(Retry::Repeatable, 0, Outcome::Status(500, None)).is_none());
    }
    #[test]
    fn sequence() {
        let expected = [500, 1000, 2000, 4000, 8000, 15000, 15000];
        for (attempt, ms) in expected.into_iter().enumerate() {
            let config = Config {
                budget: 10,
                interval: Duration::from_secs(1),
            };
            assert_eq!(
                decide(
                    Retry::Repeatable,
                    attempt,
                    &config,
                    Outcome::Status(429, None),
                    UNIX_EPOCH,
                    0
                ),
                Some(Duration::from_millis(ms))
            );
        }
    }
    #[test]
    fn jitter() {
        assert_eq!(
            decide(
                Retry::Repeatable,
                0,
                &Config::default(),
                Outcome::Status(429, None),
                UNIX_EPOCH,
                u64::MAX
            ),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            check(Retry::Repeatable, 0, Outcome::Status(429, None)),
            Some(Duration::from_millis(500))
        );
    }
    #[test]
    fn hint_seconds() {
        assert_eq!(
            check(Retry::Repeatable, 0, Outcome::Status(429, Some("9"))),
            Some(Duration::from_secs(9))
        );
        assert_eq!(
            check(Retry::Repeatable, 0, Outcome::Status(429, Some("bad"))),
            Some(Duration::from_millis(500))
        );
    }
    #[test]
    fn huge_hint() {
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("999999999999999999999999999999"))
            ),
            Some(Duration::from_secs(30))
        );
    }
    #[test]
    fn hint_cap() {
        assert_eq!(
            check(Retry::Repeatable, 0, Outcome::Status(429, Some("31"))),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            check(Retry::Repeatable, 0, Outcome::Status(429, Some("5"))),
            Some(Duration::from_secs(5))
        );
    }
    #[test]
    fn large_attempt() {
        let config = Config {
            budget: 1000,
            interval: Duration::from_secs(1),
        };
        assert_eq!(
            decide(
                Retry::Repeatable,
                120,
                &config,
                Outcome::Status(429, None),
                UNIX_EPOCH,
                0
            ),
            Some(Duration::from_secs(15))
        );
    }
    #[test]
    fn rfc850_zone() {
        for zone in [
            "UTC", "XYZ", "ABCT", "ABCXT", "ChST", "MeST", "WITA", "GMT+03", "+03",
        ] {
            let date = format!("Thursday, 01-Jan-70 00:00:08 {zone}");
            assert_eq!(
                check(Retry::Repeatable, 0, Outcome::Status(429, Some(&date))),
                Some(Duration::from_secs(8))
            );
        }
        for zone in ["ABCD", "ABCDEF", "UTC+03", "+24"] {
            let date = format!("Thursday, 01-Jan-70 00:00:08 {zone}");
            assert_eq!(
                check(Retry::Repeatable, 0, Outcome::Status(429, Some(&date))),
                Some(Duration::from_millis(500))
            );
        }
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thu, 01 Jan 1970 00:00:08 UTC"))
            ),
            Some(Duration::from_millis(500))
        );
    }
    #[test]
    fn gmt_offsets_preserve_instant() {
        for zone in ["GMT+03", "GMT-3"] {
            let date = format!("Thursday, 01-Jan-70 00:00:08 {zone}");
            assert_eq!(
                check(Retry::Repeatable, 0, Outcome::Status(429, Some(&date))),
                Some(Duration::from_secs(8))
            );
        }
    }
    #[test]
    fn legacy_dates() {
        for date in [
            "Thursday, 01-Jan-70 00:00:08 GMT",
            "Thu Jan  1 00:00:08 1970",
        ] {
            assert_eq!(
                check(Retry::Repeatable, 0, Outcome::Status(429, Some(date))),
                Some(Duration::from_secs(8))
            );
        }
    }
    #[test]
    fn date_requires_comma() {
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thu 01 Jan 1970 00:00:08 GMT"))
            ),
            Some(Duration::from_millis(500))
        );
    }
    #[test]
    fn rfc850_fraction() {
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thursday, 01-Jan-70 00:00:08.250 GMT"))
            ),
            Some(Duration::from_millis(8250))
        );
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thursday 01-Jan-70 00:00:08 GMT"))
            ),
            Some(Duration::from_millis(500))
        );
    }
    #[test]
    fn asctime_fraction() {
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thu Jan  1 00:00:08.250 1970"))
            ),
            Some(Duration::from_millis(8250))
        );
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thu, Jan  1 00:00:08 1970"))
            ),
            Some(Duration::from_millis(500))
        );
    }
    #[test]
    fn date_fraction() {
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thu, 01 Jan 1970 00:00:08.250 GMT"))
            ),
            Some(Duration::from_millis(8250))
        );
    }
    #[test]
    fn outside_date_range() {
        let now = UNIX_EPOCH + Duration::from_secs(253402300800);
        assert_eq!(
            decide(
                Retry::Repeatable,
                0,
                &Config::default(),
                Outcome::Status(429, Some("Thu, 01 Jan 1970 00:00:08 GMT")),
                now,
                0
            ),
            Some(Duration::from_millis(500))
        );
    }
    #[test]
    fn hint_date() {
        assert_eq!(
            decide(
                Retry::Repeatable,
                0,
                &Config::default(),
                Outcome::Status(429, Some("Thu, 01 Jan 1970 00:00:08 GMT")),
                UNIX_EPOCH,
                0
            ),
            Some(Duration::from_secs(8))
        );
        assert_eq!(
            check(
                Retry::Repeatable,
                0,
                Outcome::Status(429, Some("Thu, 01 Jan 1970 00:00:00 GMT"))
            ),
            Some(Duration::from_millis(500))
        );
    }
}
