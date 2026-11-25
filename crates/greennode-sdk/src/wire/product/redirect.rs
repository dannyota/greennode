use http::Method;
use std::{collections::HashSet, fmt};
use url::Url;

#[derive(Clone, Copy)]
pub(crate) enum Classification {
    Read,
    Write,
}
#[derive(Clone, Copy)]
pub(crate) enum Retry {
    Repeatable,
    NonIdempotent,
    Once,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Operation,
    Limit,
    Invalid,
    Origin,
    Userinfo,
    Fragment,
    Cycle,
}
impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "redirect refused: {self:?}")
    }
}

pub(crate) struct Operation<'a> {
    pub(crate) class: Classification,
    pub(crate) method: &'a Method,
    pub(crate) retry: Retry,
    pub(crate) no_redirect: bool,
}

pub(crate) fn decide(
    current: &Url,
    operation: Operation<'_>,
    hops: usize,
    visited: &HashSet<Url>,
    location: &str,
) -> Result<Url, Refusal> {
    if !matches!(operation.class, Classification::Read) {
        return Err(Refusal::Operation);
    }
    if !matches!(*operation.method, Method::GET | Method::HEAD) {
        return Err(Refusal::Operation);
    }
    if matches!(operation.retry, Retry::Once) {
        return Err(Refusal::Operation);
    }
    if operation.no_redirect {
        return Err(Refusal::Operation);
    }
    if hops >= 9 {
        return Err(Refusal::Limit);
    }
    if location.contains('#') {
        return Err(Refusal::Fragment);
    }
    if location
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b'\\')
    {
        return Err(Refusal::Invalid);
    }
    let next = current.join(location).map_err(|_| Refusal::Invalid)?;
    if current.scheme() != next.scheme() {
        return Err(Refusal::Origin);
    }
    if current.host() != next.host() {
        return Err(Refusal::Origin);
    }
    if current.port_or_known_default() != next.port_or_known_default() {
        return Err(Refusal::Origin);
    }
    if !next.username().is_empty() || next.password().is_some() {
        return Err(Refusal::Userinfo);
    }
    let raw = location.split('?').next().ok_or(Refusal::Invalid)?;
    let first = raw.split('/').next().ok_or(Refusal::Invalid)?;
    let reference = if let Some((scheme, rest)) = first.split_once(':') {
        if !scheme
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic())
            || !scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
            || !rest.is_empty()
            || !raw.starts_with(&format!("{scheme}://"))
        {
            return Err(Refusal::Invalid);
        }
        &raw[scheme.len() + 1..]
    } else {
        raw
    };
    let path = if let Some(authority_path) = reference.strip_prefix("//") {
        let end = authority_path.find('/').unwrap_or(authority_path.len());
        let authority = &authority_path[..end];
        if authority.contains('%') {
            return Err(Refusal::Invalid);
        }
        crate::endpoint::Base::parse(&format!("https://{authority}"))
            .map_err(|_| Refusal::Invalid)?;
        &authority_path[end..]
    } else {
        reference
    };
    let path = path
        .split('/')
        .filter(|segment| *segment != "." && *segment != "..")
        .collect::<Vec<_>>()
        .join("/");
    crate::endpoint::Base::parse(&format!("https://example.test/{path}"))
        .map_err(|_| Refusal::Invalid)?;
    let origin_and_path = &next[..url::Position::AfterPath];
    crate::endpoint::Base::parse(origin_and_path).map_err(|_| Refusal::Invalid)?;
    if visited.contains(&next) || next == *current {
        return Err(Refusal::Cycle);
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(
        class: Classification,
        method: Method,
        retry: Retry,
        hops: usize,
        visited: &[&str],
        location: &str,
    ) -> Result<Url, Refusal> {
        let current = Url::parse("https://example.test/a/start").unwrap();
        let visited = visited.iter().map(|s| Url::parse(s).unwrap()).collect();
        decide(
            &current,
            Operation {
                class,
                method: &method,
                retry,
                no_redirect: false,
            },
            hops,
            &visited,
            location,
        )
    }
    fn read(location: &str) -> Result<Url, Refusal> {
        run(
            Classification::Read,
            Method::GET,
            Retry::Repeatable,
            0,
            &[],
            location,
        )
    }
    #[test]
    fn opaque_reference() {
        assert!(read("https:next").is_err());
    }
    #[test]
    fn colon_in_path() {
        assert_eq!(
            read("/mirror/https://example.test/next").unwrap().as_str(),
            "https://example.test/mirror/https://example.test/next"
        );
    }
    #[test]
    fn network_path_dots() {
        assert_eq!(
            read("//example.test/a/../next").unwrap().as_str(),
            "https://example.test/next"
        );
    }
    #[test]
    fn escaped_host() {
        assert!(read("https://%65xample.test/next").is_err());
    }
    #[test]
    fn relative_and_absolute() {
        assert_eq!(
            read("../next?q=1").unwrap().as_str(),
            "https://example.test/next?q=1"
        );
        assert_eq!(
            read("https://example.test:443/next").unwrap().as_str(),
            "https://example.test/next"
        );
    }
    #[test]
    fn scheme() {
        assert!(read("https://example.test/ok").is_ok());
        assert_eq!(read("http://example.test:443/bad"), Err(Refusal::Origin));
    }
    #[test]
    fn host() {
        assert!(read("https://example.test/ok").is_ok());
        assert!(read("https://other.test/bad").is_err());
    }
    #[test]
    fn port() {
        assert!(read("https://example.test:443/ok").is_ok());
        assert!(read("https://example.test:444/bad").is_err());
    }
    #[test]
    fn userinfo() {
        assert!(read("/ok").is_ok());
        assert_eq!(
            read("https://user@example.test/bad"),
            Err(Refusal::Userinfo)
        );
    }
    #[test]
    fn fragment() {
        assert!(read("/ok").is_ok());
        assert_eq!(read("/bad#secret"), Err(Refusal::Fragment));
    }
    #[test]
    fn cycle() {
        assert!(
            run(
                Classification::Read,
                Method::GET,
                Retry::Repeatable,
                0,
                &["https://example.test/other"],
                "/ok"
            )
            .is_ok()
        );
        assert!(
            run(
                Classification::Read,
                Method::GET,
                Retry::Repeatable,
                0,
                &["https://example.test/bad"],
                "/bad"
            )
            .is_err()
        );
    }
    #[test]
    fn hop_limit() {
        assert!(
            run(
                Classification::Read,
                Method::GET,
                Retry::Repeatable,
                8,
                &[],
                "/ok"
            )
            .is_ok()
        );
        assert!(
            run(
                Classification::Read,
                Method::GET,
                Retry::Repeatable,
                9,
                &[],
                "/bad"
            )
            .is_err()
        );
    }
    #[test]
    fn write() {
        assert!(
            run(
                Classification::Read,
                Method::GET,
                Retry::Repeatable,
                0,
                &[],
                "/ok"
            )
            .is_ok()
        );
        assert!(
            run(
                Classification::Write,
                Method::GET,
                Retry::Repeatable,
                0,
                &[],
                "/bad"
            )
            .is_err()
        );
    }
    #[test]
    fn method() {
        assert!(
            run(
                Classification::Read,
                Method::HEAD,
                Retry::Repeatable,
                0,
                &[],
                "/ok"
            )
            .is_ok()
        );
        assert!(
            run(
                Classification::Read,
                Method::POST,
                Retry::Repeatable,
                0,
                &[],
                "/bad"
            )
            .is_err()
        );
    }
    #[test]
    fn once() {
        assert!(
            run(
                Classification::Read,
                Method::GET,
                Retry::NonIdempotent,
                0,
                &[],
                "/ok"
            )
            .is_ok()
        );
        assert!(
            run(
                Classification::Read,
                Method::GET,
                Retry::Once,
                0,
                &[],
                "/bad"
            )
            .is_err()
        );
    }
    #[test]
    fn no_redirect() {
        let current = Url::parse("https://example.test/a/start").unwrap();
        let visited = HashSet::new();
        let operation = |no_redirect| Operation {
            class: Classification::Read,
            method: &Method::GET,
            retry: Retry::Repeatable,
            no_redirect,
        };
        assert!(decide(&current, operation(false), 0, &visited, "/ok").is_ok());
        assert!(decide(&current, operation(true), 0, &visited, "/bad").is_err());
    }
    #[test]
    fn malformed() {
        assert!(read("/ok%20path").is_ok());
        assert!(read("/bad%zz").is_err());
    }
    #[test]
    fn refusal_hides_location() {
        let secret = "/sensitive-location";
        let errors = [
            run(
                Classification::Write,
                Method::GET,
                Retry::Repeatable,
                0,
                &[],
                secret,
            )
            .unwrap_err(),
            run(
                Classification::Read,
                Method::GET,
                Retry::Repeatable,
                9,
                &[],
                secret,
            )
            .unwrap_err(),
            read("/sensitive-location#fragment").unwrap_err(),
            read("/sensitive-location%zz").unwrap_err(),
            read("https://user@example.test/sensitive-location").unwrap_err(),
            read("https://other.test/sensitive-location").unwrap_err(),
            run(
                Classification::Read,
                Method::GET,
                Retry::Repeatable,
                0,
                &["https://example.test/sensitive-location"],
                secret,
            )
            .unwrap_err(),
        ];
        for error in errors {
            assert!(!format!("{error:?}").contains("sensitive-location"));
            assert!(!error.to_string().contains("sensitive-location"));
        }
    }
}
