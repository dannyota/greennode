const MAX_DEPTH: usize = 10_000;

#[derive(Clone, Copy)]
enum State {
    Value,
    FirstArrayValue,
    FirstObjectKey,
    ObjectKey,
    EndValue,
    String,
    Escape,
    Hex(u8),
    Minus,
    Zero,
    Integer,
    DecimalPoint,
    Fraction,
    Exponent,
    ExponentSign,
    ExponentDigits,
    Literal(&'static [u8]),
}

#[derive(Clone, Copy)]
enum Frame {
    ObjectKey,
    ObjectValue,
    Array,
}

#[derive(PartialEq, Eq)]
enum Step {
    Continue,
    EndBefore,
}

struct ValueExtent<'a> {
    raw: &'a [u8],
    rest: &'a [u8],
}

struct Scanner {
    state: State,
    stack: Vec<Frame>,
}

impl Scanner {
    fn new() -> Self {
        Self {
            state: State::Value,
            stack: Vec::new(),
        }
    }

    fn push(&mut self, frame: Frame) -> Result<(), ()> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(());
        }
        self.stack.push(frame);
        Ok(())
    }

    fn begin_value(&mut self, byte: u8) -> Result<State, ()> {
        let next = match byte {
            b'{' => {
                self.push(Frame::ObjectKey)?;
                State::FirstObjectKey
            }
            b'[' => {
                self.push(Frame::Array)?;
                State::FirstArrayValue
            }
            b'"' => State::String,
            b'-' => State::Minus,
            b'0' => State::Zero,
            b'1'..=b'9' => State::Integer,
            b't' => State::Literal(b"rue"),
            b'f' => State::Literal(b"alse"),
            b'n' => State::Literal(b"ull"),
            _ => return Err(()),
        };
        Ok(next)
    }

    fn end_value(&mut self, byte: u8) -> Result<Step, ()> {
        self.state = State::EndValue;
        let Some(frame) = self.stack.last_mut() else {
            return Ok(Step::EndBefore);
        };
        if is_space(byte) {
            return Ok(Step::Continue);
        }
        match (*frame, byte) {
            (Frame::ObjectKey, b':') => {
                *frame = Frame::ObjectValue;
                self.state = State::Value;
            }
            (Frame::ObjectValue, b',') => {
                *frame = Frame::ObjectKey;
                self.state = State::ObjectKey;
            }
            (Frame::Array, b',') => self.state = State::Value,
            (Frame::ObjectValue, b'}') | (Frame::Array, b']') => {
                self.stack.pop();
            }
            _ => return Err(()),
        }
        Ok(Step::Continue)
    }

    fn step(&mut self, byte: u8) -> Result<Step, ()> {
        use State::*;
        self.state = match self.state {
            Value | FirstArrayValue | FirstObjectKey | ObjectKey if is_space(byte) => {
                return Ok(Step::Continue);
            }
            FirstArrayValue if byte == b']' => return self.end_value(byte),
            FirstObjectKey if byte == b'}' => {
                self.stack.pop();
                EndValue
            }
            ObjectKey | FirstObjectKey if byte == b'"' => String,
            Value | FirstArrayValue => self.begin_value(byte)?,
            EndValue => return self.end_value(byte),
            String => match byte {
                b'"' => EndValue,
                b'\\' => Escape,
                0..=31 => return Err(()),
                _ => String,
            },
            Escape => match byte {
                b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => String,
                b'u' => Hex(4),
                _ => return Err(()),
            },
            Hex(1) if byte.is_ascii_hexdigit() => String,
            Hex(remaining) if byte.is_ascii_hexdigit() => Hex(remaining - 1),
            Minus if byte == b'0' => Zero,
            Minus if byte.is_ascii_digit() => Integer,
            Integer | Fraction | ExponentDigits if byte.is_ascii_digit() => self.state,
            Zero | Integer if byte == b'.' => DecimalPoint,
            Zero | Integer | Fraction if matches!(byte, b'e' | b'E') => Exponent,
            Zero | Integer | Fraction | ExponentDigits => return self.end_value(byte),
            DecimalPoint if byte.is_ascii_digit() => Fraction,
            Exponent if matches!(byte, b'+' | b'-') => ExponentSign,
            Exponent | ExponentSign if byte.is_ascii_digit() => ExponentDigits,
            Literal(suffix) => match suffix.split_first() {
                Some((&expected, [])) if byte == expected => EndValue,
                Some((&expected, rest)) if byte == expected => Literal(rest),
                _ => return Err(()),
            },
            _ => return Err(()),
        };
        Ok(Step::Continue)
    }

    fn next<'a>(&mut self, body: &'a [u8]) -> Result<Option<ValueExtent<'a>>, ()> {
        let body = trim_start(body);
        if body.is_empty() {
            return Ok(None);
        }
        self.state = State::Value;
        self.stack.clear();
        let mut index = 0;
        while let Some(&byte) = body.get(index) {
            if matches!(self.state, State::String) {
                if byte == b'\\' && escaped_code_unit(&body[index..]).is_some() {
                    index += repeated_code_unit_prefix(&body[index..]);
                    continue;
                }
                if is_ordinary_string_byte(byte) {
                    index += ordinary_string_prefix(&body[index..]);
                    continue;
                }
            }
            if self.step(byte)? == Step::EndBefore {
                let (raw, rest) = body.split_at(index);
                return Ok(Some(ValueExtent { raw, rest }));
            }
            index += 1;
        }
        if self.step(b' ')? == Step::EndBefore {
            Ok(Some(ValueExtent {
                raw: body,
                rest: &[],
            }))
        } else {
            Err(())
        }
    }
}

