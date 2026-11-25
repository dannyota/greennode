use greennode_sdk::{
    Config, ConfigOptions, ConfigSource, ErrorClass, FileDiagnostic, FileSlot, RequestContext,
};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "greennode-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[derive(Default)]
struct Source {
    values: HashMap<OsString, OsString>,
    reads: Mutex<Vec<OsString>>,
}
impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        self.reads.lock().unwrap().push(name.to_owned());
        self.values.get(name).cloned()
    }
}
impl Source {
    fn with(mut self, name: &str, value: impl Into<OsString>) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }
    fn assert_no_credential_reads(&self) {
        for name in self.reads.lock().unwrap().iter() {
            assert!(matches!(
                name.to_str(),
                Some(
                    "HOME"
                        | "USERPROFILE"
                        | "GREENNODE_CONFIG_FILE"
                        | "GREENNODE_SHARED_CREDENTIALS_FILE"
                        | "VNGCLOUD_CONFIG_FILE"
                        | "VNGCLOUD_SHARED_CREDENTIALS_FILE"
                        | "GREENNODE_PROFILE"
                        | "VNGCLOUD_PROFILE"
                        | "GREENNODE_REGION"
                        | "VNGCLOUD_REGION"
                        | "GREENNODE_PROJECT_ID"
                        | "VNGCLOUD_PROJECT_ID"
                        | "GREENNODE_READ_ONLY"
                        | "VNGCLOUD_READ_ONLY"
                )
            ));
        }
    }
}

#[tokio::test]
async fn resolves_each_file_mode_and_source_labels() {
    for (folder, label) in [(".greennode", "~/.greennode"), (".vngcloud", "~/.vngcloud")] {
        let dir = Dir::new();
        let base = dir.0.join(folder);
        std::fs::create_dir(&base).unwrap();
        std::fs::write(
            base.join("config"),
            "[profile chosen]\nREGION=RegionValue\n",
        )
        .unwrap();
        std::fs::write(base.join("credentials"), "toy-credential").unwrap();
        let source = Source::default()
            .with("HOME", dir.0.as_os_str())
            .with("USERPROFILE", dir.0.as_os_str());
        let options = ConfigOptions::default().with_profile("chosen".into());
        let (ctx, _) = RequestContext::new();
        let result = Config::resolve_profile(&ctx, &options, &source)
            .await
            .unwrap();
        assert_eq!(result.name(), "chosen");
        assert_eq!(result.profile_setting("region"), Some("RegionValue"));
        assert_eq!(result.config_source(), Some(label));
        assert_eq!(result.credentials_source(), Some(label));
        assert!(!format!("{result:?}").contains("RegionValue"));
        source.assert_no_credential_reads();
    }
}

#[tokio::test]
async fn respects_metadata_only_and_redacts_settings_and_errors() {
    let dir = Dir::new();
    let config = dir.0.join("config-toy-path");
    let credentials = dir.0.join("credentials-toy-path");
    std::fs::write(
        &config,
        "[default]\nclient_id=\nclient_secret=\nother=toy-value\n",
    )
    .unwrap();
    std::fs::write(&credentials, "toy-credential").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(std::fs::File::open(&credentials).is_err());
    }
    let options = ConfigOptions::default()
        .with_config_file(config.clone())
        .with_shared_credentials_file(credentials.clone());
    let (ctx, _) = RequestContext::new();
    let source = Source::default();
    assert!(!format!("{options:?}").contains("toy"));
    assert!(
        Config::resolve_profile(&ctx, &options, &source)
            .await
            .is_ok()
    );
    let result = Config::resolve_profile(&ctx, &options, &source)
        .await
        .unwrap();
    assert_eq!(result.profile_setting("client_id"), None);
    assert_eq!(result.profile_setting("client_secret"), None);
    assert_eq!(result.profile_setting("other"), Some("toy-value"));
    assert_eq!(result.config_source(), Some("option"));
    assert_eq!(result.credentials_source(), Some("option"));
    assert!(!format!("{result:?}").contains("toy"));
    std::fs::remove_file(&config).unwrap();
    let error = Config::resolve_profile(&ctx, &options, &source)
        .await
        .unwrap_err();
    assert_eq!(error.class(), ErrorClass::InvalidConfig);
    assert!(error.detail::<FileDiagnostic>().is_some());
    let detail = error.detail::<FileDiagnostic>().unwrap();
    assert_eq!(detail.slot(), FileSlot::Config);
    assert_eq!(detail.source(), "option");
    assert_eq!(detail.reason(), "not found");
    assert!(!format!("{error} {error:?} {detail:?}").contains("toy"));
    source.assert_no_credential_reads();
}

