pub(crate) const CLIENT_ID: &str = "c9e78411-f2a2-41ba-a9e4-3c56263c181a";

pub(crate) mod flow;
mod signin;
mod token;
pub(crate) mod totp;

use crate::Secret;
use crate::wire::product::sender::SentCredentials;
use data_encoding::BASE64URL_NOPAD;
use sha2::{Digest, Sha256};

fn is_two_factor_path(path: &str) -> bool {
    path.contains("/ap/auth/iam/google")
}

fn challenge(verifier: &str) -> String {
    BASE64URL_NOPAD.encode(&Sha256::digest(verifier.as_bytes()))
}

fn from_bytes(bytes: &[u8; 32]) -> (Secret, String) {
    let verifier = Secret::new(BASE64URL_NOPAD.encode(bytes));
    let challenge = challenge(verifier.expose_secret());
    (verifier, challenge)
}

fn pkce(rng: &mut (dyn FnMut(&mut [u8]) -> Result<(), ()> + Send)) -> Result<(Secret, String), ()> {
    let mut bytes = zeroize::Zeroizing::new([0; 32]);
    rng(&mut bytes[..])?;
    Ok(from_bytes(&bytes))
}

#[derive(Debug)]
struct StoredCookie {
    name: String,
    value: Secret,
    path: String,
    secure: bool,
    expires: Option<time::OffsetDateTime>,
}

#[derive(Debug)]
struct CookieJar {
    origin: url::Origin,
    cookies: Vec<StoredCookie>,
    sent: SentCredentials,
}

impl CookieJar {
    fn new(origin: url::Url) -> Self {
        Self {
            origin: origin.origin(),
            cookies: Vec::new(),
            sent: SentCredentials::default(),
        }
    }

    fn store(&mut self, url: &url::Url, header: &str, clock: &dyn crate::Clock) {
        let now = clock.utc_now();
        if url.origin() != self.origin {
            return;
        }
        let Ok(cookie) = cookie::Cookie::parse(header) else {
            return;
        };
        let value = cookie.value();
        let (value_to_validate, quoted) =
            match value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                Some(inner) => (inner, true),
                None => (value, false),
            };
        if !cookie
            .name()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || cookie.name().is_empty()
            || !value_to_validate.bytes().all(|b| {
                if quoted {
                    (0x20..0x7f).contains(&b) && !matches!(b, b'"' | b';' | b'\\')
                } else {
                    matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
                }
            })
        {
            return;
        }
        if let Some(domain) = cookie.domain() {
            let domain = domain.trim_start_matches('.');
            let Some(host) = url.host_str() else {
                return;
            };
            if !host.eq_ignore_ascii_case(domain) {
                return;
            }
        }
        let path = cookie
            .path()
            .filter(|path| path.starts_with('/'))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let path = url.path();
                match path.rfind('/') {
                    Some(index) if index > 0 => path[..index].to_owned(),
                    _ => "/".to_owned(),
                }
            });
        let expires = if let Some(age) = cookie.max_age() {
            Some(now.saturating_add(age))
        } else {
            cookie.expires_datetime()
        };
        self.cookies
            .retain(|stored| stored.name != cookie.name() || stored.path != path);
        if expires.is_some_and(|expiry| expiry <= now) {
            return;
        }
        self.cookies.push(StoredCookie {
            name: cookie.name().to_owned(),
            value: Secret::new(cookie.value()),
            path,
            secure: cookie.secure().unwrap_or(false),
            expires,
        });
    }

    fn record_form(&mut self, value: &Secret) {
        self.sent.record(value);
        let encoded: String =
            url::form_urlencoded::byte_serialize(value.expose_secret().as_bytes()).collect();
        self.sent.record(&Secret::new(encoded));
    }

    fn header(&mut self, url: &url::Url, clock: &dyn crate::Clock) -> Option<Secret> {
        let now = clock.utc_now();
        if url.origin() != self.origin {
            return None;
        }
        let mut matches: Vec<_> = self
            .cookies
            .iter()
            .filter(|cookie| {
                (!cookie.secure || url.scheme() == "https")
                    && cookie.expires.is_none_or(|expires| expires > now)
                    && path_matches(&cookie.path, url.path())
            })
            .collect();
        matches.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        if matches.is_empty() {
            return None;
        }
        for cookie in &matches {
            let value = cookie.value.expose_secret();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            self.sent.record(&Secret::new(unquoted));
        }
        Some(Secret::new(
            matches
                .iter()
                .map(|cookie| format!("{}={}", cookie.name, cookie.value.expose_secret()))
                .collect::<Vec<_>>()
                .join("; "),
        ))
    }
}

