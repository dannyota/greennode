use std::collections::BTreeMap;

use crate::cli::render::{Value, decode_string};

use super::lexer::SyntaxError;

pub(super) fn parse(input: &str, offset: usize) -> Result<Value, SyntaxError> {
    let mut parser = JsonParser {
        input,
        position: 0,
        offset,
    };
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.position != input.len() {
        return Err(parser.error());
    }
    Ok(value)
}

struct JsonParser<'a> {
    input: &'a str,
    position: usize,
    offset: usize,
}

impl JsonParser<'_> {
    fn error(&self) -> SyntaxError {
        SyntaxError {
            offset: self.offset,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.position).copied()
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, SyntaxError> {
        self.whitespace();
        match self.peek() {
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b't') => {
                self.keyword("true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.keyword("false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.keyword("null")?;
                Ok(Value::Null)
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.error()),
        }
    }

    fn keyword(&mut self, word: &str) -> Result<(), SyntaxError> {
        if self
            .input
            .get(self.position..)
            .is_some_and(|rest| rest.starts_with(word))
        {
            self.position += word.len();
            Ok(())
        } else {
            Err(self.error())
        }
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        let start = self.position;
        if !self.consume(b'"') {
            return Err(self.error());
        }
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.position += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                let quoted = self
                    .input
                    .get(start..self.position)
                    .ok_or_else(|| self.error())?;
                return decode_string(quoted).map_err(|_| self.error());
            }
        }
        Err(self.error())
    }

    fn number(&mut self) -> Result<Value, SyntaxError> {
        let start = self.position;
        self.consume(b'-');
        if self.consume(b'0') {
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error());
            }
        } else {
            if !matches!(self.peek(), Some(b'1'..=b'9')) {
                return Err(self.error());
            }
            self.digits();
        }
        if self.consume(b'.') && !self.digits() {
            return Err(self.error());
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !self.digits() {
                return Err(self.error());
            }
        }
        let text = self
            .input
            .get(start..self.position)
            .ok_or_else(|| self.error())?;
        let value = text.parse::<f64>().map_err(|_| self.error())?;
        if !value.is_finite() {
            return Err(self.error());
        }
        Ok(Value::Number(value))
    }

    fn digits(&mut self) -> bool {
        let start = self.position;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
        self.position != start
    }

    fn array(&mut self, depth: usize) -> Result<Value, SyntaxError> {
        if depth >= 64 {
            return Err(self.error());
        }
        self.consume(b'[');
        self.whitespace();
        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.value(depth + 1)?);
            self.whitespace();
            if self.consume(b']') {
                return Ok(Value::Array(values));
            }
            if !self.consume(b',') {
                return Err(self.error());
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, SyntaxError> {
        if depth >= 64 {
            return Err(self.error());
        }
        self.consume(b'{');
        self.whitespace();
        let mut values = BTreeMap::new();
        if self.consume(b'}') {
            return Ok(Value::Object(values));
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            if !self.consume(b':') {
                return Err(self.error());
            }
            let value = self.value(depth + 1)?;
            values.insert(key, value);
            self.whitespace();
            if self.consume(b'}') {
                return Ok(Value::Object(values));
            }
            if !self.consume(b',') {
                return Err(self.error());
            }
        }
    }
}
