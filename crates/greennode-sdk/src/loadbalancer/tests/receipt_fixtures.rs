use super::*;
use crate::{CredentialsProvider, Error, RequestContext, Token};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct RotatingBearer {
    attempts: AtomicUsize,
    rotates_at: usize,
}

impl CredentialsProvider for RotatingBearer {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        let token = if self.attempts.fetch_add(1, Ordering::Relaxed) >= self.rotates_at {
            "confirmation-bearer"
        } else {
            "initial-bearer"
        };
        Box::pin(async move { Ok(Token::new(Secret::new(token), None)) })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

pub(super) fn rotating_client(server: &Server, rotates_at: usize) -> Client {
    let clock = crate::testing::clock_advancing_on_sleep(
        crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
        std::time::Duration::from_secs(5),
    );
    client_with_options(
        server,
        CANARY,
        ConfigOptions::default()
            .with_credentials_provider(Arc::new(RotatingBearer {
                attempts: 0.into(),
                rotates_at,
            }))
            .with_clock(Arc::new(clock)),
    )
}
