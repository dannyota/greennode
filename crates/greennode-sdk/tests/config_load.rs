use greennode_sdk::{
    Config, ConfigOptions, ConfigSource, CredentialsProvider, Error, ErrorClass, FileDiagnostic,
    FileSlot, RequestContext, Secret, Token,
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
            "greennode-load-{}-{}",
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
impl Source {
    fn with(mut self, name: &str, value: impl Into<OsString>) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }
}
impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        self.reads.lock().unwrap().push(name.to_owned());
        self.values.get(name).cloned()
    }
}

struct Provider;
impl CredentialsProvider for Provider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Token, Error>> + Send + 'a>>
    {
        Box::pin(async { Ok(Token::new(Secret::new("toy-provider"), None)) })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

fn files(dir: &Dir, contents: &str) -> ConfigOptions {
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, "[default]\nregion=hcm-3\n").unwrap();
    std::fs::write(&credentials, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    ConfigOptions::default()
        .with_config_file(config)
        .with_shared_credentials_file(credentials)
}

#[tokio::test]
async fn environment_credentials_override_incomplete_file_and_are_read_once() {
    let dir = Dir::new();
    let options = files(&dir, "[default]\nclient_id=toy-id\n");
    let source = Source::default().with("GREENNODE_ACCESS_TOKEN", "toy-env-token");
    let (ctx, _) = RequestContext::new();
    let result = Config::load(&ctx, options, &source).await;
    assert!(result.is_ok());
    let config = result.unwrap();
    assert_eq!(config.region(), "hcm-3");
    assert!(!format!("{config:?}").contains("toy-env-token"));
    let reads = source.reads.lock().unwrap();
    assert_eq!(
        reads
            .iter()
            .filter(|name| *name == OsStr::new("GREENNODE_ACCESS_TOKEN"))
            .count(),
        1
    );
}

#[tokio::test]
async fn caller_provider_overrides_invalid_environment_and_file() {
    let dir = Dir::new();
    let options = files(&dir, "[default]\nclient_id=toy-id\n")
        .with_credentials_provider(std::sync::Arc::new(Provider));
    let source = Source::default()
        .with("GREENNODE_CLIENT_ID", "bad:id")
        .with("GREENNODE_CLIENT_SECRET", "toy-secret");
    let (ctx, _) = RequestContext::new();
    let result = Config::load(&ctx, options, &source).await;
    assert!(result.is_ok());
    let config = result.unwrap();
    assert_eq!(config.region(), "hcm-3");
    assert!(!format!("{config:?}").contains("toy-secret"));
}

#[tokio::test]
async fn alias_is_refused_and_distinct_files_load() {
    let dir = Dir::new();
    let options = files(&dir, "[default]\nclient_id=toy\nclient_secret=toy-secret\n");
    let (ctx, _) = RequestContext::new();
    assert!(
        Config::load(&ctx, options, &Source::default())
            .await
            .is_ok()
    );
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::remove_file(&credentials).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::hard_link(&config, &credentials).unwrap();
    let options = ConfigOptions::default()
        .with_config_file(config)
        .with_shared_credentials_file(credentials)
        .with_credentials_provider(std::sync::Arc::new(Provider));
    let result = Config::load(&ctx, options, &Source::default()).await;
    assert!(matches!(result, Err(error) if error.class() == ErrorClass::InvalidConfig));
}

#[tokio::test]
async fn malformed_credentials_have_safe_diagnostic() {
    let dir = Dir::new();
    let options = files(
        &dir,
        "[default]\nclient_id=toy-id\nclient_secret=toy-secret\n[unfinished-toy-secret\n",
    );
    let (ctx, _) = RequestContext::new();
    let error = Config::load(&ctx, options, &Source::default())
        .await
        .unwrap_err();
    assert_eq!(error.class(), ErrorClass::CredentialsFile);
    assert_eq!(
        error.detail::<FileDiagnostic>().unwrap().slot(),
        FileSlot::Credentials
    );
    assert!(!format!("{error:?} {error}").contains("toy-secret"));
}

#[tokio::test]
async fn ended_context_never_reads_source() {
    for expired in [true, false] {
        let (ctx, cancel) = RequestContext::new();
        let ctx = if expired {
            ctx.child_with_deadline(tokio::time::Instant::now())
        } else {
            cancel.cancel();
            ctx
        };
        let source = Source::default();
        let result = Config::load(&ctx, ConfigOptions::default(), &source).await;
        let expected = if expired {
            ErrorClass::DeadlineExceeded
        } else {
            ErrorClass::Cancelled
        };
        assert!(matches!(result, Err(error) if error.class() == expected));
        assert!(source.reads.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn loads_both_modes_with_read_only_profile_setting() {
    for folder in [".greennode", ".vngcloud"] {
        let dir = Dir::new();
        let base = dir.0.join(folder);
        std::fs::create_dir(&base).unwrap();
        std::fs::write(
            base.join("config"),
            "[default]\nregion=hcm-3\nread_only=1\nother=toy-secret\n",
        )
        .unwrap();
        let credentials = base.join("credentials");
        std::fs::write(
            &credentials,
            "[default]\nclient_id=toy-id\nclient_secret=toy-secret\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let source = Source::default()
            .with("HOME", dir.0.as_os_str())
            .with("USERPROFILE", dir.0.as_os_str());
        let (ctx, _) = RequestContext::new();
        let result = Config::load(&ctx, ConfigOptions::default(), &source).await;
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.region(), "hcm-3");
        assert_eq!(config.profile_setting("read_only"), Some("1"));
        assert!(!format!("{config:?}").contains("toy-secret"));
    }
}
