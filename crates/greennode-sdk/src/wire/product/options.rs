use crate::{Clock, RequestContext, Secret};
use std::time::Duration;
use tokio::{sync::Mutex, time::Instant};

#[derive(Clone, Copy, Debug)]
pub(crate) enum PasswordSlot<'a> {
    DatabaseUser(&'a Secret),
    IamUser(&'a Secret),
}

#[derive(Debug)]
pub(crate) struct Pacing {
    interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl Pacing {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(None),
        }
    }

    pub(super) async fn before_send(
        &self,
        ctx: &RequestContext,
        clock: &dyn Clock,
    ) -> Result<(), crate::ContextEnd> {
        let mut last = self.last.lock().await;
        if let Some(previous) = *last {
            let pause = ctx.child_with_deadline(previous + self.interval);
            let _ = pause.wait(clock).await;
        }
        if let Some(end) = ctx.check(clock) {
            return Err(end);
        }
        *last = Some(clock.monotonic_now());
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PathIds {
    #[default]
    Strict,
    WithUnderscore,
}

impl PathIds {
    pub(crate) fn allows(self, id: &str) -> bool {
        !id.is_empty()
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || byte == b'-'
                    || (byte == b'_' && self == Self::WithUnderscore)
            })
    }
}

pub(crate) type AttemptCheckFn<'a> =
    &'a (dyn Fn(&super::evidence::SentCredentials) -> Option<Box<crate::Error>> + Sync);

#[derive(Clone, Copy)]
pub(crate) struct AttemptCheck<'a>(pub(crate) AttemptCheckFn<'a>);

impl std::fmt::Debug for AttemptCheck<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AttemptCheck")
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options<'a> {
    pub(crate) path_ids: PathIds,
    pub(crate) identifier_query_keys: &'a [&'a str],
    pub(crate) pacing: Option<&'a Pacing>,
    pub(crate) project_discovery: bool,
    pub(crate) response_guard: Option<super::sender::ResponseGuard>,
    pub(crate) password_slot: Option<PasswordSlot<'a>>,
    pub(crate) headers: &'a [(&'static str, &'a str)],
    pub(crate) attempt_check: Option<AttemptCheck<'a>>,
    pub(crate) receipt_policy: super::guard::ReceiptPolicy,
}

impl Options<'_> {
    pub(crate) const DEFAULT: Self = Self {
        path_ids: PathIds::Strict,
        identifier_query_keys: &[],
        pacing: None,
        project_discovery: false,
        response_guard: None,
        password_slot: None,
        headers: &[],
        attempt_check: None,
        receipt_policy: super::guard::ReceiptPolicy::All,
    };

    pub(crate) fn apply_headers(&self, headers: &mut http::HeaderMap) -> Result<(), ()> {
        for (name, value) in self.headers {
            let valid = match *name {
                "portal-user-id" => {
                    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
                }
                "user-type" => PathIds::WithUnderscore.allows(value),
                "region" | "region_id" => PathIds::Strict.allows(value),
                _ => false,
            };
            if value.len() > 128 || !valid {
                return Err(());
            }
            let name = http::HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?;
            let value = http::HeaderValue::from_str(value).map_err(|_| ())?;
            if headers.contains_key(&name) {
                return Err(());
            }
            headers.insert(name, value);
        }
        Ok(())
    }
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Options;

    #[test]
    fn portal_user_id_requires_ascii_digits() {
        let mut headers = http::HeaderMap::new();
        for rejected in ["", "-7", "7a", " 7", "７"] {
            let options = Options {
                headers: &[("portal-user-id", rejected)],
                ..Options::DEFAULT
            };
            assert!(options.apply_headers(&mut headers).is_err());
            assert!(headers.is_empty());
        }
        let options = Options {
            headers: &[("portal-user-id", "123")],
            ..Options::DEFAULT
        };
        assert!(options.apply_headers(&mut headers).is_ok());
        assert_eq!(headers["portal-user-id"], "123");
    }
}