fn path_matches(cookie: &str, request: &str) -> bool {
    request == cookie
        || request
            .strip_prefix(cookie)
            .is_some_and(|suffix| cookie.ends_with('/') || suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }
    fn now() -> crate::TestClock {
        crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH)
    }

    fn later() -> crate::TestClock {
        crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1))
    }

    #[test]
    fn origin_is_restricted() {
        let base = url("https://example.test/a");
        let other = url("https://other.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(&other, "foreign=x; Path=/", &now());
        assert!(jar.header(&base, &now()).is_none());
        jar.store(&base, "local=y; Path=/", &now());
        assert!(jar.header(&base, &now()).is_some());
        assert!(jar.header(&other, &now()).is_none());
    }

    #[test]
    fn secure_only_over_https() {
        let base = url("http://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(&base, "secure=x; Secure", &now());
        assert!(jar.header(&base, &now()).is_none());
        jar.store(&base, "plain=y", &now());
        assert!(jar.header(&base, &now()).is_some());
    }

    #[test]
    fn expiry_is_enforced() {
        let base = url("https://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(&base, "old=x; Max-Age=1", &now());
        assert!(jar.header(&base, &now()).is_some());
        assert!(jar.header(&base, &later()).is_none());
    }

    #[test]
    fn path_is_enforced() {
        let base = url("https://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(&base, "path=x; Path=/a", &now());
        assert!(jar.header(&base, &now()).is_some());
        assert!(
            jar.header(&url("https://example.test/ab"), &now())
                .is_none()
        );
    }

    #[test]
    fn max_age_precedes_expires() {
        let base = url("https://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(
            &base,
            "age=x; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=1",
            &now(),
        );
        assert!(jar.header(&base, &now()).is_some());
        assert!(jar.header(&base, &later()).is_none());
    }

    #[test]
    fn debug_redacts_cookie() {
        let base = url("https://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(&base, "name=canary-cookie-value", &now());
        assert!(jar.header(&base, &now()).is_some());
        assert!(!format!("{jar:?}").contains("canary-cookie-value"));
    }

    #[test]
    fn expires_attribute_is_enforced() {
        let base = url("https://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(
            &base,
            "old=x; Expires=Thu, 01 Jan 1970 00:00:01 GMT",
            &now(),
        );
        assert!(jar.header(&base, &now()).is_some());
        assert!(jar.header(&base, &later()).is_none());
    }

    #[test]
    fn domain_must_be_exact_host() {
        let base = url("https://a.example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(&base, "parent=x; Domain=example.test", &now());
        jar.store(&base, "sibling=x; Domain=b.example.test", &now());
        assert!(jar.header(&base, &now()).is_none());
        jar.store(&base, "local=y; Domain=A.EXAMPLE.TEST", &now());
        assert!(jar.header(&base, &now()).is_some());
    }

    #[test]
    fn quoted_cookie_rejects_invalid_enclosed_octets() {
        let base = url("https://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        for bad in [
            "session=\"bad\\value\"",
            "session=\"bad\"value\"",
            "session=\"bad\u{7f}value\"",
        ] {
            jar.store(&base, bad, &now());
            assert!(jar.header(&base, &now()).is_none());
        }
        jar.store(&base, "session=\"hello world,ok\"", &now());
        assert!(jar.header(&base, &now()).is_some());
    }

    #[test]
    fn malformed_cookie_is_dropped() {
        let base = url("https://example.test/a");
        let mut jar = CookieJar::new(base.clone());
        jar.store(&base, "bad\r\nname=x", &now());
        assert!(jar.header(&base, &now()).is_none());
        jar.store(&base, "good=y", &now());
        assert!(jar.header(&base, &now()).is_some());
    }

    #[test]
    fn rfc_challenge() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_matches_baseline_encoding() {
        let (verifier, _) = pkce(&mut |bytes| {
            bytes.fill(0xff);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            verifier.expose_secret(),
            "__________________________________________8"
        );
    }
}
