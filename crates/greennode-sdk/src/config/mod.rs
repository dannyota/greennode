#![allow(dead_code)]

mod credentials;
mod file;
mod ini;
mod load;
mod mode;
mod settings;

#[derive(Clone, Copy)]
enum Slot {
    Config,
    Credentials,
}

impl Slot {
    fn label(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Credentials => "credentials",
        }
    }
}

/// Options for resolving local configuration.
#[derive(Clone, Default)]
pub struct ConfigOptions {
    profile: Option<String>,
    config_file: Option<std::path::PathBuf>,
    credentials_file: Option<std::path::PathBuf>,
    provider: Option<std::sync::Arc<dyn crate::CredentialsProvider>>,
}

impl ConfigOptions {
    /// Supplies the management credentials provider.
    pub fn with_credentials_provider(
        mut self,
        provider: std::sync::Arc<dyn crate::CredentialsProvider>,
    ) -> Self {
        self.provider = Some(provider);
        self
    }
    /// Selects a named profile.
    pub fn with_profile(mut self, profile: String) -> Self {
        self.profile = Some(profile);
        self
    }
    /// Selects a configuration file.
    pub fn with_config_file(mut self, path: std::path::PathBuf) -> Self {
        self.config_file = Some(path);
        self
    }
    /// Selects a shared credentials file.
    pub fn with_shared_credentials_file(mut self, path: std::path::PathBuf) -> Self {
        self.credentials_file = Some(path);
        self
    }
}

impl std::fmt::Debug for ConfigOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigOptions([REDACTED])")
    }
}

/// A management configuration.
#[derive(Clone)]
pub struct Config {
    region: String,
    profile: String,
    project_id: Option<String>,
    settings: std::collections::BTreeMap<String, String>,
    credentials: std::sync::Arc<credentials::Selected>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Config([REDACTED])")
    }
}

/// The settings and file provenance selected by an offline lookup.
pub struct ResolvedProfile {
    name: String,
    settings: std::collections::BTreeMap<String, String>,
    section_present: bool,
    config_source: Option<&'static str>,
    credentials_source: Option<&'static str>,
}

impl std::fmt::Debug for ResolvedProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolvedProfile([REDACTED])")
    }
}

impl ResolvedProfile {
    /// Returns the selected profile name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Reports whether the selected section exists in the config file.
    pub fn section_present(&self) -> bool {
        self.section_present
    }
    /// Returns a stored non-credential setting from the selected section.
    pub fn profile_setting(&self, key: &str) -> Option<&str> {
        self.settings.get(key).map(String::as_str)
    }
    /// Returns the selected configuration file's fixed source label.
    pub fn config_source(&self) -> Option<&'static str> {
        self.config_source
    }
    /// Returns the selected credentials file's fixed source label.
    pub fn credentials_source(&self) -> Option<&'static str> {
        self.credentials_source
    }
}

impl Config {
    /// Loads configuration and credentials without network access.
    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Error by value"
    )]
    pub async fn load(
        ctx: &crate::RequestContext,
        options: ConfigOptions,
        source: &dyn crate::ConfigSource,
    ) -> Result<Self, crate::Error> {
        load::load(ctx, options, source).await
    }
    /// Returns the selected region.
    pub fn region(&self) -> &str {
        &self.region
    }
    /// Returns a non-credential profile setting.
    pub fn profile_setting(&self, key: &str) -> Option<&str> {
        self.settings.get(key).map(String::as_str)
    }
    /// Resolves local profile settings without reading credentials.
    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Error by value"
    )]
    pub async fn resolve_profile(
        ctx: &crate::RequestContext,
        options: &ConfigOptions,
        source: &dyn crate::ConfigSource,
    ) -> Result<ResolvedProfile, crate::Error> {
        resolve_profile_with(ctx, options, source, || {}, || {}).await
    }
}