pub(crate) fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

const HEX_DIGITS: [u8; 256] = {
    let mut entries = [u8::MAX; 256];
    let mut value = 0;
    while value < 10 {
        entries[b'0' as usize + value] = value as u8;
        value += 1;
    }
    while value < 16 {
        entries[b'a' as usize + value - 10] = value as u8;
        entries[b'A' as usize + value - 10] = value as u8;
        value += 1;
    }
    entries
};

#[inline(always)]
pub(crate) fn escaped_code_unit(input: &[u8]) -> Option<u16> {
    let [b'\\', b'u', a, b, c, d, ..] = *input else {
        return None;
    };
    let (a, b, c, d) = (
        HEX_DIGITS[a as usize],
        HEX_DIGITS[b as usize],
        HEX_DIGITS[c as usize],
        HEX_DIGITS[d as usize],
    );
    if a | b | c | d > 15 {
        return None;
    }
    Some(u16::from_be_bytes([(a << 4) | b, (c << 4) | d]))
}

pub(crate) fn repeated_code_unit_prefix(input: &[u8]) -> usize {
    const UNIT: usize = 6;
    const BLOCK: usize = 64 * UNIT;
    let unit = &input[..UNIT];
    let mut end = UNIT;
    while input[end..].starts_with(unit) {
        end += UNIT;
        if end == BLOCK {
            let repeated = &input[..end];
            while input[end..].starts_with(repeated) {
                end += repeated.len();
            }
        }
    }
    end
}

fn is_ordinary_string_byte(byte: u8) -> bool {
    byte >= 32 && !matches!(byte, b'"' | b'\\')
}

pub(crate) fn ordinary_string_prefix(bytes: &[u8]) -> usize {
    byte_prefix(bytes, is_ordinary_string_byte, |word| {
        contains_small_byte(word) || contains_byte(word, b'"') || contains_byte(word, b'\\')
    })
}

pub(crate) fn printable_byte_prefix(bytes: &[u8]) -> usize {
    byte_prefix(
        bytes,
        |byte| !byte.is_ascii_control(),
        |word| contains_small_byte(word) || contains_byte(word, 0x7f),
    )
}

#[inline(always)]
fn contains_zero_byte(word: u64) -> bool {
    (word.wrapping_sub(0x0101_0101_0101_0101) & !word & 0x8080_8080_8080_8080) != 0
}

#[inline(always)]
fn contains_byte(word: u64, byte: u8) -> bool {
    contains_zero_byte(word ^ u64::from_ne_bytes([byte; 8]))
}

