// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Unknown,
    Star,
    Dot,
    Filter,
    Flatten,
    Lparen,
    Rparen,
    Lbracket,
    Rbracket,
    Lbrace,
    Rbrace,
    Or,
    Pipe,
    Number,
    UnquotedIdentifier,
    QuotedIdentifier,
    Comma,
    Colon,
    Lt,
    Lte,
    Gt,
    Gte,
    Eq,
    Ne,
    JsonLiteral,
    StringLiteral,
    Current,
    Expref,
    And,
    Not,
    Eof,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Token {
    pub(super) kind: Kind,
    pub(super) value: String,
    pub(super) position: usize,
    pub(super) length: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub(super) offset: usize,
}

struct Lexer<'a> {
    expression: &'a str,
    position: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.expression.get(self.position..)?.chars().next()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.position += c.len_utf8();
        Some(c)
    }

    fn token(&self, kind: Kind, value: String, position: usize) -> Token {
        let length = value.len();
        Token {
            kind,
            value,
            position,
            length,
        }
    }

    fn delimited(&mut self, end: char) -> Result<String, SyntaxError> {
        let start = self.position;
        loop {
            let before = self.position;
            match self.next() {
                Some(c) if c == end => {
                    return Ok(self
                        .expression
                        .get(start..before)
                        .unwrap_or_default()
                        .to_owned());
                }
                Some('\\') => {
                    self.next();
                }
                Some(_) => {}
                None => {
                    return Err(SyntaxError {
                        offset: self.expression.len(),
                    });
                }
            }
        }
    }

    fn raw(&mut self) -> Result<String, SyntaxError> {
        let mut value = String::new();
        loop {
            match self.next() {
                Some('\'') => return Ok(value),
                Some('\\') if self.peek() == Some('\'') => {
                    self.next();
                    value.push('\'');
                }
                Some(c) => value.push(c),
                None => {
                    return Err(SyntaxError {
                        offset: self.expression.len(),
                    });
                }
            }
        }
    }
}

