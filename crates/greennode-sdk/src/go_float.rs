fn valid_digits(value: &str, radix: u32) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().all(|(i, byte)| {
        if *byte == b'_' {
            i > 0
                && i + 1 < bytes.len()
                && bytes
                    .get(i - 1)
                    .is_some_and(|b| (*b as char).is_digit(radix))
                && bytes
                    .get(i + 1)
                    .is_some_and(|b| (*b as char).is_digit(radix))
        } else {
            (*byte as char).is_digit(radix)
        }
    })
}

pub fn parse_go_float(value: &str) -> Option<f64> {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "nan" => return Some(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let negative = lower.starts_with('-');
    let unsigned = lower.strip_prefix(['+', '-']).unwrap_or(&lower);
    let (hex, number) = match unsigned.strip_prefix("0x") {
        Some(rest) => (true, rest),
        None => (false, unsigned),
    };
    let marker = if hex { 'p' } else { 'e' };
    let (significand, exponent_text) = match number.split_once(marker) {
        Some((a, b)) => (a, Some(b)),
        None if hex => return None,
        None => (number, None),
    };
    let exponent = if let Some(text) = exponent_text {
        let neg = text.starts_with('-');
        let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
        if digits.is_empty() || !valid_digits(digits, 10) {
            return None;
        }
        let clean = digits.replace('_', "");
        let significant = clean.trim_start_matches('0');
        let prefix = significant.get(..significant.len().min(5))?;
        let magnitude = if prefix.is_empty() {
            0
        } else {
            prefix.parse::<i64>().ok()?
        };
        if neg { -magnitude } else { magnitude }
    } else {
        0
    };
    let significand = if hex && significand.starts_with('_') {
        let rest = significand.get(1..)?;
        if !rest.as_bytes().first().is_some_and(u8::is_ascii_hexdigit) {
            return None;
        }
        rest
    } else {
        significand
    };
    let (integer, fraction) = significand.split_once('.').unwrap_or((significand, ""));
    let radix = if hex { 16 } else { 10 };
    if (integer.is_empty() && fraction.is_empty())
        || !valid_digits(integer, radix)
        || !valid_digits(fraction, radix)
    {
        return None;
    }
    if !hex {
        let clean = format!(
            "{}{}e{exponent}",
            if negative { "-" } else { "" },
            significand.replace('_', "")
        );
        let parsed = clean.parse::<f64>().ok()?;
        return parsed.is_finite().then_some(parsed);
    }
    let integer = integer.replace('_', "");
    let fraction = fraction.replace('_', "");
    let point = i64::try_from(integer.len()).ok()?;
    let digits = integer + &fraction;
    let trimmed = digits.trim_start_matches('0');
    let leading = i64::try_from(digits.len() - trimmed.len()).ok()?;
    let kept = trimmed.len().min(16);
    let mantissa = if kept == 0 {
        0
    } else {
        u64::from_str_radix(trimmed.get(..kept)?, 16).ok()?
    };
    let truncated = trimmed.get(kept..)?.bytes().any(|b| b != b'0');
    let scale = exponent.saturating_add(4 * (point - leading - i64::try_from(kept).ok()?));
    let sign = u64::from(negative) << 63;
    if mantissa == 0 {
        return Some(f64::from_bits(sign));
    }
    let high = i64::from(63 - mantissa.leading_zeros());
    let mut power = high.saturating_add(scale);
    if power > 1023 {
        return None;
    }
    let shift = if power >= -1022 {
        high - 52
    } else {
        (-1074i64).saturating_sub(scale)
    };
    let rounded = if shift <= 0 {
        mantissa << u32::try_from(-shift).ok()?
    } else if shift > 64 {
        0
    } else {
        let shift = u32::try_from(shift).ok()?;
        let retained = if shift == 64 { 0 } else { mantissa >> shift };
        let halfway = 1u64 << (shift - 1);
        let remainder = if shift == 64 {
            mantissa
        } else {
            mantissa & ((1u64 << shift) - 1)
        };
        retained
            + u64::from(
                remainder > halfway || (remainder == halfway && (truncated || retained & 1 != 0)),
            )
    };
    let bits = if power < -1022 {
        rounded
    } else {
        let mut significand = rounded;
        if significand == 1 << 53 {
            significand >>= 1;
            power += 1;
        }
        if power > 1023 {
            return None;
        }
        (((power + 1023) as u64) << 52) | (significand & ((1 << 52) - 1))
    };
    Some(f64::from_bits(sign | bits))
}
