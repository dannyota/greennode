use std::borrow::Cow;

use serde::{Deserialize, Deserializer, de};
use serde_json::value::RawValue;

pub(super) fn decode_money<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<f64>, D::Error> {
    let raw = <&RawValue>::deserialize(deserializer)?.get();
    if raw == "null" {
        return Ok(None);
    }
    let value = if raw.starts_with('"') {
        let text = serde_json::from_str::<String>(raw).map_err(de::Error::custom)?;
        parse_money(&text)
    } else {
        serde_json::from_str(raw).ok()
    };
    value
        .filter(|value: &f64| value.is_finite())
        .map(Some)
        .ok_or_else(|| de::Error::custom("invalid balance"))
}

fn parse_money(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        let (mantissa, power) = hex.split_once(['p', 'P'])?;
        let mantissa = match mantissa.strip_prefix('_') {
            Some(rest) if rest.as_bytes().first().is_some_and(u8::is_ascii_hexdigit) => rest,
            _ => mantissa,
        };
        let mantissa = remove_separators(mantissa, 16)?;
        let power = remove_separators(power, 10)?;
        let exponent = parse_exponent(&power)?;
        let bits = hex_bits(&mantissa, exponent)?;
        let sign = u64::from(text.starts_with('-')) << 63;
        return Some(f64::from_bits(sign | bits));
    }
    remove_separators(text, 10)?.parse().ok()
}

fn remove_separators(text: &str, radix: u32) -> Option<Cow<'_, str>> {
    if !text.contains('_') {
        return Some(Cow::Borrowed(text));
    }
    let bytes = text.as_bytes();
    for (index, _) in text.match_indices('_') {
        let previous = bytes.get(index.checked_sub(1)?)?;
        let next = bytes.get(index + 1)?;
        if !char::from(*previous).is_digit(radix) || !char::from(*next).is_digit(radix) {
            return None;
        }
    }
    Some(Cow::Owned(text.replace('_', "")))
}

fn parse_exponent(power: &str) -> Option<i64> {
    let digits = power.strip_prefix(['+', '-']).unwrap_or(power);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(power.parse().unwrap_or_else(|_| {
        if power.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }
    }))
}

fn hex_bits(mantissa: &str, power: i64) -> Option<u64> {
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }

    let mut leading = 0_u64;
    let mut trailing_bits = 0_i64;
    let mut nonzero_tail = false;
    for byte in whole.bytes().chain(fraction.bytes()) {
        let digit = u64::from(char::from(byte).to_digit(16)?);
        if leading < (1 << 52) {
            leading = (leading << 4) | digit;
        } else {
            trailing_bits += 4;
            nonzero_tail |= digit != 0;
        }
    }
    if leading == 0 {
        return Some(0);
    }

    let leading_bits = i64::from(u64::BITS - leading.leading_zeros());
    let fraction_bits = 4 * i64::try_from(fraction.len()).ok()?;
    let exponent = power.saturating_add(leading_bits + trailing_bits - fraction_bits - 1);
    if exponent < -1075 {
        return Some(0);
    }
    if exponent > 1023 {
        return None;
    }

    let precision = (exponent + 1075).min(53);
    let significand = if leading_bits <= precision {
        leading << (precision - leading_bits)
    } else {
        let discarded = leading_bits - precision;
        let truncated = leading >> discarded;
        let remainder = leading & ((1 << discarded) - 1);
        let halfway = 1 << (discarded - 1);
        let round_up =
            remainder > halfway || (remainder == halfway && (nonzero_tail || truncated & 1 != 0));
        truncated + u64::from(round_up)
    };

    let biased_exponent = u64::try_from((exponent + 1022).max(0)).ok()?;
    Some((biased_exponent << 52) + significand)
}
