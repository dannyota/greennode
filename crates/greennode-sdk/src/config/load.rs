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
            if let Some(value) = source.var_os(std::ffi::OsStr::new(name)) {
                snapshot.values.insert(name.into(), value);
            }
        }
    }
    if let Some(end) = ctx.check(&crate::SystemClock) {
        return Err(end.into());
    }
    let worker_ctx = ctx.clone();
    let ended = Arc::new(AtomicBool::new(false));
    let _lifetime = WorkerLifetime(ended.clone());
    let worker =
        tokio::task::spawn_blocking(move || load_local(options, snapshot, &worker_ctx, &ended));
    super::wait_profile_worker(ctx, worker).await
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
fn load_local(
    options: ConfigOptions,
    source: SourceSnapshot,
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
    let pair = file::open_pair_checked(&selection, check).map_err(|error| *error)?;
    let settings_options = settings::Options {
        profile: options.profile,
        ..Default::default()
    };
    let resolved = settings::resolve_checked(
        &selection,
        pair.config,
        &settings_options,
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
    let region = resolved
        .region
        .as_ref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| crate::Error::new(crate::ErrorClass::InvalidConfig))?;
    crate::endpoint::defaults(region, Default::default())
        .map_err(|_| crate::Error::new(crate::ErrorClass::InvalidConfig))?;
    let credentials_options = credentials::Options {
        provider: options.provider,
        ..Default::default()
    };
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
    Ok(Config {
        region: region.clone(),
        profile: resolved.profile,
        project_id: resolved.project_id,
        settings: resolved.profile_settings,
        credentials: Arc::new(credentials),
    })
}
