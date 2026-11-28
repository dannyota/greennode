use std::{collections::BTreeSet, net::IpAddr};

use bytes::Bytes;
use http::{Method, StatusCode, header::CONTENT_TYPE};
use serde::Serialize;

use super::models::redact_text;
use super::wire::{convert, operation, raw, read_failure};
use super::{Client, ErrorDetail};
use crate::error::SafeText;
use crate::wire::product::evidence::Evidence;
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext};

const OP: &str = "cdn.ListIPRanges";
const CAP: usize = 4 << 20;

/// Input for reading the public CDN IP ranges.
#[derive(Clone, Copy, Default)]
pub struct ListIpRangesInput;

crate::error::debug_record!(ListIpRangesInput unit);

/// The canonical IP prefixes and their source.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListIpRangesOutput {
    /// Canonical IP ranges sorted by IP address and prefix length.
    pub items: Vec<String>,
    /// The configured public documentation URL.
    pub source: String,
}

crate::wire::product::output::public_fields!(ListIpRangesOutput { items, source });

impl Client {
    /// Reads the public CDN IP ranges without using account credentials.
    pub async fn list_ip_ranges(
        &self,
        ctx: &RequestContext,
        _: ListIpRangesInput,
    ) -> Result<ListIpRangesOutput, Failure<ListIpRangesOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut source = self
                .session
                .endpoints()
                .cdn_docs
                .as_deref()
                .ok_or_else(|| read_failure(Error::new(ErrorClass::InvalidConfig)))?
                .to_owned();
            let operation = operation(OP, "", Method::GET, CAP, true);
            let mut evidence = Evidence::new();
            let reply = raw(
                self,
                ctx,
                &operation,
                &[],
                Bytes::new(),
                &mut evidence,
                true,
            )
            .await
            .map_err(|failure| {
                if let Some(status) =
                    crate::wire::product::operation::raw_overflow_status(failure.error())
                {
                    if status == StatusCode::OK {
                        return page_format(&format!("body over {CAP} bytes"), &evidence);
                    }
                    return status_error(status, None);
                }
                convert(failure)
            })?;
            if reply.status != StatusCode::OK {
                return Err(status_error(reply.status, reply.clean_absence()));
            }
            let content_type = reply
                .headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !is_html(content_type) {
                let reason = if evidence.text_reflects(content_type) {
                    String::new()
                } else {
                    format!("content-type {content_type:?} is not text/html")
                };
                return Err(page_format(&reason, &evidence));
            }
            let mut items = reply
                .decode_projection(|body| Ok(ranges(&String::from_utf8_lossy(body))))
                .map_err(|error| read_failure(*error))?
                .map_err(|reason| page_format(&reason, &evidence))?;
            for range in &mut items {
                redact_text(range, &evidence);
            }
            redact_text(&mut source, &evidence);
            Ok(ListIpRangesOutput { items, source })
        })
    }
}

fn status_error<T>(
    status: StatusCode,
    absence: Option<crate::wire::product::absence::CleanAbsence>,
) -> Failure<T> {
    let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
    read_failure(
        Error::from_public_api(ApiError::new(
            OP,
            Some(status.as_u16()),
            SafeText::new("", &[]),
            SafeText::new("", &[]),
            retryable,
        ))
        .with_clean_absence(absence),
    )
}

fn page_format<T>(reason: &str, evidence: &Evidence) -> Failure<T> {
    let reflected = evidence.text_reflects(reason);
    let reason = SafeText::new(reason, &[]);
    let prefix = "cdn: IP range page format not recognized";
    let message =
        if reflected || reason.as_str().is_empty() || evidence.text_reflects(reason.as_str()) {
            prefix.to_owned()
        } else {
            format!("{prefix}: {}", reason.as_str())
        };
    let api = crate::ApiError::without_fallback(
        "",
        None,
        SafeText::new("PageFormat", &[]),
        SafeText::new(&message, &[]),
        false,
    );
    read_failure(Error::from_api(api).with_outer_detail(ErrorDetail::PageFormat))
}

fn is_html(content_type: &str) -> bool {
    let (media, parameters) = content_type.split_once(';').unwrap_or((content_type, ""));
    media.trim().eq_ignore_ascii_case("text/html")
        && parameters.split(';').all(|pair| {
            pair.trim().is_empty()
                || pair.split_once('=').is_some_and(|(name, value)| {
                    !name.trim().is_empty() && !value.trim().is_empty()
                })
        })
}

struct Tag<'a> {
    name: &'a str,
    closing: bool,
    self_closing: bool,
}

enum Part<'a> {
    Text(&'a str),
    Tag(Tag<'a>),
}

fn page_parts(mut rest: &str) -> impl Iterator<Item = Part<'_>> {
    let mut hidden = Vec::new();
    std::iter::from_fn(move || {
        while !rest.is_empty() {
            if !rest.starts_with('<') {
                let len = rest.find('<').unwrap_or(rest.len());
                let (text, next) = rest.split_at(len);
                rest = next;
                if hidden.is_empty() {
                    return Some(Part::Text(text));
                }
                continue;
            }
            rest = &rest[1..];
            if let Some(comment) = rest.strip_prefix("!--") {
                rest = comment.split_once("-->").map_or("", |(_, tail)| tail);
                continue;
            }
            if let Some(name @ ("script" | "style")) = hidden.last().copied() {
                if let Some(tail) = raw_element_end(rest, name) {
                    rest = tail;
                    hidden.pop();
                }
                continue;
            }
            let (tag, tail) = parse_tag(rest)?;
            rest = tail;
            if let Some(name) = hidden_element(tag.name) {
                if tag.closing && hidden.last() == Some(&name) {
                    hidden.pop();
                } else if !tag.closing && !tag.self_closing {
                    hidden.push(name);
                }
                continue;
            }
            if hidden.is_empty() {
                return Some(Part::Tag(tag));
            }
        }
        None
    })
}

