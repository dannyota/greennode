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
    if let Ok(seconds) = value.parse::<i64>() {
        return (seconds > 0).then(|| Duration::from_secs(seconds as u64));
    }
    let date = parse_http_date(value)?;
    let now = time::OffsetDateTime::from(now);
    let seconds = date
        .unix_timestamp_nanos()
        .checked_sub(now.unix_timestamp_nanos())?;
    (seconds > 0).then(|| Duration::from_nanos(u64::try_from(seconds).unwrap_or(u64::MAX)))
}

fn parse_http_date(value: &str) -> Option<time::OffsetDateTime> {
    let fields: Vec<_> = value.split_whitespace().take(7).collect();
    let (weekday, day, month, year, clock, zone) = match fields.as_slice() {
        [weekday, day, month, year, clock, zone] => {
            (*weekday, *day, *month, *year, *clock, Some(*zone))
        }
        [weekday, date, clock, zone] => {
            let mut parts = date.split('-');
            let day = parts.next()?;
            let month = parts.next()?;
            let year = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            (*weekday, day, month, year, *clock, Some(*zone))
        }
        [weekday, month, day, clock, year] => (*weekday, *day, *month, *year, *clock, None),
        _ => return None,
    };
    let weekday = weekday.trim_end_matches(',');
    if !matches!(
        weekday,
        "Mon"
            | "Tue"
            | "Wed"
            | "Thu"
            | "Fri"
            | "Sat"
            | "Sun"
            | "Monday"
            | "Tuesday"
            | "Wednesday"
            | "Thursday"
            | "Friday"
            | "Saturday"
            | "Sunday"
    ) || zone.is_some_and(|s| s != "GMT")
    {
        return None;
    }
    let month = match month {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let mut year_num: i32 = year.parse().ok()?;
    if year.len() == 2 {
        year_num += if year_num >= 69 { 1900 } else { 2000 };
    } else if year.len() != 4 {
        return None;
    }
    let mut parts = clock.split(':');
    let hour = parts.next()?.parse().ok()?;
    let minute = parts.next()?.parse().ok()?;
    let second = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let date = time::Date::from_calendar_date(
        year_num,
        time::Month::try_from(month).ok()?,
        day.parse().ok()?,
    )
    .ok()?;
    let clock = time::Time::from_hms(hour, minute, second).ok()?;
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
