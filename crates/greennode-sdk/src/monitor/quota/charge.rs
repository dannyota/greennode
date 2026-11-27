use super::scrub::{Scrubber, escaped_len};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::validate;

pub(super) fn go_secrets<'a>(
    evidence: &'a Evidence,
    cdn: Option<&'a crate::Secret>,
) -> Vec<&'a str> {
    let mut secrets: Vec<&str> = evidence
        .credentials()
        .bearers()
        .into_iter()
        .map(crate::Secret::expose_secret)
        .collect();
    if let Some(key) = cdn.map(crate::Secret::expose_secret)
        && !key.is_empty()
        && !secrets.contains(&key)
    {
        secrets.push(key);
    }
    secrets
}

pub(super) struct ValidBody<'a>(&'a str);

impl<'a> ValidBody<'a> {
    pub(super) fn check(raw: &'a str) -> Result<Self, ()> {
        validate(raw.as_bytes(), &[], raw.len())
            .map(|()| Self(raw))
            .map_err(|_| ())
    }
}

pub(super) fn charged_len(body: &ValidBody<'_>, secrets: &[&str]) -> Result<usize, ()> {
    charged_len_with(body, &mut Scrubber::new(secrets))
}

pub(super) fn charged_len_with(body: &ValidBody<'_>, scrubber: &mut Scrubber) -> Result<usize, ()> {
    let mut cursor = Cursor {
        bytes: body.0.as_bytes(),
        at: 0,
        scrubber,
    };
    let size = cursor.value().ok_or(())?;
    cursor.skip_space();
    (cursor.at == cursor.bytes.len()).then_some(size).ok_or(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    scrubber: &'a mut Scrubber,
}

impl Cursor<'_> {
    fn skip_space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip_space();
        let found = self.bytes.get(self.at) == Some(&byte);
        self.at += usize::from(found);
        found
    }

    fn value(&mut self) -> Option<usize> {
        self.skip_space();
        match *self.bytes.get(self.at)? {
            b'"' => {
                let text = self.string()?;
                Some(string_charge(&self.scrubber.scrub(&text)))
            }
            b'{' => self.object(),
            b'[' => self.array(),
            b't' | b'n' => self.literal(4),
            b'f' => self.literal(5),
            _ => crate::monitor::raw_json::number(self.bytes, &mut self.at),
        }
    }

    fn literal(&mut self, length: usize) -> Option<usize> {
        self.at += length;
        (self.at <= self.bytes.len()).then_some(length)
    }

    fn array(&mut self) -> Option<usize> {
        self.at += 1;
        let (mut total, mut count) = (0, 0usize);
        if !self.eat(b']') {
            loop {
                total += self.value()?;
                count += 1;
                if self.eat(b']') {
                    break;
                }
                if !self.eat(b',') {
                    return None;
                }
            }
        }
        Some(2 + total + count.saturating_sub(1))
    }

    fn object(&mut self) -> Option<usize> {
        self.at += 1;
        let mut kept: std::collections::HashMap<String, usize> = Default::default();
        if !self.eat(b'}') {
            loop {
                self.skip_space();
                let text = self.string()?;
                let key = self.scrubber.scrub(&text);
                if !self.eat(b':') {
                    return None;
                }
                let size = self.value()?;
                kept.entry(key)
                    .and_modify(|stored| *stored = (*stored).max(size))
                    .or_insert(size);
                if self.eat(b'}') {
                    break;
                }
                if !self.eat(b',') {
                    return None;
                }
            }
        }
        let total: usize = kept
            .iter()
            .map(|(key, size)| string_charge(key) + 1 + size)
            .sum();
        Some(2 + total + kept.len().saturating_sub(1))
    }

    fn string(&mut self) -> Option<String> {
        crate::monitor::raw_json::string(self.bytes, &mut self.at)
    }
}

