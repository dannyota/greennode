const GO_REDACTED: &str = "[redacted]";
const GO_SHORT_VALUE: usize = 8;
const GO_WORK_LIMIT: usize = 8 << 20;
const GO_SPAN_LIMIT: usize = 4096;
const GO_FORM_LIMIT: usize = 256;

struct Budget {
    work: usize,
    spans: usize,
}

impl Budget {
    fn take(&mut self, n: usize) -> bool {
        if n > self.work {
            return false;
        }
        self.work -= n;
        true
    }
}

struct Form {
    text: Vec<u8>,
    prefix: Vec<usize>,
    prefix_work: usize,
}

impl Form {
    fn new(text: &str) -> Self {
        let f = text.as_bytes();
        let mut prefix: Vec<usize> = Vec::with_capacity(f.len());
        let (mut matched, mut work) = (0usize, 0usize);
        for (i, byte) in f.iter().enumerate() {
            if i == 0 {
                prefix.push(0);
                continue;
            }
            loop {
                work = work.saturating_add(1);
                if f.get(matched) == Some(byte) {
                    matched = matched.saturating_add(1);
                    break;
                }
                if matched == 0 {
                    break;
                }
                matched = matched
                    .checked_sub(1)
                    .and_then(|at| prefix.get(at))
                    .copied()
                    .unwrap_or(0);
            }
            prefix.push(matched);
        }
        Self {
            text: f.to_vec(),
            prefix,
            prefix_work: work,
        }
    }
}

struct PreparedSecret {
    length: usize,
    short: bool,
    value: usize,
    escaped: Option<usize>,
    lines: Vec<usize>,
}

struct Resume {
    next: usize,
    work: usize,
    inserted: Vec<usize>,
    whole: bool,
}

pub(super) struct Scrubber {
    forms: Vec<Form>,
    secrets: Vec<PreparedSecret>,
    stamps: Vec<u32>,
    generation: u32,
    resume: Resume,
}

#[allow(clippy::too_many_arguments)]
fn walk(
    secrets: &[PreparedSecret],
    forms: &[Form],
    from: usize,
    budget: &mut Budget,
    inserted: &mut Vec<usize>,
    stamps: &mut [u32],
    generation: u32,
    text: Option<&str>,
) -> Option<usize> {
    for (index, secret) in secrets.iter().enumerate().skip(from) {
        if secret.short && secret.length > 0 && text.is_none() {
            return Some(index);
        }
        if !budget.take(1) {
            return None;
        }
        if secret.length == 0 {
            continue;
        }
        if secret.length > budget.work / 6 {
            return None;
        }
        budget.take(secret.length.saturating_mul(6));
        if secret.short {
            let text = text?;
            for id in std::iter::once(secret.value).chain(secret.escaped) {
                let form = forms.get(id)?;
                match find_spans(text, form, None, budget) {
                    Some(spans) if spans.is_empty() => {}
                    _ => return None,
                }
            }
            continue;
        }
        let mut add = |id: usize, budget: &mut Budget| {
            let Some(length) = forms.get(id).map(|form| form.text.len()) else {
                return false;
            };
            if !budget.take(length) || !budget.take(length.saturating_add(1)) {
                return false;
            }
            let Some(stamp) = stamps.get_mut(id) else {
                return false;
            };
            if *stamp != generation {
                if inserted.len() == GO_FORM_LIMIT {
                    return false;
                }
                *stamp = generation;
                inserted.push(id);
            }
            true
        };
        if !add(secret.value, budget) || secret.escaped.is_some_and(|id| !add(id, budget)) {
            return None;
        }
        for &line in &secret.lines {
            if !add(line, budget) {
                return None;
            }
        }
    }
    Some(secrets.len())
}

