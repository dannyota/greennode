use super::{
    Config, ConfigOptions, SourceSnapshot, WorkerLifetime, credentials, file, mode, settings,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const CREDENTIAL_VARIABLES: [&str; 16] = [
    "GREENNODE_ACCESS_TOKEN",
    "GREENNODE_CLIENT_ID",
    "GREENNODE_CLIENT_SECRET",
    "GREENNODE_ROOT_EMAIL",
    "GREENNODE_USERNAME",
    "GREENNODE_PASSWORD",
    "GREENNODE_TOTP_SECRET",
    "VNGCLOUD_ACCESS_TOKEN",
    "VNGCLOUD_CLIENT_ID",
    "VNGCLOUD_CLIENT_SECRET",
    "VNGCLOUD_ROOT_EMAIL",
    "VNGCLOUD_USERNAME",
    "VNGCLOUD_PASSWORD",
    "VNGCLOUD_TOTP_SECRET",
    "GREENNODE_VCDN_API_KEY",
    "VNGCLOUD_VCDN_API_KEY",
];

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
pub(super) async fn load(
    ctx: &crate::RequestContext,
    options: ConfigOptions,
    source: &dyn crate::ConfigSource,
) -> Result<Config, crate::Error> {
    load_with(ctx, options, source, || {}).await
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
async fn load_with(
    ctx: &crate::RequestContext,
    options: ConfigOptions,
    source: &dyn crate::ConfigSource,
    before_worker: impl FnOnce(),
) -> Result<Config, crate::Error> {
    if let Some(end) = ctx.check(&crate::SystemClock) {
        return Err(end.into());
    }
    let mut snapshot = SourceSnapshot::capture(&options, source);
    let explicit = options
        .profile
        .as_ref()
        .is_some_and(|value| !value.is_empty());
    if !explicit {
        for name in CREDENTIAL_VARIABLES {
            if options.provider.is_some() && !name.ends_with("_VCDN_API_KEY") {
                continue;
            }
            if let Some(value) = source.var_os(std::ffi::OsStr::new(name)) {
                snapshot.secrets.insert(
                    name.into(),
                    zeroize::Zeroizing::new(value.into_encoded_bytes()),
                );
            }
        }
    }
    if let Some(end) = ctx.check(&crate::SystemClock) {
        return Err(end.into());
    }
    let cwd = std::path::absolute(".").ok();
    let worker_ctx = ctx.clone();
    let ended = Arc::new(AtomicBool::new(false));
    let _lifetime = WorkerLifetime(ended.clone());
    before_worker();
    let worker = tokio::task::spawn_blocking(move || {
        load_local(options, snapshot, cwd, &worker_ctx, &ended)
    });
    super::wait_profile_worker(ctx, worker).await
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
fn load_local(
    options: ConfigOptions,
    source: SourceSnapshot,
    cwd: Option<std::path::PathBuf>,
    ctx: &crate::RequestContext,
    ended: &AtomicBool,
) -> Result<Config, crate::Error> {
    let check = || {
        ctx.check(&crate::SystemClock).or_else(|| {
            ended
                .load(Ordering::Acquire)
                .then_some(crate::ContextEnd::Cancelled)
        })
    };
    if let Some(end) = check() {
        return Err(end.into());
    }
    let settings_options = settings::Options {
        profile: options.profile.clone(),
        region: options.region.clone(),
        project_id: options.project_id.clone(),
        ..Default::default()
    };
    settings::select_profile(&settings_options, &source)
        .map_err(|_| crate::Error::new(crate::ErrorClass::InvalidConfig))?;
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
    let pair = file::open_pair_checked(&selection, check).map_err(|error| *error)?;
    let resolved = settings::resolve_guarded(
        &selection,
        pair.config,
        &settings_options,
        &source,
        settings::Policy::Read,
        &check,
        #[cfg(test)]
        options.hooks.as_deref(),
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
    let region = resolved
        .region
        .as_ref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| crate::Error::new(crate::ErrorClass::InvalidConfig))?;
    crate::endpoint::defaults(region, Default::default())
        .map_err(|_| crate::Error::new(crate::ErrorClass::InvalidConfig))?;
    let credentials_options = options.credential_options();
    let credentials = credentials::select_checked(
        &selection,
        &resolved,
        &settings_options,
        &credentials_options,
        &source,
        pair.credentials,
        &check,
    )
    .map_err(|error| *error)?;
    if let Some(end) = check() {
        return Err(end.into());
    }
    let session = super::session::Session::new(
        credentials,
        region,
        resolved.profile,
        resolved.project_id,
        options.token_cache.map(|path| {
            if path.is_relative() {
                cwd.as_ref().map_or(path.clone(), |cwd| cwd.join(&path))
            } else {
                path
            }
        }),
        source.home.filter(|path| path.is_absolute()),
        (options.iam_details, options.endpoint_overrides),
    )?;
    #[cfg(any(test, feature = "test-support"))]
    let session = session.with_fixture_clock(options.test_clock);
    if let Some(end) = check() {
        return Err(end.into());
    }
    #[cfg(any(test, feature = "test-support"))]
    session
        .set_product_fixtures(&options.product_fixtures)
        .map_err(|error| *error)?;
    Ok(Config {
        region: region.clone(),
        settings: resolved.profile_settings,
        session: Arc::new(session),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    #[tokio::test]
    async fn dropping_load_at_read_stops_parsing() {
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        };
        struct Empty;
        impl crate::ConfigSource for Empty {
            fn var_os(&self, _: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
                None
            }
        }
        struct Done(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Done {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let path = env::temp_dir().join(format!(
            "greennode-drop-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(
            path.join("config"),
            format!("[default]\nregion=hcm-3\n{}", "key=value\n".repeat(200)),
        )
        .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let started = Mutex::new(Some(started_tx));
        let release = Mutex::new(release_rx);
        let parsed = Arc::new(AtomicBool::new(false));
        let recorded = parsed.clone();
        let done = Done(Some(done_tx));
        let hooks = super::super::ParseHooks {
            on_read: Box::new(move |count| {
                if count == 0
                    && let Some(sender) = started.lock().unwrap().take()
                {
                    let _ = sender.send(());
                    let _ = release.lock().unwrap().recv();
                }
            }),
            on_line: Box::new(move || {
                let _ = &done;
                recorded.store(true, Ordering::SeqCst);
            }),
        };
        let mut options = ConfigOptions::default().with_config_file(path.join("config"));
        options.hooks = Some(Arc::new(hooks));
        let (ctx, _) = crate::RequestContext::new();
        let source = Empty;
        let mut future = Box::pin(Config::load(&ctx, options, &source));
        tokio::select! { started = started_rx => assert!(started.is_ok()), _ = &mut future => panic!("load completed before read hook") }
        drop(future);
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), done_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(!parsed.load(Ordering::SeqCst));
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn relative_paths_use_cwd_at_call() {
        let original = env::current_dir().unwrap();
        let status = std::process::Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "--ignored",
                "config::load::tests::cwd_child_entry",
            ])
            .env_clear()
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(env::current_dir().unwrap(), original);
    }

    #[test]
    #[ignore]
    fn cwd_child_entry() {
        assert!(
            crate::ConfigSource::var_os(
                &crate::ProcessConfigSource,
                std::ffi::OsStr::new("GREENNODE_TEST_CWD_CHILD")
            )
            .is_none()
        );
        struct Empty;
        impl crate::ConfigSource for Empty {
            fn var_os(&self, _: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
                None
            }
        }
        let root = env::temp_dir().join(format!("greennode-cwd-{}", std::process::id()));
        let first = root.join("first");
        let second = root.join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("config"), "[default]\nregion=hcm-3\n").unwrap();
        std::fs::write(
            first.join("credentials"),
            "[default]\nclient_id=toy\nclient_secret=toy\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                first.join("credentials"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        env::set_current_dir(&first).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (ctx, _) = crate::RequestContext::new();
        let options = ConfigOptions::default()
            .with_config_file("config".into())
            .with_shared_credentials_file("credentials".into());
        let result = runtime.block_on(load_with(&ctx, options, &Empty, || {
            env::set_current_dir(&second).unwrap()
        }));
        assert!(result.is_ok());
        env::set_current_dir(env::temp_dir()).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
