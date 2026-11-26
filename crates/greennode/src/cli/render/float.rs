use super::Error;
pub(super) use greennode_sdk::output_number::Style;
use greennode_sdk::output_number::{self, format};

fn invalid() -> Error {
    Error::Invalid("invalid float".into())
}

pub(super) fn f64(value: f64, style: Style) -> Result<String, Error> {
    output_number::f64(value, style).ok_or_else(invalid)
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
    .ok_or_else(invalid)
}