impl Scrubber {
    pub(super) fn new(secrets: &[&str]) -> Self {
        let mut ids: std::collections::HashMap<String, usize> = Default::default();
        let mut forms: Vec<Form> = Vec::new();
        let mut intern = |text: &str| -> usize {
            *ids.entry(text.to_owned()).or_insert_with(|| {
                forms.push(Form::new(text));
                forms.len() - 1
            })
        };
        let prepared: Vec<PreparedSecret> = secrets
            .iter()
            .map(|secret| {
                let escaped = go_escape(secret);
                let value = intern(secret);
                let escaped_id = (!escaped.is_empty()).then(|| intern(&escaped));
                let short = secret.len() < GO_SHORT_VALUE;
                let lines = if short || secret.is_empty() {
                    Vec::new()
                } else {
                    secret
                        .split('\n')
                        .map(str::trim)
                        .filter(|line| line.len() >= GO_SHORT_VALUE)
                        .map(&mut intern)
                        .collect()
                };
                PreparedSecret {
                    length: secret.len(),
                    short,
                    value,
                    escaped: escaped_id,
                    lines,
                }
            })
            .collect();
        let count = forms.len();
        let mut scrubber = Self {
            forms,
            secrets: prepared,
            stamps: vec![0; count],
            generation: 0,
            resume: Resume {
                next: 0,
                work: GO_WORK_LIMIT,
                inserted: Vec::new(),
                whole: false,
            },
        };
        let mut budget = Budget {
            work: GO_WORK_LIMIT,
            spans: GO_SPAN_LIMIT,
        };
        let mut inserted = Vec::new();
        let mut stamps = vec![0u32; count];
        match walk(
            &scrubber.secrets,
            &scrubber.forms,
            0,
            &mut budget,
            &mut inserted,
            &mut stamps,
            1,
            None,
        ) {
            Some(next) => {
                scrubber.resume = Resume {
                    next,
                    work: budget.work,
                    inserted,
                    whole: false,
                };
            }
            None => scrubber.resume.whole = true,
        }
        scrubber
    }

    #[cfg(test)]
    pub(super) fn forms_built(&self) -> usize {
        self.forms.len()
    }

    pub(super) fn scrub(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        if withholds_credential_key(text) {
            return self.redact(GO_REDACTED);
        }
        self.redact(text)
    }

    fn redact(&mut self, text: &str) -> String {
        if text.is_empty() || self.secrets.is_empty() {
            return text.to_owned();
        }
        let whole = || GO_REDACTED.to_owned();
        if self.resume.whole {
            return whole();
        }
        let mut budget = Budget {
            work: self.resume.work,
            spans: GO_SPAN_LIMIT,
        };
        let mut owned: Vec<usize>;
        let inserted: &[usize] = if self.resume.next == self.secrets.len() {
            &self.resume.inserted
        } else {
            self.generation = self.generation.wrapping_add(1);
            if self.generation == 0 {
                self.stamps.fill(0);
                self.generation = 1;
            }
            owned = self.resume.inserted.clone();
            for &id in &owned {
                if let Some(stamp) = self.stamps.get_mut(id) {
                    *stamp = self.generation;
                }
            }
            if walk(
                &self.secrets,
                &self.forms,
                self.resume.next,
                &mut budget,
                &mut owned,
                &mut self.stamps,
                self.generation,
                Some(text),
            )
            .is_none()
            {
                return whole();
            }
            &owned
        };
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for &id in inserted {
            let Some(form) = self.forms.get(id) else {
                return whole();
            };
            match find_spans(text, form, Some(spans), &mut budget) {
                Some(next) => spans = next,
                None => return whole(),
            }
        }
        if spans.is_empty() {
            return text.to_owned();
        }
        spans.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        merge_spans(text, &spans).unwrap_or_else(whole)
    }
}

fn merge_spans(text: &str, spans: &[(usize, usize)]) -> Option<String> {
    let (mut start, mut end) = *spans.first()?;
    let mut cleaned = String::new();
    let mut previous = 0;
    for &(next_start, next_end) in spans.iter().skip(1) {
        if next_start < end {
            end = end.max(next_end);
            continue;
        }
        cleaned.push_str(text.get(previous..start)?);
        cleaned.push_str(GO_REDACTED);
        previous = end;
        (start, end) = (next_start, next_end);
    }
    cleaned.push_str(text.get(previous..start)?);
    cleaned.push_str(GO_REDACTED);
    cleaned.push_str(text.get(end..)?);
    Some(cleaned)
}

#[cfg(test)]
fn go_scrub(text: &str, secrets: &[&str]) -> String {
    Scrubber::new(secrets).scrub(text)
}