fn string_charge(text: &str) -> usize {
    2 + text.chars().map(escaped_len).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::{ValidBody, charged_len, charged_len_with, go_secrets};
    use crate::monitor::quota::scrub::Scrubber;

    fn charged(raw: &str, secrets: &[&str]) -> usize {
        charged_len(&ValidBody::check(raw).unwrap(), secrets).unwrap()
    }

    fn charged_len_plain(raw: &str) -> usize {
        charged(raw, &[])
    }

    #[test]
    fn counts_go_marshal_length() {
        assert_eq!(charged_len_plain(r#"{ "k" : "\u0061" }"#), 9);
        assert_eq!(charged_len_plain(r#""\u003c""#), 8);
        assert_eq!(charged_len_plain("\"<\""), 8);
        assert_eq!(
            charged_len_plain(r#""\n\b\u0001\"\\""#),
            2 + 2 + 2 + 6 + 2 + 2
        );
        assert_eq!(charged_len_plain(r#""\ud83d\ude00""#), 6);
        assert_eq!(charged_len_plain(r#""\ud83d""#), 5);
        assert_eq!(charged_len_plain(r#""\u2028""#), 8);
        assert_eq!(charged_len_plain("[ 1.50 , true ]"), 11);
    }

    #[test]
    fn scrubbed_key_collisions_charge_the_larger_value_once() {
        let raw = r#"{"aABCDEFGH":"a much longer retained value","a[redacted]":"y"}"#;
        assert_eq!(
            charged(raw, &["ABCDEFGH"]),
            r#"{"a[redacted]":"a much longer retained value"}"#.len()
        );
        let raw = r#"{"aABCDEFGH":"<<","a[redacted]":"abcdefg"}"#;
        assert_eq!(
            charged(raw, &["ABCDEFGH"]),
            r#"{"a[redacted]":"\u003c\u003c"}"#.len()
        );
    }

    #[test]
    fn colliding_keys_are_charged_in_linear_time() {
        let mut raw = format!(r#"{{"clientsecret0":"{}""#, "v".repeat(500_000));
        for i in 1..25_000 {
            raw.push_str(&format!(r#","clientsecret{i}":"w""#));
        }
        raw.push('}');
        let started = std::time::Instant::now();
        let charged = charged(&raw, &[]);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(charged, r#"{"[redacted]":""#.len() + 500_000 + 2);
    }

    #[test]
    fn keys_are_scrubbed_without_any_recorded_credential() {
        assert_eq!(
            charged(r#"{"clientSecret":"abc"}"#, &[]),
            r#"{"[redacted]":"abc"}"#.len()
        );
    }

    #[test]
    fn scrub_set_is_the_sent_bearers_plus_the_cdn_key() {
        let mut evidence = crate::wire::product::evidence::Evidence::new();
        evidence.add_secret(&crate::Secret::new("client-id"));
        evidence.add_bearer(&crate::Secret::new("sent-bearer"));
        let cdn = crate::Secret::new("cdn-key");
        assert_eq!(
            go_secrets(&evidence, Some(&cdn)),
            ["sent-bearer", "cdn-key"]
        );
    }

    fn nested(levels: usize, inner: &str) -> String {
        format!("{}\"{inner}\"{}", "[".repeat(levels), "]".repeat(levels))
    }

    #[test]
    fn lone_surrogates_do_not_skip_the_scrub() {
        assert_eq!(charged(r#""\ud83dtok""#, &["tok"]), 12);
        assert_eq!(charged(r#""\\ud83dtok""#, &["tok"]), 12);
    }

    #[test]
    fn nested_bodies_are_scrubbed_at_every_accepted_depth() {
        let inner = format!("{0}tok{0}", "<".repeat(1000));
        let scrubbed = |levels: usize| 2 * levels + 2 + "[redacted]".len();
        assert!(ValidBody::check(&nested(129, &inner)).is_err());
        for levels in [1, 127, 128] {
            assert_eq!(
                charged(&nested(levels, &inner), &["tok"]),
                scrubbed(levels),
                "{levels}"
            );
        }
    }

    #[test]
    fn numbers_keep_their_literal_length() {
        for number in ["1e5", "1E5", "1e-5", "-0.0e5", "1.50", "100"] {
            assert_eq!(charged_len_plain(number), number.len(), "{number}");
        }
    }

    #[test]
    fn an_undecodable_validated_body_is_an_invalid_structure() {
        assert!(charged_len(&ValidBody("{"), &[]).is_err());
        assert!(charged_len(&ValidBody("[1,"), &[]).is_err());
        assert!(charged_len(&ValidBody("1 2"), &[]).is_err());
    }
    #[test]
    fn forms_are_built_once_per_body_not_once_per_string() {
        let secrets: Vec<String> = (0..256).map(|i| format!("secret-{i:04}")).collect();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let raw = format!("[{}\"a\"]", "\"a\",".repeat(262_143));
        let body = ValidBody::check(&raw).unwrap();
        let mut scrubber = Scrubber::new(&refs);
        let built = scrubber.forms_built();
        assert_eq!(built, 256);
        let size = charged_len_with(&body, &mut scrubber).unwrap();
        assert_eq!(size, 2 + 262_144 * 3 + 262_143);
        assert_eq!(scrubber.forms_built(), built);
    }
}
