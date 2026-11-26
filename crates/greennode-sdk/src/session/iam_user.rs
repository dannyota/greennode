use crate::wire::credential::iam_user::{flow, totp};
use crate::wire::product::{
    sender::{BearerSource, BearerToken},
    unauthorized::ObtainedAt,
};
use crate::{Clock, Error, ErrorClass, RequestContext, Secret, Token};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub(crate) type TotpProvider =
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Secret, ()>> + Send>> + Send + Sync;

pub(crate) struct Options {
    pub(crate) root_email: String,
    pub(crate) username: String,
    pub(crate) password: Secret,
    pub(crate) seed: Option<Secret>,
    pub(crate) provider: Option<Arc<TotpProvider>>,
    pub(crate) profile: String,
    pub(crate) directory: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) signin: url::Url,
    pub(crate) dashboard: url::Url,
    pub(crate) token_url: url::Url,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) timeout: Duration,
}

pub(crate) struct IamUser {
    root_email: String,
    username: String,
    password: Secret,
    seed: Option<Secret>,
    provider: Option<Arc<TotpProvider>>,
    profile: String,
    directory: Option<PathBuf>,
    home: Option<PathBuf>,
    signin: url::Url,
    dashboard: url::Url,
    token_url: url::Url,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    state: Mutex<State>,
    last_step: Arc<std::sync::Mutex<Option<i64>>>,
    #[cfg(test)]
    roots: Option<Vec<rustls::pki_types::CertificateDer<'static>>>,
}

#[derive(Default)]
struct State {
    cached: Option<Cached>,
    rejected: Option<Secret>,
    failure: Option<(time::OffsetDateTime, &'static str, Option<u16>, bool)>,
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
    fn reject(&self, sent: &Secret, clock: &dyn Clock) -> bool {
        !sent.expose_secret().is_empty()
            && sent.expose_secret() == self.token.access_token.expose_secret()
            && self.old(clock)
    }
}

impl std::fmt::Debug for IamUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IamUser([redacted])")
    }
}

impl IamUser {
    pub(crate) fn new(options: Options) -> Self {
        let Options {
            root_email,
            username,
            password,
            seed,
            provider,
            profile,
            directory,
            home,
            signin,
            dashboard,
            token_url,
            clock,
            timeout,
        } = options;
        Self {
            root_email,
            username,
            password,
            seed,
            provider,
            profile,
            directory,
            home,
            signin,
            dashboard,
            token_url,
            clock,
            timeout,
            state: Mutex::new(State::default()),
            last_step: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            roots: None,
        }
    }

