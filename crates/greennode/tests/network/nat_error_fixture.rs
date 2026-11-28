use super::*;
use greennode_sdk::{
    Config, CredentialsProvider, IamUserAuth, TestClock, Token,
    testing::{clock_advancing_on_sleep, with_clock},
};
use shape::{SdkShape, actual};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

pub(super) enum Observed {
    Sdk(Option<SdkShape>),
    Cli(ExitCode, Vec<u8>),
}

struct PendingInvalidation(Mutex<Option<oneshot::Sender<()>>>);

impl CredentialsProvider for PendingInvalidation {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async { Ok(Token::new(Secret::new(TOKEN), None)) })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async {
            self.0.lock().unwrap().take().unwrap().send(()).unwrap();
            std::future::pending().await
        })
    }
}

struct CacheFile(std::path::PathBuf);

impl CacheFile {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("greennode-nat-cache-{}", getrandom::u64().unwrap()));
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        Self(path)
    }
}

impl Drop for CacheFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn iam_options(server: &Server) -> ConfigOptions {
    let auth = IamUserAuth::new(
        "owner@example.test".into(),
        "toy-user".into(),
        Secret::new("toy-password"),
    )
    .with_signin_base_url(server.origin.clone());
    options().with_iam_user(auth)
}

pub(super) async fn run_case(
    target: Target,
    stage: Stage,
    outcome: Outcome,
    cli: bool,
) -> (Observed, Option<std::time::Duration>) {
    let local = matches!(
        outcome,
        Outcome::Login
            | Outcome::TokenCache
            | Outcome::InvalidPath
            | Outcome::NegativePage
            | Outcome::UnsupportedRegion
    );
    let (projects, network, mut ready) = if local {
        (Vec::new(), Vec::new(), None)
    } else {
        let (reply, ready) = affected(target, stage, outcome);
        let repeats = if matches!(outcome, Outcome::Status(429)) {
            4
        } else {
            1
        };
        let replies_for_stage =
            std::iter::once(reply).chain((1..repeats).map(|_| affected(target, stage, outcome).0));
        if stage == Stage::Project {
            (replies_for_stage.collect(), Vec::new(), ready)
        } else {
            let mut replies: Vec<_> = target
                .stages()
                .iter()
                .copied()
                .skip(1)
                .take_while(|item| *item != stage)
                .map(|item| response("200 OK", good(target, item)))
                .collect();
            replies.extend(replies_for_stage);
            (vec![response("200 OK", PROJECT)], replies, ready)
        }
    };
    let expected_requests = (projects.len(), network.len());
    let projects = Server::start(projects);
    let network = Server::start(network);
    let cache = matches!(outcome, Outcome::TokenCache).then(CacheFile::new);
    let opts = match outcome {
        Outcome::Login => iam_options(&network),
        Outcome::TokenCache => {
            iam_options(&network).with_token_cache(cache.as_ref().unwrap().0.clone())
        }
        Outcome::WithheldNotFound => options().with_static_token(Secret::new("404")),
        Outcome::CancelledInvalidation => {
            let (sent, waiting) = oneshot::channel();
            ready = Some(waiting);
            options()
                .with_credentials_provider(Arc::new(PendingInvalidation(Mutex::new(Some(sent)))))
        }
        _ => options().with_static_token(Secret::new(TOKEN)),
    };
    let opts = with_product_loopback(
        Product::Vserver,
        opts.with_region(region(outcome).to_owned()),
        &projects,
    );
    let opts = with_product_loopback(Product::Vnetwork, opts, &network);
    let retry_clock =
        matches!(outcome, Outcome::Status(429)).then(|| TestClock::new(SystemClock.utc_now()));
    let retry_start = retry_clock.as_ref().map(Clock::monotonic_now);
    let (ctx, cancel) = RequestContext::new();
    let deadline_clock = matches!(outcome, Outcome::Deadline)
        .then(|| Arc::new(TestClock::new(SystemClock.utc_now())));
    let (opts, ctx) = if let Some(clock) = &deadline_clock {
        let end = clock.monotonic_now() + std::time::Duration::from_secs(1);
        (
            with_clock(opts, clock.clone()),
            ctx.child_with_deadline(end),
        )
    } else {
        (opts, ctx)
    };
    let retry_driver = retry_clock.as_ref().map(|clock| {
        Arc::new(clock_advancing_on_sleep(
            clock.clone(),
            std::time::Duration::from_secs(1),
        ))
    });
    let (opts, clock): (_, &dyn Clock) = if let Some(clock) = &retry_driver {
        (with_clock(opts, clock.clone()), clock.as_ref())
    } else if let Some(clock) = &deadline_clock {
        (opts, clock.as_ref())
    } else {
        (opts, &SystemClock)
    };
    let call = async {
        if cli {
            let (status, stderr) = target.cli(opts, &ctx, clock, outcome).await;
            Observed::Cli(status, stderr)
        } else {
            let config = Config::new(opts).unwrap();
            Observed::Sdk(
                target
                    .call(&Client::new(&config), &ctx, outcome)
                    .await
                    .map(actual),
            )
        }
    };
    let result = if let Some(ready) = ready {
        tokio::pin!(call);
        tokio::select! {
            result = &mut call => result,
            sent = ready => {
                sent.unwrap();
                if let Some(clock) = &deadline_clock {
                    clock.advance(std::time::Duration::from_secs(1));
                } else {
                    cancel.cancel();
                }
                call.await
            }
        }
    } else {
        call.await
    };
    assert_eq!(
        (projects.requests(), network.requests()),
        expected_requests,
        "{target:?} {stage:?} {outcome:?}"
    );
    let elapsed = retry_clock
        .as_ref()
        .zip(retry_start)
        .map(|(clock, start)| clock.monotonic_now() - start);
    (result, elapsed)
}