struct WorkerLifetime(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
async fn resolve_profile_with(
    ctx: &crate::RequestContext,
    options: &ConfigOptions,
    source: &dyn crate::ConfigSource,
    before_io: impl FnOnce() + Send + 'static,
    on_io: impl FnOnce() + Send + 'static,
) -> Result<ResolvedProfile, crate::Error> {
    if let Some(end) = ctx.check(&crate::SystemClock) {
        return Err(end.into());
    }
    let snapshot = SourceSnapshot::capture(options, source);
    if let Some(end) = ctx.check(&crate::SystemClock) {
        return Err(end.into());
    }
    let options = options.clone();
    let worker_ctx = ctx.clone();
    let ended = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _lifetime = WorkerLifetime(ended.clone());
    let worker = tokio::task::spawn_blocking(move || {
        resolve_profile_local(&options, snapshot, &worker_ctx, &ended, before_io, on_io)
    });
    wait_profile_worker(ctx, worker).await
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
async fn wait_profile_worker<T>(
    ctx: &crate::RequestContext,
    worker: tokio::task::JoinHandle<Result<T, crate::Error>>,
) -> Result<T, crate::Error> {
    let clock = crate::SystemClock;
    let mut worker = std::pin::pin!(worker);
    let mut ended = std::pin::pin!(ctx.wait(&clock));
    std::future::poll_fn(|cx| {
        use std::future::Future;
        use std::task::Poll;
        if let Poll::Ready(end) = ended.as_mut().poll(cx) {
            return Poll::Ready(Err(end.into()));
        }
        match worker.as_mut().poll(cx) {
            Poll::Ready(result) => {
                if let Some(end) = ctx.check(&clock) {
                    return Poll::Ready(Err(end.into()));
                }
                Poll::Ready(
                    result.unwrap_or_else(|_| {
                        Err(crate::Error::new(crate::ErrorClass::InvalidConfig))
                    }),
                )
            }
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

const SETTINGS_VARIABLES: [&str; 12] = [
    "GREENNODE_CONFIG_FILE",
    "GREENNODE_SHARED_CREDENTIALS_FILE",
    "VNGCLOUD_CONFIG_FILE",
    "VNGCLOUD_SHARED_CREDENTIALS_FILE",
    "GREENNODE_READ_ONLY",
    "VNGCLOUD_READ_ONLY",
    "GREENNODE_PROFILE",
    "VNGCLOUD_PROFILE",
    "GREENNODE_REGION",
    "VNGCLOUD_REGION",
    "GREENNODE_PROJECT_ID",
    "VNGCLOUD_PROJECT_ID",
];

struct SourceSnapshot {
    values: std::collections::HashMap<std::ffi::OsString, std::ffi::OsString>,
    secrets: std::collections::HashMap<std::ffi::OsString, zeroize::Zeroizing<Vec<u8>>>,
    home: Option<std::path::PathBuf>,
}

#[cfg(unix)]
fn invalid_os_string() -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(vec![0xff])
}

#[cfg(windows)]
fn invalid_os_string() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    std::ffi::OsString::from_wide(&[0xd800])
}

#[cfg(not(any(unix, windows)))]
fn invalid_os_string() -> std::ffi::OsString {
    std::ffi::OsString::from("\0")
}

impl crate::ConfigSource for SourceSnapshot {
    fn var_os(&self, name: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
        self.values.get(name).cloned().or_else(|| {
            self.secrets
                .get(name)
                .map(|value| match std::str::from_utf8(value) {
                    Ok(value) => std::ffi::OsString::from(value),
                    Err(_) => invalid_os_string(),
                })
        })
    }
    fn home_dir(&self) -> Option<std::path::PathBuf> {
        self.home.clone()
    }
}

impl SourceSnapshot {
    fn capture(options: &ConfigOptions, source: &dyn crate::ConfigSource) -> Self {
        let mut values = std::collections::HashMap::new();
        let explicit = options
            .profile
            .as_ref()
            .is_some_and(|value| !value.is_empty());
        for name in SETTINGS_VARIABLES {
            let first = match name {
                "GREENNODE_PROFILE" | "GREENNODE_PROJECT_ID" if explicit => continue,
                "VNGCLOUD_PROFILE" if explicit => continue,
                "VNGCLOUD_PROJECT_ID" if explicit => continue,
                "VNGCLOUD_PROFILE" => Some("GREENNODE_PROFILE"),
                "VNGCLOUD_REGION" => Some("GREENNODE_REGION"),
                "VNGCLOUD_PROJECT_ID" => Some("GREENNODE_PROJECT_ID"),
                _ => None,
            };
            if first
                .and_then(|name| values.get(std::ffi::OsStr::new(name)))
                .is_some_and(|value: &std::ffi::OsString| !value.is_empty())
            {
                continue;
            }
            if let Some(value) = source.var_os(std::ffi::OsStr::new(name)) {
                values.insert(name.into(), value);
            }
        }
        Self {
            values,
            secrets: Default::default(),
            home: source.home_dir(),
        }
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
fn resolve_profile_local(
    options: &ConfigOptions,
    source: SourceSnapshot,
    ctx: &crate::RequestContext,
    ended: &std::sync::atomic::AtomicBool,
    before_io: impl FnOnce(),
    on_io: impl FnOnce(),
) -> Result<ResolvedProfile, crate::Error> {
    let check = || {
        ctx.check(&crate::SystemClock).or_else(|| {
            ended
                .load(std::sync::atomic::Ordering::Acquire)
                .then_some(crate::ContextEnd::Cancelled)
        })
    };
    before_io();
    if let Some(end) = check() {
        return Err(end.into());
    }
    on_io();
    let cwd = std::path::absolute(".").ok();
    let selection = mode::select(
        options.config_file.as_deref(),
        options.credentials_file.as_deref(),
        &source,
        cwd.as_deref(),
        |path| mode::probe_checked(path, &|| check().is_some()),
    );
    if let Some(end) = check() {
        return Err(end.into());
    }
    let selection = selection.map_err(|error| *error)?;
    let file = file::open_settings_checked(&selection, check).map_err(|error| *error)?;
    if let Some(end) = check() {
        return Err(end.into());
    }
    let local_options = settings::Options {
        profile: options.profile.clone(),
        ..Default::default()
    };
    let resolved = settings::resolve_checked(
        &selection,
        file,
        &local_options,
        &source,
        settings::Policy::Read,
        &check,
    );
    if let Some(end) = check() {
        return Err(end.into());
    }
    let resolved = resolved.map_err(|error| match error {
        settings::SettingsError::Parse {
            source, diagnostic, ..
        } => crate::Error::new(crate::ErrorClass::InvalidConfig).with_detail(
            crate::FileDiagnostic::from_text(crate::FileSlot::Config, source, &diagnostic),
        ),
        _ => crate::Error::new(crate::ErrorClass::InvalidConfig),
    })?;
    Ok(ResolvedProfile {
        name: resolved.profile,
        settings: resolved.profile_settings,
        section_present: resolved.section_present,
        config_source: selection.config_source,
        credentials_source: selection.credentials_source,
    })
}

#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    #[expect(
        clippy::result_large_err,
        reason = "The worker returns the public Error type"
    )]
    async fn cancellation_and_deadline_do_not_wait_for_worker() {
        for deadline in [false, true] {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            let worker = tokio::task::spawn_blocking(move || {
                let _ = started_tx.send(());
                let _ = release_rx.recv();
                let _ = done_tx.send(());
                Err::<ResolvedProfile, _>(crate::Error::new(crate::ErrorClass::InvalidConfig))
            });
            started_rx.await.unwrap();
            let (ctx, cancel) = crate::RequestContext::new();
            let ctx = if deadline {
                ctx.child_with_deadline(tokio::time::Instant::now() + Duration::from_millis(30))
            } else {
                cancel.cancel();
                ctx
            };
            let result =
                tokio::time::timeout(Duration::from_secs(2), wait_profile_worker(&ctx, worker))
                    .await;
            let expected = if deadline {
                crate::ErrorClass::DeadlineExceeded
            } else {
                crate::ErrorClass::Cancelled
            };
            assert!(matches!(result, Ok(Err(error)) if error.class() == expected));
            release_tx.send(()).unwrap();
            done_rx.await.unwrap();
        }
    }
    #[tokio::test]
    async fn dropping_resolution_stops_worker_before_file_io() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct Empty;
        impl crate::ConfigSource for Empty {
            fn var_os(&self, _: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
                None
            }
        }
        struct Completion(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Completion {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let wrote = Arc::new(AtomicBool::new(false));
        let recorded = wrote.clone();
        let completion = Completion(Some(done_tx));
        let (ctx, _) = crate::RequestContext::new();
        let options = ConfigOptions::default();
        let source = Empty;
        let mut future = Box::pin(resolve_profile_with(
            &ctx,
            &options,
            &source,
            move || {
                let _ = started_tx.send(());
                let _ = release_rx.recv();
            },
            move || {
                let _completion = completion;
                recorded.store(true, Ordering::SeqCst);
            },
        ));
        tokio::select! { _ = started_rx => {}, _ = &mut future => panic!("worker finished before pause") }
        drop(future);
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), done_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(!wrote.load(Ordering::SeqCst));
        assert!(ctx.check(&crate::SystemClock).is_none());
    }
}