#[inline(always)]
fn contains_small_byte(word: u64) -> bool {
    contains_zero_byte((word >> 5) & 0x0707_0707_0707_0707)
}

fn byte_prefix(
    bytes: &[u8],
    scan_byte: impl Fn(u8) -> bool,
    scan_word: impl Fn(u64) -> bool,
) -> usize {
    let (words, tail) = bytes.as_chunks::<8>();
    let mut consumed = 0;
    for block in words {
        if scan_word(u64::from_ne_bytes(*block)) {
            return consumed + block.iter().take_while(|&&byte| scan_byte(byte)).count();
        }
        consumed += block.len();
    }
    consumed + tail.iter().take_while(|&&byte| scan_byte(byte)).count()
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\r' | b'\t')
}

fn trim_start(body: &[u8]) -> &[u8] {
    let first = body.iter().position(|&byte| !is_space(byte));
    first.map_or(&[], |index| &body[index..])
}

pub(crate) fn valid_stream(mut body: &[u8]) -> bool {
    let mut scan = Scanner::new();
    loop {
        match scan.next(body) {
            Ok(Some(extent)) => body = extent.rest,
            Ok(None) => return true,
            Err(()) => return false,
        }
    }
}

pub(crate) fn stream_values(mut body: &[u8]) -> impl Iterator<Item = Result<&[u8], ()>> {
    let mut scan = Scanner::new();
    std::iter::from_fn(move || match scan.next(body) {
        Ok(Some(value)) => {
            body = value.rest;
            Some(Ok(value.raw))
        }
        Ok(None) => None,
        Err(()) => {
            body = &[];
            Some(Err(()))
        }
    })
}

pub(crate) fn raw_message(body: &[u8]) -> Result<Option<&[u8]>, ()> {
    match Scanner::new().next(body)? {
        Some(extent) if trim_start(extent.rest).is_empty() => Ok(Some(extent.raw)),
        Some(_) => Err(()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{raw_message, valid_stream};

    #[test]
    fn fast_string_prefix_agrees_with_scalar_at_every_byte_and_alignment() {
        for index in 0..24 {
            for byte in 0..=u8::MAX {
                let mut input = [b'x'; 25];
                input[index] = byte;
                let expected = input
                    .iter()
                    .take_while(|&&byte| super::is_ordinary_string_byte(byte))
                    .count();
                assert_eq!(super::ordinary_string_prefix(&input), expected);
                let expected = input
                    .iter()
                    .take_while(|byte| !byte.is_ascii_control())
                    .count();
                assert_eq!(super::printable_byte_prefix(&input), expected);
            }
        }
    }

    #[test]
    fn reports_stream_boundaries_independently_of_document_shape() {
        for body in [b"01".as_slice(), b"1true", b"{}{}", b"-1-2", b"\"x\"null"] {
            assert!(valid_stream(body));
            assert!(raw_message(body).is_err());
        }
        for body in [
            b"[01]".as_slice(),
            b"0e+",
            b"{} {broken",
            b"[false,]",
            b"{\"k\": 1,}",
            b"\x0b",
            b"\"\\uD80\"",
        ] {
            assert!(!valid_stream(body));
        }
        let raw = b" {\"str\":\"\\uD800\",\"num\":-0.12E-3,\"empty\":[]} ";
        assert_eq!(raw_message(raw), Ok(Some(&raw[1..raw.len() - 1])));
        assert_eq!(raw_message(b" \t\r\n"), Ok(None));
    }

    #[test]
    fn shipped_go_nesting_limit_applies_to_documents_and_streams() {
        let mut body = vec![b'['; super::MAX_DEPTH];
        body.extend(std::iter::repeat_n(b']', super::MAX_DEPTH));
        assert_eq!(raw_message(&body), Ok(Some(body.as_slice())));
        assert!(valid_stream(&body));
        body.insert(0, b'[');
        body.push(b']');
        assert!(!valid_stream(&body));
        assert!(raw_message(&body).is_err());
    }
}
