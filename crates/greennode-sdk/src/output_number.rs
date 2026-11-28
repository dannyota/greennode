#[derive(Clone, Copy)]
pub enum Style {
    Json,
    Fixed,
    Key,
}

struct Decimal {
    negative: bool,
    digits: String,
    point: i32,
}

impl Decimal {
    fn parse(value: &str) -> Option<Self> {
        if value.len() > 32 {
            return None;
        }
        let (negative, value) = match value.strip_prefix('-') {
            Some(value) => (true, value),
            None => (false, value),
        };
        let (mantissa, exponent) = value.split_once('e').unwrap_or((value, "0"));
        let exponent: i32 = exponent.parse().ok()?;
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits: String = whole.chars().chain(fraction.chars()).collect();
        if !(-324..=308).contains(&exponent)
            || digits.is_empty()
            || !digits.bytes().all(|digit| digit.is_ascii_digit())
        {
            return None;
        }
        let leading = digits
            .len()
            .checked_sub(digits.trim_start_matches('0').len())?;
        let significant = digits.trim_start_matches('0').trim_end_matches('0');
        if significant.is_empty() {
            return Some(Self {
                negative,
                digits: "0".into(),
                point: 1,
            });
        }
        let point = i32::try_from(whole.len())
            .ok()
            .and_then(|n| n.checked_add(exponent))
            .and_then(|n| n.checked_sub(i32::try_from(leading).ok()?))?;
        Some(Self {
            negative,
            digits: significant.into(),
            point,
        })
    }

    fn render(&self, scientific: bool, key: bool) -> Option<String> {
        let mut out = String::new();
        if self.negative {
            out.push('-');
        }
        if scientific && self.digits != "0" {
            let mut chars = self.digits.chars();
            out.push(chars.next()?);
            let tail = chars.as_str();
            if !tail.is_empty() {
                out.push('.');
                out.push_str(tail);
            }
            let exponent = self.point.checked_sub(1)?;
            if key {
                out.push_str(&format!("e{exponent:+03}"));
            } else {
                out.push_str(&format!("e{exponent:+}"));
            }
        } else if self.point <= 0 {
            out.push_str("0.");
            let count = usize::try_from(self.point.checked_neg()?).ok()?;
            out.push_str(&"0".repeat(count));
            out.push_str(&self.digits);
        } else {
            let point = usize::try_from(self.point).ok()?;
            for (index, digit) in self.digits.chars().enumerate() {
                if index == point {
                    out.push('.');
                }
                out.push(digit);
            }
            if point > self.digits.len() {
                out.push_str(&"0".repeat(point.checked_sub(self.digits.len())?));
            }
        }
        Some(out)
    }
}

pub fn format(shortest: &str, json_scientific: bool, style: Style) -> Option<String> {
    let decimal = Decimal::parse(shortest)?;
    let exponent = decimal.point.checked_sub(1)?;
    let scientific = match style {
        Style::Json => json_scientific,
        Style::Fixed => false,
        Style::Key => !(-4..6).contains(&exponent),
    };
    decimal.render(scientific, matches!(style, Style::Key))
}

fn json_scientific(value: f64) -> bool {
    value != 0.0 && (value.abs() < 1e-6 || value.abs() >= 1e21)
}

pub fn f64(value: f64, style: Style) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    let shortest = serde_json::to_string(&value).ok()?;
    format(&shortest, json_scientific(value), style)
}

/// Formats a price to two decimal places for CLI estimates.
pub fn fixed_two(value: f64) -> String {
    format!("{value:.2}")
}

pub(crate) fn reflects(
    value: f64,
    credentials: &crate::wire::product::sender::SentCredentials,
    include_sdk_json: bool,
) -> bool {
    if credentials.values().is_empty() {
        return !value.is_finite();
    }
    any_spelling(value, include_sdk_json, |text| {
        crate::wire::product::response::reflects_bytes(text.as_bytes(), credentials)
    })
}

pub(crate) fn any_spelling(
    value: f64,
    include_sdk_json: bool,
    reflects: impl Fn(&str) -> bool,
) -> bool {
    if !value.is_finite() {
        return true;
    }
    let Ok(shortest) = serde_json::to_string(&value) else {
        return true;
    };
    (include_sdk_json && reflects(&shortest))
        || [Style::Json, Style::Fixed].into_iter().any(|style| {
            format(&shortest, json_scientific(value), style).is_none_or(|text| reflects(&text))
        })
}

pub(crate) fn rendered_matches(value: f64, matches: impl Fn(&str) -> bool) -> bool {
    if !value.is_finite() {
        return true;
    }
    let Ok(shortest) = serde_json::to_string(&value) else {
        return true;
    };
    matches(&shortest)
        || matches(&fixed_two(value))
        || [Style::Json, Style::Fixed, Style::Key]
            .into_iter()
            .any(|style| {
                format(&shortest, json_scientific(value), style).is_none_or(|text| matches(&text))
            })
}
