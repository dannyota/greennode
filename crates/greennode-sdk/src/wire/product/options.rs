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

const ALLOWED_HEADERS: &[&str] = &["user-type", "region", "region_id"];

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options<'a> {
    pub(crate) path_ids: PathIds,
    pub(crate) pacing: Option<&'a Pacing>,
    pub(crate) response_guard: Option<super::sender::ResponseGuard>,
    pub(crate) password_slot: Option<PasswordSlot<'a>>,
    pub(crate) headers: &'a [(&'static str, &'a str)],
}

impl Options<'_> {
    pub(crate) const DEFAULT: Self = Self {
        path_ids: PathIds::Strict,
        pacing: None,
        response_guard: None,
        password_slot: None,
        headers: &[],
    };

    pub(crate) fn apply_headers(&self, headers: &mut http::HeaderMap) -> Result<(), ()> {
        for (name, value) in self.headers {
            if !ALLOWED_HEADERS.contains(name)
                || value.len() > 128
                || !(if *name == "user-type" {
                    PathIds::WithUnderscore
                } else {
                    PathIds::Strict
                })
                .allows(value)
            {
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
