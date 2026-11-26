use std::{collections::BTreeSet, net::IpAddr};

use bytes::Bytes;
use http::{Method, StatusCode, header::CONTENT_TYPE};
use serde::Serialize;

use super::models::redact_text;
use super::wire::{api_error, convert, operation, raw, read_failure};
use super::{Client, ErrorDetail};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext};

const OP: &str = "cdn.ListIPRanges";
const CAP: usize = 4 << 20;

/// Input for reading the public CDN IP ranges.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListIpRangesInput;

/// The canonical IP prefixes and their source.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListIpRangesOutput {
    /// Canonical IP ranges sorted by IP address and prefix length.
    pub items: Vec<String>,
    /// The configured public documentation URL.
    pub source: String,
}

impl Client {
    /// Reads the public CDN IP ranges without using account credentials.
    pub async fn list_ip_ranges(
        &self,
        ctx: &RequestContext,
        _: ListIpRangesInput,
    ) -> Result<ListIpRangesOutput, Failure<ListIpRangesOutput>> {
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
            if failure.error().is_body_too_large()
                && failure.error().detail::<StatusCode>() == Some(&StatusCode::OK)
            {
                return page_format(&format!("body over {CAP} bytes"), &evidence);
            }
            let status = failure.error().detail::<StatusCode>().copied().or_else(|| {
                failure
                    .error()
                    .api_error()
                    .and_then(crate::ApiError::status)
                    .and_then(|status| StatusCode::from_u16(status).ok())
            });
            if let Some(status) = status
                && status != StatusCode::OK
            {
                return status_error(status);
            }
            convert(failure)
        })?;
        if reply.status != StatusCode::OK {
            return Err(status_error(reply.status));
        }
        let content_type = reply
            .headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !is_html(content_type) {
            return Err(page_format(
                &format!("content-type {content_type:?} is not text/html"),
                &evidence,
            ));
        }
        let mut items = ranges(&String::from_utf8_lossy(&reply.body))
            .map_err(|reason| page_format(&reason, &evidence))?;
        for range in &mut items {
            redact_text(range, &evidence);
        }
        redact_text(&mut source, &evidence);
        Ok(ListIpRangesOutput { items, source })
    }
}

fn status_error<T>(status: StatusCode) -> Failure<T> {
    let class = match status {
        StatusCode::FORBIDDEN => ErrorClass::PermissionDenied,
        StatusCode::NOT_FOUND => ErrorClass::NotFound,
        StatusCode::TOO_MANY_REQUESTS => ErrorClass::RateLimited,
        _ => ErrorClass::Api,
    };
    let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
    let message = match class {
        ErrorClass::PermissionDenied => "greennode: permission denied".to_owned(),
        ErrorClass::NotFound => "greennode: resource not found".to_owned(),
        ErrorClass::RateLimited => "greennode: rate limited".to_owned(),
        _ => format!("request failed with status {}", status.as_u16()),
    };
    read_failure(api_error(
        OP,
        status.as_u16(),
        "",
        &message,
        class,
        retryable,
    ))
}

fn page_format<T>(reason: &str, evidence: &Evidence) -> Failure<T> {
    use crate::error::SafeText;
    let secrets: Vec<_> = evidence.credentials().values().iter().collect();
    let reason = SafeText::new(reason, &secrets);
    let prefix = "cdn: IP range page format not recognized";
    let message = if reason.as_str().is_empty() {
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
    start: usize,
    end: usize,
    closing: bool,
    self_closing: bool,
}

fn tags(html: &str) -> impl Iterator<Item = Tag<'_>> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        let start = pos + html[pos..].find('<')?;
        let (tag, end) = parse_tag(html, start)?;
        pos = end;
        Some(tag)
    })
}