pub(super) fn tokenize(expression: &str) -> Result<Vec<Token>, SyntaxError> {
    if expression.len() > 16 * 1024 {
        return Err(SyntaxError { offset: 16 * 1024 });
    }
    let mut lexer = Lexer {
        expression,
        position: 0,
    };
    let mut tokens = Vec::new();
    while let Some(c) = lexer.next() {
        let start = lexer.position - c.len_utf8();
        let token = match c {
            ' ' | '\t' | '\n' | '\r' => continue,
            'a'..='z' | 'A'..='Z' | '_' => {
                while lexer
                    .peek()
                    .is_some_and(|x| x.is_ascii_alphanumeric() || x == '_')
                {
                    lexer.next();
                }
                lexer.token(
                    Kind::UnquotedIdentifier,
                    expression
                        .get(start..lexer.position)
                        .unwrap_or_default()
                        .to_owned(),
                    start,
                )
            }
            '0'..='9' | '-' => {
                while lexer.peek().is_some_and(|x| x.is_ascii_digit()) {
                    lexer.next();
                }
                lexer.token(
                    Kind::Number,
                    expression
                        .get(start..lexer.position)
                        .unwrap_or_default()
                        .to_owned(),
                    start,
                )
            }
            '"' => {
                let content = lexer.delimited('"')?;
                let quoted = format!("\"{content}\"");
                let value = crate::cli::render::decode_string(&quoted)
                    .map_err(|_| SyntaxError { offset: start })?;
                lexer.token(Kind::QuotedIdentifier, value, start)
            }
            '\'' => {
                let value = lexer.raw()?;
                lexer.token(Kind::StringLiteral, value, start + 1)
            }
            '`' => {
                let value = lexer.delimited('`')?.replace("\\`", "`");
                lexer.token(Kind::JsonLiteral, value, start + 1)
            }
            _ => {
                let pair = match (c, lexer.peek()) {
                    ('[', Some('?')) => Some(Kind::Filter),
                    ('[', Some(']')) => Some(Kind::Flatten),
                    ('|', Some('|')) => Some(Kind::Or),
                    ('<', Some('=')) => Some(Kind::Lte),
                    ('>', Some('=')) => Some(Kind::Gte),
                    ('!', Some('=')) => Some(Kind::Ne),
                    ('=', Some('=')) => Some(Kind::Eq),
                    ('&', Some('&')) => Some(Kind::And),
                    _ => None,
                };
                let kind = if let Some(kind) = pair {
                    lexer.next();
                    kind
                } else {
                    match c {
                        '*' => Kind::Star,
                        '.' => Kind::Dot,
                        '(' => Kind::Lparen,
                        ')' => Kind::Rparen,
                        '[' => Kind::Lbracket,
                        ']' => Kind::Rbracket,
                        '{' => Kind::Lbrace,
                        '}' => Kind::Rbrace,
                        '|' => Kind::Pipe,
                        ',' => Kind::Comma,
                        ':' => Kind::Colon,
                        '<' => Kind::Lt,
                        '>' => Kind::Gt,
                        '!' => Kind::Not,
                        '=' => Kind::Unknown,
                        '@' => Kind::Current,
                        '&' => Kind::Expref,
                        _ => {
                            return Err(SyntaxError {
                                offset: lexer.position - 1,
                            });
                        }
                    }
                };
                lexer.token(
                    kind,
                    expression
                        .get(start..lexer.position)
                        .unwrap_or_default()
                        .to_owned(),
                    start,
                )
            }
        };
        tokens.push(token);
    }
    tokens.push(Token {
        kind: Kind::Eof,
        value: String::new(),
        position: expression.len(),
        length: 0,
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_single_tokens() {
        let cases = [
            ("*", Kind::Star, "*", 0),
            (".", Kind::Dot, ".", 0),
            ("[?", Kind::Filter, "[?", 0),
            ("[]", Kind::Flatten, "[]", 0),
            ("(", Kind::Lparen, "(", 0),
            (")", Kind::Rparen, ")", 0),
            ("[", Kind::Lbracket, "[", 0),
            ("]", Kind::Rbracket, "]", 0),
            ("{", Kind::Lbrace, "{", 0),
            ("}", Kind::Rbrace, "}", 0),
            ("||", Kind::Or, "||", 0),
            ("|", Kind::Pipe, "|", 0),
            ("29", Kind::Number, "29", 0),
            ("2", Kind::Number, "2", 0),
            ("0", Kind::Number, "0", 0),
            ("-20", Kind::Number, "-20", 0),
            ("foo", Kind::UnquotedIdentifier, "foo", 0),
            (r#""bar""#, Kind::QuotedIdentifier, "bar", 0),
            (r#""bar\"baz""#, Kind::QuotedIdentifier, "bar\"baz", 0),
            (",", Kind::Comma, ",", 0),
            (":", Kind::Colon, ":", 0),
            ("<", Kind::Lt, "<", 0),
            ("<=", Kind::Lte, "<=", 0),
            (">", Kind::Gt, ">", 0),
            (">=", Kind::Gte, ">=", 0),
            ("==", Kind::Eq, "==", 0),
            ("!=", Kind::Ne, "!=", 0),
            ("`[0, 1, 2]`", Kind::JsonLiteral, "[0, 1, 2]", 1),
            (r"`a\`b`", Kind::JsonLiteral, "a`b", 1),
            (r#""✓""#, Kind::QuotedIdentifier, "✓", 0),
            ("'foo'", Kind::StringLiteral, "foo", 1),
            ("'a'", Kind::StringLiteral, "a", 1),
            (r"'foo\'bar'", Kind::StringLiteral, "foo'bar", 1),
            ("@", Kind::Current, "@", 0),
            ("&", Kind::Expref, "&", 0),
            (r#""✓""#, Kind::QuotedIdentifier, "✓", 0),
            (r#""\\""#, Kind::QuotedIdentifier, "\\", 0),
            ("`\"foo\"`", Kind::JsonLiteral, "\"foo\"", 1),
            ("&&", Kind::And, "&&", 0),
            ("!", Kind::Not, "!", 0),
            ("=", Kind::Unknown, "=", 0),
        ];
        for (expression, kind, value, position) in cases {
            let actual = tokenize(expression);
            assert_eq!(
                actual,
                Ok(vec![
                    Token {
                        kind,
                        value: value.into(),
                        position,
                        length: value.len()
                    },
                    Token {
                        kind: Kind::Eof,
                        value: String::new(),
                        position: expression.len(),
                        length: 0
                    }
                ]),
                "{expression}"
            );
        }
    }

    #[test]
    fn combinations() {
        for (expression, expected) in [
            (
                "foo.bar",
                vec![
                    (Kind::UnquotedIdentifier, "foo", 0),
                    (Kind::Dot, ".", 3),
                    (Kind::UnquotedIdentifier, "bar", 4),
                ],
            ),
            (
                "foo[0]",
                vec![
                    (Kind::UnquotedIdentifier, "foo", 0),
                    (Kind::Lbracket, "[", 3),
                    (Kind::Number, "0", 4),
                    (Kind::Rbracket, "]", 5),
                ],
            ),
            (
                "foo[?a<b]",
                vec![
                    (Kind::UnquotedIdentifier, "foo", 0),
                    (Kind::Filter, "[?", 3),
                    (Kind::UnquotedIdentifier, "a", 5),
                    (Kind::Lt, "<", 6),
                    (Kind::UnquotedIdentifier, "b", 7),
                    (Kind::Rbracket, "]", 8),
                ],
            ),
        ] {
            let mut tokens = expected
                .into_iter()
                .map(|(kind, value, position)| Token {
                    kind,
                    value: value.into(),
                    position,
                    length: value.len(),
                })
                .collect::<Vec<_>>();
            tokens.push(Token {
                kind: Kind::Eof,
                value: String::new(),
                position: expression.len(),
                length: 0,
            });
            assert_eq!(tokenize(expression), Ok(tokens));
        }
    }

    #[test]
    fn errors_and_limits() {
        assert_eq!(tokenize("'foo"), Err(SyntaxError { offset: 4 }));
        assert_eq!(tokenize("[?foo==bar?]"), Err(SyntaxError { offset: 10 }));
        assert_eq!(tokenize("é"), Err(SyntaxError { offset: 1 }));
        assert!(tokenize(&" ".repeat(16 * 1024)).is_ok());
        assert!(tokenize(&" ".repeat(16 * 1024 + 1)).is_err());
    }

    #[test]
    fn surrogate_identifier_and_raw_literal() {
        let identifier = tokenize(r#""\ud800""#);
        assert_eq!(
            identifier
                .as_ref()
                .ok()
                .and_then(|tokens| tokens.first())
                .map(|token| token.value.as_str()),
            Some("�")
        );
        let literal = tokenize(r#"`"\ud800"`"#);
        assert_eq!(
            literal
                .as_ref()
                .ok()
                .and_then(|tokens| tokens.first())
                .map(|token| token.value.as_str()),
            Some(r#""\ud800""#)
        );
    }

    #[test]
    fn lex_star() {
        assert_eq!(
            tokenize("*"),
            Ok(vec![
                Token {
                    kind: Kind::Star,
                    value: "*".into(),
                    position: 0,
                    length: 1
                },
                Token {
                    kind: Kind::Eof,
                    value: "".into(),
                    position: 1,
                    length: 0
                }
            ])
        );
    }
}
