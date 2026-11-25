use super::Error;

pub(super) enum Style {
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
    fn parse(value: &str) -> Result<Self, Error> {
        let (negative, value) = match value.strip_prefix('-') {
            Some(value) => (true, value),
            None => (false, value),
        };
        let (mantissa, exponent) = value.split_once('e').unwrap_or((value, "0"));
        let exponent: i32 = exponent
            .parse()
            .map_err(|_| Error::Invalid("invalid float exponent".into()))?;
        let before = mantissa.find('.').unwrap_or(mantissa.len());
        let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
        let leading = digits
            .len()
            .checked_sub(digits.trim_start_matches('0').len())
            .ok_or_else(|| Error::Invalid("invalid float digits".into()))?;
        let significant = digits.trim_start_matches('0').trim_end_matches('0');
        if significant.is_empty() {
            return Ok(Self {
                negative,
                digits: "0".into(),
                point: 1,
            });
        }
        let point = i32::try_from(before)
            .ok()
            .and_then(|n| n.checked_add(exponent))
            .and_then(|n| n.checked_sub(i32::try_from(leading).ok()?))
            .ok_or_else(|| Error::Invalid("invalid float position".into()))?;
        Ok(Self {
            negative,
            digits: significant.into(),
            point,
        })
    }

    fn render(&self, scientific: bool, key: bool) -> Result<String, Error> {
        let mut out = String::new();
        if self.negative {
            out.push('-');
        }
        if scientific && self.digits != "0" {
            let mut chars = self.digits.chars();
            out.push(
                chars
                    .next()
                    .ok_or_else(|| Error::Invalid("empty float digits".into()))?,
            );
            let tail = chars.as_str();
            if !tail.is_empty() {
                out.push('.');
                out.push_str(tail);
            }
            let exponent = self
                .point
                .checked_sub(1)
                .ok_or_else(|| Error::Invalid("invalid float exponent".into()))?;
            if key {
                out.push_str(&format!("e{exponent:+03}"));
            } else {
                out.push_str(&format!("e{exponent:+}"));
            }
        } else if self.point <= 0 {
            out.push_str("0.");
            let count = usize::try_from(
                self.point
                    .checked_neg()
                    .ok_or_else(|| Error::Invalid("invalid float position".into()))?,
            )
            .map_err(|_| Error::Invalid("invalid float position".into()))?;
            out.push_str(&"0".repeat(count));
            out.push_str(&self.digits);
        } else {
            let point = usize::try_from(self.point)
                .map_err(|_| Error::Invalid("invalid float position".into()))?;
            for (index, digit) in self.digits.chars().enumerate() {
                if index == point {
                    out.push('.');
                }
                out.push(digit);
            }
            if point > self.digits.len() {
                out.push_str(
                    &"0".repeat(
                        point
                            .checked_sub(self.digits.len())
                            .ok_or_else(|| Error::Invalid("invalid float position".into()))?,
                    ),
                );
            }
        }
        Ok(out)
    }
}

fn format(shortest: &str, json_scientific: bool, style: Style) -> Result<String, Error> {
    let decimal = Decimal::parse(shortest)?;
    let exponent = decimal
        .point
        .checked_sub(1)
        .ok_or_else(|| Error::Invalid("invalid float exponent".into()))?;
    let scientific = match style {
        Style::Json => json_scientific,
        Style::Fixed => false,
        Style::Key => !(-4..6).contains(&exponent),
    };
    decimal.render(scientific, matches!(style, Style::Key))
}

pub(super) fn f64(value: f64, style: Style) -> Result<String, Error> {
    if !value.is_finite() {
        return Err(Error::Invalid("nonfinite float".into()));
    }
    let mut buffer = ryu::Buffer::new();
    format(
        buffer.format_finite(value),
        value != 0.0 && (value.abs() < 1e-6 || value.abs() >= 1e21),
        style,
    )
}

pub(super) fn f32(value: f32, style: Style) -> Result<String, Error> {
    if !value.is_finite() {
        return Err(Error::Invalid("nonfinite float".into()));
    }
    let mut buffer = ryu::Buffer::new();
    format(
        buffer.format_finite(value),
        value != 0.0 && (value.abs() < 1e-6 || value.abs() >= 1e21),
        style,
    )
}