fn parse_tag(html: &str, start: usize) -> Option<(Tag<'_>, usize)> {
    let tail = &html[start + 1..];
    if let Some(comment) = tail.strip_prefix("!--") {
        let end = comment
            .find("-->")
            .map_or(html.len(), |end| start + end + 7);
        return Some((
            Tag {
                name: "!--",
                start,
                end,
                closing: false,
                self_closing: true,
            },
            end,
        ));
    }
    let mut quote = None;
    let end = tail.bytes().position(|byte| match (quote, byte) {
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
    let raw = &tail[..end];
    let closing = raw.starts_with('/');
    let name = raw
        .trim_start_matches('/')
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '/')
        .next()?;
    let end = start + end + 2;
    Some((
        Tag {
            name,
            start,
            end,
            closing,
            self_closing: raw.ends_with('/'),
        },
        end,
    ))
}

fn visible(html: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;
    while let Some(tag) = tags(&html[pos..]).find(|tag| {
        !tag.closing
            && (tag.name == "!--"
                || ["script", "style", "noscript", "template", "svg"]
                    .iter()
                    .any(|name| tag.name.eq_ignore_ascii_case(name)))
    }) {
        result.push_str(&html[pos..pos + tag.start]);
        pos += tag.end;
        if !tag.self_closing {
            let close = html[pos..].match_indices("</").find_map(|(start, _)| {
                let text = &html[pos + start + 2..];
                text.get(..tag.name.len())
                    .filter(|name| name.eq_ignore_ascii_case(tag.name))?;
                let next = *text.as_bytes().get(tag.name.len())?;
                if !matches!(next, b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r' | 12) {
                    return None;
                }
                parse_tag(html, pos + start).map(|(_, end)| end)
            });
            let Some(close) = close else { return result };
            pos = close;
        }
    }
    result.push_str(&html[pos..]);
    result
}

fn text_of(html: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;
    for tag in tags(html) {
        decode_text(&html[pos..tag.start], &mut result);
        result.push(' ');
        pos = tag.end;
    }
    let tail = &html[pos..];
    decode_text(tail.split('<').next().unwrap_or(""), &mut result);
    result
}

fn decode_text(mut text: &str, output: &mut String) {
    while let Some((before, after)) = text.split_once('&') {
        output.push_str(before);
        let end = after
            .bytes()
            .take(32)
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'#')
            .count();
        let name = &after[..end];
        let semicolon = after.as_bytes().get(end) == Some(&b';');
        if let Some(decoded) = entity(name, semicolon) {
            output.push(decoded);
            text = &after[end + usize::from(semicolon)..];
        } else {
            output.push('&');
            text = after;
        }
    }
    output.push_str(text);
}

fn entity(name: &str, semicolon: bool) -> Option<char> {
    if let Some(value) = name.strip_prefix('#') {
        let number = if let Some(hex) = value.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16)
        } else {
            value.parse()
        };
        return number
            .ok()
            .map(|number| char::from_u32(number).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    match (name, semicolon) {
        ("quot", _) => Some('"'),
        ("apos", true) => Some('\''),
        ("amp", _) => Some('&'),
        ("lt", _) => Some('<'),
        ("gt", _) => Some('>'),
        ("nbsp", _) | ("Tab" | "NewLine" | "ensp" | "emsp" | "thinsp", true) => Some(' '),
        ("period", true) => Some('.'),
        ("sol", true) => Some('/'),
        ("colon", true) => Some(':'),
        _ => None,
    }
}

fn level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h' | b'H', digit @ b'1'..=b'6'] => Some(*digit - b'0'),
        _ => None,
    }
}

fn ranges(html: &str) -> Result<Vec<String>, String> {
    let html = visible(html);
    let mut headings = tags(&html).filter(|tag| level(tag.name).is_some());
    let mut section = None;
    let mut end = None;
    let mut matches = 0;
    while let Some(heading) = headings.next() {
        if heading.closing {
            continue;
        }
        let Some(current) = level(heading.name) else {
            continue;
        };
        if let Some((_, matched)) = section
            && current <= matched
            && end.is_none()
        {
            end = Some(heading.start);
        }
        let close = headings.find(|tag| tag.closing && level(tag.name) == Some(current));
        let text_end = close.as_ref().map_or(html.len(), |tag| tag.start);
        let text = text_of(&html[heading.end..text_end]);
        let normal = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if normal.contains("cdn ip range") {
            matches += 1;
            if section.is_none() {
                section = Some((close.map_or(html.len(), |tag| tag.end), current));
            }
        }
    }
    if matches > 1 {
        return Err(format!("heading \"CDN IP range\" matched {matches} times"));
    }
    let (start, _) = section.ok_or("heading \"CDN IP range\" not found")?;
    let end = end.ok_or("no heading found after the matched section")?;
    let mut result = BTreeSet::new();
    for token in text_of(&html[start..end])
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | ':' | '/')))
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
