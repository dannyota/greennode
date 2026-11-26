#[cfg(any(unix, test))]
use crate::ErrorClass;
use crate::wire::product::{
    sender::{BearerSource, BearerToken},
    unauthorized::ObtainedAt,
};
use crate::{Clock, Error, RequestContext, Secret, Token};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub(crate) struct ServiceAccount {
    id: String,
    secret: Secret,
    profile: String,
    directory: Option<PathBuf>,
    home: Option<PathBuf>,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    state: Mutex<State>,
    #[cfg(test)]
    endpoint: Option<(String, rustls::pki_types::CertificateDer<'static>)>,
    #[cfg(test)]
    disk_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    read_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    sync_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(unix)]
struct LiveWrite(Arc<std::sync::atomic::AtomicBool>);
#[cfg(unix)]
impl Drop for LiveWrite {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Default)]
struct State {
    cached: Option<Cached>,
    rejected: Option<Secret>,
}

struct Cached {
    token: Token,
    obtained: ObtainedAt,
    expires_monotonic: Option<tokio::time::Instant>,
}

impl Cached {
    fn bearer(&self) -> BearerToken {
        BearerToken {
            token: self.token.clone(),
            obtained_at: Some(match self.obtained {
                ObtainedAt::Disk(at) => ObtainedAt::Disk(at),
                ObtainedAt::InProcess(at) => ObtainedAt::InProcess(at),
            }),
        }
    }
    fn fresh(&self, clock: &dyn Clock) -> bool {
        !self.token.access_token.expose_secret().is_empty()
            && match self.expires_monotonic {
                Some(expiry) => expiry
                    .checked_duration_since(clock.monotonic_now())
                    .is_some_and(|remaining| remaining > Duration::from_secs(30)),
                None => self
                    .token
                    .expires_at
                    .is_some_and(|expiry| expiry - clock.utc_now() > time::Duration::seconds(30)),
            }
    }
    fn old(&self, clock: &dyn Clock) -> bool {
        match self.obtained {
            ObtainedAt::Disk(at) => {
                clock.utc_now() < at || clock.utc_now() - at >= time::Duration::seconds(30)
            }
            ObtainedAt::InProcess(at) => clock
                .monotonic_now()
                .checked_duration_since(at)
                .is_none_or(|age| age >= Duration::from_secs(30)),
        }
    }
}

impl std::fmt::Debug for ServiceAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceAccount([redacted])")
    }
}

impl ServiceAccount {
    #[cfg(test)]
    pub(crate) fn set_test_endpoint(
        &mut self,
        origin: String,
        root: rustls::pki_types::CertificateDer<'static>,
    ) {
        self.endpoint = Some((origin, root));
    }

    pub(crate) fn new(
        id: String,
        secret: Secret,
        profile: String,
        directory: Option<PathBuf>,
        home: Option<PathBuf>,
        clock: Arc<dyn Clock>,
        timeout: Duration,
    ) -> Self {
        Self {
            id,
            secret,
            profile,
            directory,
            home,
            clock,
            timeout,
            state: Mutex::new(State::default()),
            #[cfg(test)]
            endpoint: None,
            #[cfg(test)]
            disk_hook: None,
            #[cfg(test)]
            read_hook: None,
            #[cfg(test)]
            sync_hook: None,
        }
    }

    async fn exchange(
        &self,
        ctx: &RequestContext,
    ) -> Result<crate::wire::credential::service_account::TimedToken, Box<Error>> {
        #[cfg(test)]
        if let Some((url, root)) = &self.endpoint {
            return crate::wire::credential::service_account::exchange_test(
                &self.id,
                &self.secret,
                url,
                crate::wire::connection::Trust::test(root.clone()),
                ctx,
                self.clock.as_ref(),
                self.timeout,
            )
            .await;
        }
        crate::wire::credential::service_account::exchange(
            &self.id,
            &self.secret,
            ctx,
            self.clock.as_ref(),
            self.timeout,
        )
        .await
    }

