use std::str::FromStr;

use time::{Date, Month, Time};

pub(super) fn parse(value: &str) -> Option<i64> {
    let (date, clock) = value.split_once('T')?;
    let (year, date) = date.split_once('-')?;
    let (month, day) = date.split_once('-')?;
    let date = Date::from_calendar_date(
        digits(year, 4)?,
        Month::try_from(digits::<u8>(month, 2)?).ok()?,
        digits(day, 2)?,
    )
    .ok()?;
    let (clock, offset) = zone(clock)?;
    let (hour, clock) = clock.split_once(':')?;
    let (minute, clock) = clock.split_once(':')?;
    let (second, millis) = if let Some((seconds, fraction)) = clock.split_once(['.', ',']) {
        (seconds, milliseconds(fraction)?)
    } else {
        (clock, 0)
    };
    if !matches!(hour.len(), 1 | 2) {
        return None;
    }
    let time = Time::from_hms(
        digits(hour, hour.len())?,
        digits(minute, 2)?,
        digits(second, 2)?,
    )
    .ok()?;
    let seconds = date.with_time(time).assume_utc().unix_timestamp();
    Some((seconds - offset) * 1000 + i64::from(millis))
}

fn zone(clock: &str) -> Option<(&str, i64)> {
    if let Some(clock) = clock.strip_suffix('Z') {
        return Some((clock, 0));
    }
    let (clock, offset) = clock.split_at_checked(clock.len().checked_sub(6)?)?;
    let (hours, minutes) = offset.get(1..)?.split_once(':')?;
    let hours: i64 = digits(hours, 2)?;
    let minutes: i64 = digits(minutes, 2)?;
    if hours > 24 || minutes > 60 {
        return None;
    }
    let magnitude = hours * 3600 + minutes * 60;
    match offset.as_bytes().first()? {
        b'+' => Some((clock, magnitude)),
        b'-' => Some((clock, -magnitude)),
        _ => None,
    }
}

fn milliseconds(value: &str) -> Option<u16> {
    if value.is_empty() || !value.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    Some(
        value
            .bytes()
            .zip([100, 10, 1])
            .map(|(digit, factor)| u16::from(digit - b'0') * factor)
            .sum(),
    )
}

fn digits<T: FromStr>(value: &str, width: usize) -> Option<T> {
    if value.len() != width || !value.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}