    async fn acquire(&self, ctx: &RequestContext) -> Result<BearerToken, Box<Error>> {
        let clock = self.clock.as_ref();
        let mut state = bounded(ctx, clock, self.state.lock()).await?;
        if let Some(cached) = &state.cached
            && cached.fresh(clock)
        {
            return Ok(cached.bearer());
        }
        #[cfg(unix)]
        if let Some(directory) = &self.directory {
            let directory = directory.clone();
            let home = self.home.clone();
            let context = ctx.clone();
            let worker_clock = self.clock.clone();
            let store = blocking(ctx, self.clock.clone(), move || {
                if let Some(end) = context.check(worker_clock.as_ref()) {
                    return Err(Box::new(end.into()));
                }
                let checked = crate::cache::checked_directory(home.as_deref(), &directory)?;
                let checked = crate::cache::resolve(&checked, true)
                    .map_err(|()| Box::new(Error::new(ErrorClass::TokenCache)))?;
                Ok(Arc::new(crate::cache::store::Store::create_avoiding(
                    &checked,
                    home.as_deref(),
                )?))
            })
            .await?;
            let stem = crate::cache::iam_stem(
                &self.profile,
                &self.root_email,
                &self.username,
                self.signin.as_str().trim_end_matches('/'),
                self.token_url.as_str(),
            );
            let name = format!("{stem}.json");
            let lock = Arc::new(
                crate::cache::lock::acquire(store.clone(), format!("{stem}.lock"), ctx, clock)
                    .await?,
            );
            let reader = store.clone();
            let read_name = name.clone();
            let rejected = state.rejected.clone();
            let now = clock.utc_now();
            let read_lock = lock.clone();
            let entry = blocking(ctx, self.clock.clone(), move || {
                let _lock = read_lock;
                crate::cache::entry::read(
                    &reader,
                    &read_name,
                    crate::cache::entry::Kind::Iam,
                    now,
                    rejected.as_ref(),
                )
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
            let wall = clock.utc_now();
            let mono = clock.monotonic_now();
            let token = self
                .login(
                    ctx,
                    Some((store.clone(), format!("{stem}.totp"), lock.clone())),
                    &mut state,
                )
                .await?;
            let obtained = clock.utc_now();
            let obtained_mono = clock.monotonic_now();
            let expires = token
                .expires_at
                .ok_or_else(|| Box::new(Error::new(ErrorClass::Auth)))?;
            let entry = crate::cache::entry::Entry {
                token: token.access_token.clone(),
                expires,
                obtained,
            };
            let writer_context = ctx.clone();
            let writer_clock = self.clock.clone();
            let live = LiveWrite(Arc::new(std::sync::atomic::AtomicBool::new(false)));
            let writer_live = live.0.clone();
            let (_lock, result) = blocking(ctx, self.clock.clone(), move || {
                let result = crate::cache::entry::write_hook(
                    &store,
                    &name,
                    &entry,
                    || {
                        writer_context
                            .check(writer_clock.as_ref())
                            .map_or(Ok(()), |end| Err(Box::new(end.into())))
                    },
                    None,
                    Some(&writer_live),
                );
                Ok((lock, result))
            })
            .await?;
            result?;
            if let Some(end) = ctx.check(clock) {
                return Err(Box::new(end.into()));
            }
            let expires_monotonic = Duration::try_from(expires - wall)
                .ok()
                .and_then(|duration| mono.checked_add(duration));
            let cached = Cached {
                token,
                obtained: ObtainedAt::InProcess(obtained_mono),
                expires_monotonic,
            };
            if !cached.fresh(clock)
                || state
                    .rejected
                    .as_ref()
                    .is_some_and(|sent| cached.reject(sent, clock))
            {
                return Err(Box::new(Error::new(ErrorClass::Auth)));
            }
            let result = cached.bearer();
            state.cached = Some(cached);
            state.rejected = None;
            return Ok(result);
        }
        let wall = clock.utc_now();
        let mono = clock.monotonic_now();
        let token = self.login(ctx, None, &mut state).await?;
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        let expires_monotonic = token
            .expires_at
            .and_then(|expiry| Duration::try_from(expiry - wall).ok())
            .and_then(|duration| mono.checked_add(duration));
        let cached = Cached {
            token,
            obtained: ObtainedAt::InProcess(clock.monotonic_now()),
            expires_monotonic,
        };
        let result = cached.bearer();
        state.cached = Some(cached);
        Ok(result)
    }

    async fn login(
        &self,
        ctx: &RequestContext,
        #[cfg(unix)] marker: Option<(Arc<crate::cache::store::Store>, String, Arc<std::fs::File>)>,
        #[cfg(not(unix))] _marker: Option<()>,
        state: &mut State,
    ) -> Result<Token, Box<Error>> {
        let clock = self.clock.as_ref();
        if let Some((at, reason, status, captcha)) = state.failure {
            let age = clock.utc_now() - at;
            if age >= time::Duration::ZERO && age < time::Duration::seconds(30) {
                return Err(Box::new(
                    Error::new(ErrorClass::Auth)
                        .with_login(crate::LoginError::new(reason, status, captcha)),
                ));
            }
        }
        state.failure = None;
        let deadline = clock
            .monotonic_now()
            .checked_add(if self.timeout.is_zero() {
                Duration::from_secs(120)
            } else {
                self.timeout
            })
            .ok_or_else(|| Box::new(Error::new(ErrorClass::Auth)))?;
        let bounded_ctx = ctx.child_with_deadline(deadline);
        let deadline = bounded_ctx
            .deadline()
            .ok_or_else(|| Box::new(Error::new(ErrorClass::Auth)))?;
        let provider =
            || -> Pin<Box<dyn Future<Output = Result<Secret, totp::ProviderError>> + Send + '_>> {
                Box::pin(async {
                    let mut last = *self
                        .last_step
                        .lock()
                        .map_err(|_| totp::ProviderError::Failed)?;
                    #[cfg(unix)]
                    if let Some((store, name, lock)) = &marker {
                        let store = store.clone();
                        let name = name.clone();
                        let lock = lock.clone();
                        let current = clock.utc_now().unix_timestamp() / 30;
                        let disk = blocking(&bounded_ctx, self.clock.clone(), move || {
                            let _lock = lock;
                            Ok(crate::cache::marker::read(&store, &name, current))
                        })
                        .await
                        .map_err(|_| totp::ProviderError::TokenCache)?;
                        if let Some(disk) = disk {
                            last = Some(last.map_or(disk, |value| value.max(disk)));
                        }
                    }
                    let step = loop {
                        let now = clock.utc_now();
                        let step = now.unix_timestamp() / 30;
                        if last.is_none_or(|last| last < step) {
                            break step;
                        }
                        let next = (step + 1) * 30;
                        let delay = Duration::try_from(
                            time::Duration::seconds(next)
                                - (now - time::OffsetDateTime::UNIX_EPOCH),
                        )
                        .map_err(|_| totp::ProviderError::Failed)?;
                        let until = clock
                            .monotonic_now()
                            .checked_add(delay)
                            .ok_or(totp::ProviderError::Failed)?;
                        bounded(&bounded_ctx, clock, clock.sleep_until(until))
                            .await
                            .map_err(|_| totp::ProviderError::Failed)?;
                    };
                    let (code, step) = if let Some(provider) = &self.provider {
                        let code = bounded(&bounded_ctx, clock, provider())
                            .await
                            .map_err(|_| totp::ProviderError::Failed)?
                            .map_err(|_| totp::ProviderError::Failed)?;
                        (code, step.max(clock.utc_now().unix_timestamp() / 30))
                    } else {
                        let seed = self.seed.as_ref().ok_or(totp::ProviderError::Failed)?;
                        (
                            totp::code(seed, step * 30).map_err(|_| totp::ProviderError::Failed)?,
                            step,
                        )
                    };
                    #[cfg(unix)]
                    if let Some((store, name, lock)) = &marker {
                        let store = store.clone();
                        let name = name.clone();
                        let lock = lock.clone();
                        let context = bounded_ctx.clone();
                        let worker_clock = self.clock.clone();
                        let live = LiveWrite(Arc::new(std::sync::atomic::AtomicBool::new(false)));
                        let flag = live.0.clone();
                        blocking(&bounded_ctx, self.clock.clone(), move || {
                            let _lock = lock;
                            crate::cache::marker::write_hook(
                                &store,
                                &name,
                                step,
                                &context,
                                worker_clock.as_ref(),
                                &flag,
                            )
                        })
                        .await
                        .map_err(|_| totp::ProviderError::TokenCache)?;
                    }
                    *self
                        .last_step
                        .lock()
                        .map_err(|_| totp::ProviderError::Failed)? = Some(step);
                    Ok(code)
                })
            };
        let login = flow::Login {
            origin: &self.signin,
            dashboard: &self.dashboard,
            root_email: &self.root_email,
            username: &self.username,
            password: &self.password,
            totp: (self.seed.is_some() || self.provider.is_some())
                .then_some(totp::Source::Provider(&provider)),
            clock,
            deadline,
        };
        #[cfg(test)]
        let work = async {
            match &self.roots {
                Some(roots) => flow::run_test(login, &self.token_url, roots.clone()).await,
                None => flow::run(login, &self.token_url).await,
            }
        };
        #[cfg(not(test))]
        let work = flow::run(login, &self.token_url);
        let result = bounded(&bounded_ctx, clock, work).await?;
        result.map_err(|error| {
            if matches!(error.reason(), "signin rejection" | "totp rejection")
                || error.captcha_suspected()
            {
                state.failure = Some((
                    clock.utc_now(),
                    error.reason(),
                    error.status(),
                    error.captcha_suspected(),
                ));
            }
            if error.reason() == "token cache" {
                Box::new(Error::new(ErrorClass::TokenCache))
            } else {
                Box::new(Error::new(ErrorClass::Auth).with_login(error))
            }
        })
    }
}

#[cfg(unix)]
struct LiveWrite(Arc<std::sync::atomic::AtomicBool>);
#[cfg(unix)]
impl Drop for LiveWrite {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
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

impl BearerSource for IamUser {
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
            let mut state = bounded(ctx, self.clock.as_ref(), self.state.lock())
                .await
                .map_err(|error| *error)?;
            if evict
                && state
                    .cached
                    .as_ref()
                    .is_some_and(|cached| cached.reject(sent, self.clock.as_ref()))
            {
                if let Some(end) = ctx.check(self.clock.as_ref()) {
                    return Err(end.into());
                }
                state.cached = None;
                if cfg!(unix) && self.directory.is_some() {
                    state.rejected = Some(sent.clone());
                }
            }
            if let Some(end) = ctx.check(self.clock.as_ref()) {
                return Err(end.into());
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