    async fn acquire(&self, ctx: &RequestContext) -> Result<BearerToken, Box<Error>> {
        let clock = self.clock.as_ref();
        let mut state = bounded(ctx, clock, self.state.lock()).await?;
        if let Some(cached) = &state.cached
            && cached.fresh(clock)
        {
            if let Some(end) = ctx.check(clock) {
                return Err(Box::new(end.into()));
            }
            return Ok(cached.bearer());
        }
        #[cfg(unix)]
        if let Some(directory) = &self.directory {
            let directory = directory.clone();
            let home = self.home.clone();
            let worker_context = ctx.clone();
            let worker_clock = self.clock.clone();
            #[cfg(test)]
            let disk_hook = self.disk_hook.clone();
            let store = blocking(ctx, self.clock.clone(), move || {
                #[cfg(test)]
                if let Some(hook) = disk_hook {
                    hook();
                }
                if let Some(end) = worker_context.check(worker_clock.as_ref()) {
                    return Err(Box::new(end.into()));
                }
                let checked = crate::cache::checked_directory(home.as_deref(), &directory)?;
                let checked = crate::cache::resolve(&checked, true)
                    .map_err(|()| Box::new(Error::new(ErrorClass::TokenCache)))?;
                if let Some(end) = worker_context.check(worker_clock.as_ref()) {
                    return Err(Box::new(end.into()));
                }
                let root = crate::cache::store::Store::create_avoiding(&checked, home.as_deref())?;
                if let Some(end) = worker_context.check(worker_clock.as_ref()) {
                    return Err(Box::new(end.into()));
                }
                Ok(Arc::new(root.create_child("service-account-v1")?))
            })
            .await?;
            let stem = crate::cache::service_account_stem(
                &self.profile,
                &self.id,
                "https://iamapis.vngcloud.vn/accounts-api/v2/auth/token",
                &self.secret,
            );
            let name = format!("{stem}.json");
            let lock =
                crate::cache::lock::acquire(store.clone(), format!("{stem}.lock"), ctx, clock)
                    .await?;
            let reader = store.clone();
            let read_name = name.clone();
            let rejected = state.rejected.clone();
            let now = clock.utc_now();
            #[cfg(test)]
            let read_hook = self.read_hook.clone();
            let entry = blocking(ctx, self.clock.clone(), move || {
                let result = crate::cache::entry::read(
                    &reader,
                    &read_name,
                    crate::cache::entry::Kind::ServiceAccount,
                    now,
                    rejected.as_ref(),
                );
                #[cfg(test)]
                if let Some(hook) = read_hook {
                    hook();
                }
                result
            })
            .await?;
            if let Some(entry) = entry
                && entry.expires - clock.utc_now() > time::Duration::seconds(30)
                && !state.rejected.as_ref().is_some_and(|rejected| {
                    let age = clock.utc_now() - entry.obtained;
                    rejected.expose_secret() == entry.token.expose_secret()
                        && (age < time::Duration::ZERO || age >= time::Duration::seconds(30))
                })
            {
                let cached = Cached {
                    token: Token::new(entry.token, Some(entry.expires)),
                    obtained: ObtainedAt::Disk(entry.obtained),
                    expires_monotonic: None,
                };
                let result = cached.bearer();
                if let Some(end) = ctx.check(clock) {
                    return Err(Box::new(end.into()));
                }
                state.cached = Some(cached);
                state.rejected = None;
                return Ok(result);
            }
            {
                let timed = self.exchange(ctx).await?;
                let exchange_wall = timed.started_wall;
                let exchange_mono = timed.started_monotonic;
                let token = timed.token;
                let obtained = clock.utc_now();
                let mono = clock.monotonic_now();
                let expires = token
                    .expires_at
                    .ok_or_else(|| Error::new(ErrorClass::Auth))?;
                let entry = crate::cache::entry::Entry {
                    token: token.access_token.clone(),
                    expires,
                    obtained,
                };
                let writer_context = ctx.clone();
                let writer_clock = self.clock.clone();
                let live = LiveWrite(Arc::new(std::sync::atomic::AtomicBool::new(false)));
                let writer_live = live.0.clone();
                let writer_store = store.clone();
                let writer_name = name.clone();
                #[cfg(test)]
                let sync_hook = self.sync_hook.clone();
                let (_lock, write) = blocking(ctx, self.clock.clone(), move || {
                    #[cfg(test)]
                    let hook = sync_hook.as_deref();
                    #[cfg(not(test))]
                    let hook = None;
                    let result = crate::cache::entry::write_hook(
                        &writer_store,
                        &writer_name,
                        &entry,
                        || {
                            writer_context
                                .check(writer_clock.as_ref())
                                .map_or(Ok(()), |end| Err(Box::new(end.into())))
                        },
                        hook,
                        Some(&writer_live),
                    );
                    Ok((lock, result))
                })
                .await?;
                if let Some(end) = ctx.check(clock) {
                    return Err(Box::new(end.into()));
                }
                if let Err(error) = write
                    && error.class() != ErrorClass::TokenCache
                {
                    return Err(error);
                }
                let expires_monotonic = Duration::try_from(expires - exchange_wall)
                    .ok()
                    .and_then(|duration| exchange_mono.checked_add(duration));
                let cached = Cached {
                    token,
                    obtained: ObtainedAt::InProcess(mono),
                    expires_monotonic,
                };
                if !cached.fresh(clock)
                    || state.rejected.as_ref().is_some_and(|rejected| {
                        rejected.expose_secret() == cached.token.access_token.expose_secret()
                            && cached.old(clock)
                    })
                {
                    return Err(Box::new(Error::new(ErrorClass::Auth)));
                }
                let result = cached.bearer();
                state.cached = Some(cached);
                state.rejected = None;
                return Ok(result);
            }
        }
        let timed = self.exchange(ctx).await?;
        let exchange_wall = timed.started_wall;
        let exchange_mono = timed.started_monotonic;
        let token = timed.token;
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        let mono = clock.monotonic_now();
        let expires_monotonic = token
            .expires_at
            .and_then(|expiry| Duration::try_from(expiry - exchange_wall).ok())
            .and_then(|duration| exchange_mono.checked_add(duration));
        let cached = Cached {
            token,
            obtained: ObtainedAt::InProcess(mono),
            expires_monotonic,
        };
        let result = cached.bearer();
        state.cached = Some(cached);
        Ok(result)
    }
}