#[tokio::test]
async fn refuses_config_credentials_alias() {
    let dir = Dir::new();
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, "[default]\nregion=toy\n").unwrap();
    std::fs::hard_link(&config, &credentials).unwrap();
    let options = ConfigOptions::default()
        .with_config_file(config)
        .with_shared_credentials_file(credentials);
    let (ctx, _) = RequestContext::new();
    let source = Source::default();
    assert!(
        matches!(Config::resolve_profile(&ctx, &options, &source).await, Err(error) if error.class() == ErrorClass::InvalidConfig)
    );
    source.assert_no_credential_reads();
}

#[tokio::test]
async fn reports_profile_section_presence() {
    let dir = Dir::new();
    let config = dir.0.join("config");
    std::fs::write(&config, "[profile selected]\n").unwrap();
    let source = Source::default();
    let (ctx, _) = RequestContext::new();
    let options = ConfigOptions::default()
        .with_profile("selected".into())
        .with_config_file(config.clone());
    assert!(
        Config::resolve_profile(&ctx, &options, &source)
            .await
            .unwrap()
            .section_present()
    );
    let options = ConfigOptions::default()
        .with_profile("absent".into())
        .with_config_file(config);
    assert!(
        !Config::resolve_profile(&ctx, &options, &source)
            .await
            .unwrap()
            .section_present()
    );
    let options = ConfigOptions::default().with_profile("absent".into());
    assert!(
        !Config::resolve_profile(&ctx, &options, &source)
            .await
            .unwrap()
            .section_present()
    );
}

#[tokio::test]
async fn expired_or_cancelled_context_refuses_before_source_access() {
    let source = Source::default();
    let options = ConfigOptions::default();
    let (ctx, handle) = RequestContext::new();
    handle.cancel();
    assert!(
        matches!(Config::resolve_profile(&ctx, &options, &source).await, Err(error) if error.class() == ErrorClass::Cancelled)
    );
    let (ctx, _) = RequestContext::new();
    let expired =
        ctx.child_with_deadline(tokio::time::Instant::now() - std::time::Duration::from_secs(1));
    assert!(
        matches!(Config::resolve_profile(&expired, &options, &source).await, Err(error) if error.class() == ErrorClass::DeadlineExceeded)
    );
    assert!(source.reads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_profile_consults_only_eligible_settings_once() {
    let source = Source::default();
    let options = ConfigOptions::default().with_profile("chosen".into());
    let (ctx, _) = RequestContext::new();
    assert!(
        Config::resolve_profile(&ctx, &options, &source)
            .await
            .is_ok()
    );
    let mut actual: Vec<_> = source
        .reads
        .lock()
        .unwrap()
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    actual.sort();
    let mut expected = vec![
        "GREENNODE_CONFIG_FILE",
        "GREENNODE_SHARED_CREDENTIALS_FILE",
        "VNGCLOUD_CONFIG_FILE",
        "VNGCLOUD_SHARED_CREDENTIALS_FILE",
        "GREENNODE_READ_ONLY",
        "VNGCLOUD_READ_ONLY",
        "GREENNODE_REGION",
        "VNGCLOUD_REGION",
        if cfg!(windows) { "USERPROFILE" } else { "HOME" },
    ];
    expected.sort();
    assert_eq!(actual, expected);
}

#[cfg(unix)]
#[tokio::test]
async fn ambiguous_explicit_config_has_public_diagnostic() {
    let dir = Dir::new();
    let parent = dir.0.join("not-a-directory");
    std::fs::write(&parent, "toy").unwrap();
    let options = ConfigOptions::default().with_config_file(parent.join("config"));
    let (ctx, _) = RequestContext::new();
    let error = Config::resolve_profile(&ctx, &options, &Source::default())
        .await
        .unwrap_err();
    assert_eq!(error.class(), ErrorClass::InvalidConfig);
    assert!(error.detail::<FileDiagnostic>().is_some());
    let detail = error.detail::<FileDiagnostic>().unwrap();
    assert_eq!(detail.slot(), FileSlot::Config);
    assert_eq!(detail.source(), "option");
    assert_eq!(detail.reason(), "lookup failed");
    assert_eq!(detail.line(), None);
}

#[tokio::test]
async fn file_decoding_errors_have_no_line_but_parse_errors_do() {
    let dir = Dir::new();
    let path = dir.0.join("config");
    let options = ConfigOptions::default().with_config_file(path.clone());
    let source = Source::default();
    let (ctx, _) = RequestContext::new();
    std::fs::write(&path, [0xff]).unwrap();
    let error = Config::resolve_profile(&ctx, &options, &source)
        .await
        .unwrap_err();
    let detail = error.detail::<FileDiagnostic>().unwrap();
    assert_eq!(detail.reason(), "invalid UTF-8");
    assert_eq!(detail.line(), None);
    std::fs::write(&path, "[default]\ninvalid line\n").unwrap();
    let error = Config::resolve_profile(&ctx, &options, &source)
        .await
        .unwrap_err();
    let detail = error.detail::<FileDiagnostic>().unwrap();
    assert_eq!(detail.reason(), "line has no '='");
    assert_eq!(detail.line(), Some(2));
}