fn hidden_element(name: &str) -> Option<&'static str> {
    ["script", "style", "noscript", "template", "svg"]
        .into_iter()
        .find(|hidden| name.eq_ignore_ascii_case(hidden))
}

fn raw_element_end<'a>(rest: &'a str, name: &str) -> Option<&'a str> {
    let content = rest.strip_prefix('/')?;
    let (tag, end) = content.split_at_checked(name.len())?;
    if !tag.eq_ignore_ascii_case(name) {
        return None;
    }
    end.trim_start_matches(|ch: char| ch.is_ascii_whitespace())
        .strip_prefix('>')
}

fn parse_tag(rest: &str) -> Option<(Tag<'_>, &str)> {
    let mut quote = None;
    let end = rest.bytes().position(|byte| match (quote, byte) {
        (Some(expected), found) if expected == found => {
            quote = None;
            false
        }
        (None, b'\'' | b'"') => {
            quote = Some(byte);
            false
        }
        (None, b'>') => true,
        _ => false,
    })?;
    let raw = &rest[..end];
    let closing = raw.starts_with('/');
    let content = raw.strip_prefix('/').unwrap_or(raw);
    let name = content
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '/')
        .next()?;
    let name = if closing && content.trim_end_matches(|ch: char| ch.is_ascii_whitespace()) != name {
        ""
    } else {
        name
    };
    Some((
        Tag {
            name,
            closing,
            self_closing: raw.ends_with('/'),
        },
        &rest[end + 1..],
    ))
}

fn level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h' | b'H', digit @ b'1'..=b'6'] => Some(*digit - b'0'),
        _ => None,
    }
}

fn heading_matches(text: &str) -> bool {
    crate::go_text::unescape_html(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
        .contains("cdn ip range")
}

fn ranges(html: &str) -> Result<Vec<String>, String> {
    let mut heading = None;
    let mut heading_text = String::new();
    let mut section = String::new();
    let mut target = None;
    let mut section_closed = false;
    let mut matches = 0;
    for part in page_parts(html) {
        let text = match part {
            Part::Text(text) => text,
            Part::Tag(tag) => {
                let level = level(tag.name);
                if !tag.closing
                    && heading.is_none()
                    && let Some(level) = level
                {
                    section_closed |= target.is_some_and(|matched| level <= matched);
                    heading = Some(level);
                }
                if tag.closing && level.is_some() && level == heading {
                    if heading_matches(&heading_text) {
                        matches += 1;
                        if target.is_none() {
                            target = level;
                        }
                    }
                    heading = None;
                    heading_text.clear();
                }
                " "
            }
        };
        if heading.is_some() {
            heading_text.push_str(text);
        }
        if target.is_some() && !section_closed {
            section.push_str(text);
        }
    }
    if heading.is_some() && heading_matches(&heading_text) {
        matches += 1;
    }
    if matches > 1 {
        return Err(format!("heading \"CDN IP range\" matched {matches} times"));
    }
    if matches == 0 {
        return Err("heading \"CDN IP range\" not found".into());
    }
    if !section_closed {
        return Err("no heading found after the matched section".into());
    }
    let decoded = crate::go_text::unescape_html(&section);
    let mut result = BTreeSet::new();
    for token in
        decoded.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | ':' | '/')))
    {
        let token = token.trim_end_matches(['.', ':']);
        let ip = token.split('/').next().unwrap_or("");
        if dotted(ip)
            || (token.contains('/') && token.bytes().filter(|byte| *byte == b':').count() >= 2)
        {
            result.insert(
                prefix(token)
                    .ok_or_else(|| format!("invalid CIDR {:?}", &token[..token.len().min(64)]))?,
            );
        } else if embedded_ipv4(token) {
            return Err(format!(
                "token {:?} holds a CIDR glued to other text",
                &token[..token.len().min(64)]
            ));
        }
    }
    if result.is_empty() {
        return Err("no CIDR found in the matched section".into());
    }
    Ok(result
        .into_iter()
        .map(|(ip, length)| format!("{ip}/{length}"))
        .collect())
}

fn dotted(address: &str) -> bool {
    let mut groups = address.split('.');
    (0..4).all(|_| {
        groups.next().is_some_and(|group| {
            !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_digit())
        })
    }) && groups.next().is_none()
}

fn embedded_ipv4(token: &str) -> bool {
    token
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .any(|run| {
            let mut groups = 0;
            run.split('.').any(|group| {
                groups = if group.is_empty() { 0 } else { groups + 1 };
                groups >= 4
            })
        })
}

fn prefix(token: &str) -> Option<(IpAddr, u8)> {
    let (address, length) = token.split_once('/')?;
    if length.is_empty()
        || (length.len() > 1 && length.starts_with('0'))
        || !length.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let length = length.parse::<u8>().ok()?;
    let address = address.parse().ok()?;
    let canonical = match address {
        IpAddr::V4(ip) if length <= 32 => ip.to_bits().trailing_zeros() >= 32 - u32::from(length),
        IpAddr::V6(ip) if length <= 128 => ip.to_bits().trailing_zeros() >= 128 - u32::from(length),
        _ => false,
    };
    canonical.then_some((address, length))
}