async fn bounded<T>(
    ctx: &RequestContext,
    clock: &dyn Clock,
    future: impl Future<Output = T>,
) -> Result<T, Box<Error>> {
    if let Some(end) = ctx.check(clock) {
        return Err(Box::new(end.into()));
    }
    let mut future = Box::pin(future);
    let mut ended = Box::pin(ctx.wait(clock));
    let result = std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(end) = ended.as_mut().poll(cx) {
            return std::task::Poll::Ready(Err(Box::new(end.into())));
        }
        future.as_mut().poll(cx).map(Ok)
    })
    .await?;
    if let Some(end) = ctx.check(clock) {
        return Err(Box::new(end.into()));
    }
    Ok(result)
}

#[cfg(unix)]
async fn blocking<T: Send + 'static>(
    ctx: &RequestContext,
    clock: Arc<dyn Clock>,
    work: impl FnOnce() -> Result<T, Box<Error>> + Send + 'static,
) -> Result<T, Box<Error>> {
    struct Abort<T>(tokio::task::JoinHandle<T>);
    impl<T> Drop for Abort<T> {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    if let Some(end) = ctx.check(clock.as_ref()) {
        return Err(Box::new(end.into()));
    }
    let mut task = Abort(tokio::task::spawn_blocking(work));
    bounded(ctx, clock.as_ref(), &mut task.0)
        .await?
        .map_err(|_| Box::new(Error::new(ErrorClass::TokenCache)))?
}

impl BearerSource for ServiceAccount {
    fn token<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, Error>> + Send + 'a>> {
        Box::pin(async move { self.acquire(ctx).await.map_err(|error| *error) })
    }
    fn rejected<'a>(
        &'a self,
        ctx: &'a RequestContext,
        sent: &'a Secret,
        evict: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let clock = self.clock.as_ref();
            let mut state = bounded(ctx, clock, self.state.lock())
                .await
                .map_err(|error| *error)?;
            if evict
                && !sent.expose_secret().is_empty()
                && state.cached.as_ref().is_some_and(|cached| {
                    cached.token.access_token.expose_secret() == sent.expose_secret()
                        && cached.old(clock)
                })
            {
                if let Some(end) = ctx.check(clock) {
                    return Err(end.into());
                }
                state.cached = None;
                if cfg!(unix) && self.directory.is_some() {
                    state.rejected = Some(sent.clone());
                }
            }
            if let Some(end) = ctx.check(clock) {
                return Err(end.into());
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