fn find_spans(
    text: &str,
    form: &Form,
    spans: Option<Vec<(usize, usize)>>,
    budget: &mut Budget,
) -> Option<Vec<(usize, usize)>> {
    let mut spans = spans.unwrap_or_default();
    let (s, f) = (text.as_bytes(), form.text.as_slice());
    if f.is_empty() || f.len() > s.len() {
        return Some(spans);
    }
    if !budget.take(f.len()) || !budget.take(form.prefix_work) {
        return None;
    }
    let prefix = &form.prefix;
    let base = spans.len();
    let mut matched = 0usize;
    for (i, byte) in s.iter().enumerate() {
        loop {
            if !budget.take(1) {
                return None;
            }
            if f.get(matched) == Some(byte) {
                matched = matched.saturating_add(1);
                break;
            }
            if matched == 0 {
                break;
            }
            matched = matched
                .checked_sub(1)
                .and_then(|at| prefix.get(at))
                .copied()?;
        }
        if matched != f.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.checked_sub(f.len())?;
        let extend = spans.len() > base && spans.last().is_some_and(|last| start < last.1);
        match spans.last_mut() {
            Some(last) if extend => last.1 = end,
            _ => {
                budget.spans = budget.spans.checked_sub(1)?;
                spans.push((start, end));
            }
        }
        matched = matched
            .checked_sub(1)
            .and_then(|at| prefix.get(at))
            .copied()?;
    }
    Some(spans)
}

fn withholds_credential_key(text: &str) -> bool {
    const KEY: &[u8] = b"clientsecret";
    let normalized: Vec<u8> = text
        .chars()
        .filter_map(crate::go_text::credential_key_byte)
        .collect();
    normalized.windows(KEY.len()).any(|window| window == KEY)
}

fn go_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

pub(super) fn escaped_len(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => 6,
        ch if (ch as u32) < 0x20 => 6,
        ch => ch.len_utf8(),
    }
}

#[cfg(test)]
mod tests {
    use super::{go_scrub, withholds_credential_key};

    #[test]
    fn scrubs_substrings_like_go() {
        assert_eq!(go_scrub("xxABCDEFGHyy", &["ABCDEFGH"]), "xx[redacted]yy");
        assert_eq!(
            go_scrub("ABCDEFGHABCDEFGH", &["ABCDEFGH"]),
            "[redacted][redacted]"
        );
        assert_eq!(go_scrub("xtokx", &["tok"]), "[redacted]");
        assert_eq!(go_scrub("plain", &["ABCDEFGH"]), "plain");
        assert_eq!(go_scrub("Client_Secret", &[]), "[redacted]");
    }

    #[test]
    fn scrub_withholds_the_whole_string_when_a_budget_is_exhausted() {
        let long = "k".repeat(1_500_000);
        assert_eq!(go_scrub("xx", &[long.as_str()]), "[redacted]");

        let token = |i: usize| format!("token-{i:04}");
        let many: Vec<String> = (0..256).map(token).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(go_scrub("none", &refs), "none");
        let many: Vec<String> = (0..257).map(token).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(go_scrub("none", &refs), "[redacted]");

        let hit = format!("{}-", "ABCDEFGH");
        for (count, expected) in [(4096, false), (4097, true)] {
            let text = hit.repeat(count);
            let out = go_scrub(&text, &["ABCDEFGH"]);
            assert_eq!(out == "[redacted]", expected, "{count}");
        }

        let text = "z".repeat(1 << 20);
        let few: Vec<String> = (0..4).map(token).collect();
        let refs: Vec<&str> = few.iter().map(String::as_str).collect();
        assert_eq!(go_scrub(&text, &refs), text);
        let many: Vec<String> = (0..9).map(token).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(go_scrub(&text, &refs), "[redacted]");
    }

    #[test]
    fn credential_key_classification_follows_go_unicode_tables() {
        for text in [
            "client\u{b2}secret",
            "client\u{2160}secret",
            "client\u{345}secret",
            "CLIENT\u{24b6}SECRET",
            "cl\u{130}entsecret",
            "Client_Secret",
        ] {
            assert!(withholds_credential_key(text), "{text}");
        }
        for text in ["clientXsecret", "clientsecre"] {
            assert!(!withholds_credential_key(text), "{text}");
        }
    }

    #[test]
    fn long_secrets_after_a_short_one_still_scrub_in_place() {
        let secrets = ["ABCDEFGH", "tok", "IJKLMNOP"];
        assert_eq!(go_scrub("xtokx", &secrets), "[redacted]");
        assert_eq!(go_scrub("xIJKLMNOPx", &secrets), "x[redacted]x");
        assert_eq!(
            go_scrub("xABCDEFGHxIJKLMNOPx", &secrets),
            "x[redacted]x[redacted]x"
        );
        assert_eq!(go_scrub("plain", &secrets), "plain");
    }
}
