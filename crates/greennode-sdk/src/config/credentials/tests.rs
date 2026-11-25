mod diagnostics;

use super::*;
use crate::config::mode::{Mode, Paths};
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[derive(Default)]
struct Map(HashMap<OsString, OsString>);
impl ConfigSource for Map {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        self.0.get(name).cloned()
    }
}
impl Map {
    fn with(mut self, name: &str, value: &str) -> Self {
        self.0.insert(name.into(), value.into());
        self
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new(contents: &str) -> Self {
        let path = env::temp_dir().join(format!(
            "greennode-credentials-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("credentials"), contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                path.join("credentials"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        Self(path)
    }
    fn selection(&self, mode: Mode) -> Selection {
        Selection {
            paths: Paths {
                mode,
                config: None,
                credentials: Some(self.0.join("credentials")),
                config_named: false,
                credentials_named: false,
            },
            config_present: false,
            credentials_present: true,
            config_source: None,
            credentials_source: Some(if mode == Mode::Greennode {
                "~/.greennode"
            } else {
                "~/.vngcloud"
            }),
        }
    }
    fn file(&self) -> File {
        crate::config::file::open(
            &self.0.join("credentials"),
            crate::config::Slot::Credentials,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn settings(profile: &str) -> Settings {
    Settings {
        profile: profile.into(),
        region: None,
        project_id: None,
        read_only_source: None,
        file_read_only: None,
        output: "json".into(),
        section_present: false,
        profile_settings: Default::default(),
    }
}

#[cfg(unix)]
#[test]
fn lower_priority_malformed_method_does_not_override_token() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new("[default]\n");
    let mut source = Map::default().with("GREENNODE_ACCESS_TOKEN", "toy-token");
    source.0.insert(
        "GREENNODE_CLIENT_SECRET".into(),
        OsString::from_vec(vec![255]),
    );
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert!(result.is_ok());
    assert!(matches!(result.unwrap().kind, Kind::StaticToken(_)));
}

#[test]
fn greennode_service_precedes_legacy_token_and_same_source_iam() {
    let fixture = Fixture::new("[default]\nroot_email=file\nusername=file\npassword=file\n");
    let source = Map::default()
        .with("GREENNODE_CLIENT_ID", "toy-id")
        .with("GREENNODE_CLIENT_SECRET", "toy-secret")
        .with("GREENNODE_ROOT_EMAIL", "toy")
        .with("GREENNODE_USERNAME", "toy")
        .with("GREENNODE_PASSWORD", "toy")
        .with("VNGCLOUD_ACCESS_TOKEN", "toy-token");
    let result = select(
        &fixture.selection(Mode::Legacy),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    )
    .unwrap();
    assert_eq!(result.source, Source::GreennodeEnvironment);
    assert!(matches!(result.kind, Kind::ServiceAccount(_)));
    let invalid = Map::default()
        .with("GREENNODE_CLIENT_ID", "bad:id")
        .with("GREENNODE_CLIENT_SECRET", "toy")
        .with("VNGCLOUD_ACCESS_TOKEN", "toy");
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &invalid,
        Some(fixture.file()),
    );
    assert!(result.is_err());
    assert_eq!(
        result.err().unwrap().class(),
        crate::ErrorClass::InvalidConfig
    );
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &Map::default(),
        Some(fixture.file()),
    );
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.source, Source::File);
    assert!(matches!(result.kind, Kind::IamUser(_)));
}

#[test]
fn first_nonempty_management_source_wins_whole() {
    let fixture = Fixture::new("[default]\nclient_id=file-id\nclient_secret=file-secret\n");
    for mode in [Mode::Greennode, Mode::Legacy] {
        let source = Map::default().with("VNGCLOUD_ACCESS_TOKEN", "legacy-token");
        let selected = select(
            &fixture.selection(mode),
            &settings("default"),
            &SettingsOptions::default(),
            &Options::default(),
            &source,
            Some(fixture.file()),
        );
        assert!(selected.is_ok());
        let selected = selected.unwrap();
        assert_eq!(selected.source, Source::LegacyEnvironment);
        assert!(matches!(selected.kind, Kind::StaticToken(_)));
        let source = source.with("GREENNODE_CLIENT_ID", "partial-id");
        let error = select(
            &fixture.selection(mode),
            &settings("default"),
            &SettingsOptions::default(),
            &Options::default(),
            &source,
            Some(fixture.file()),
        );
        assert!(error.is_err());
        assert_eq!(
            error.err().unwrap().class(),
            crate::ErrorClass::NoCredentials
        );
    }
}

#[test]
fn explicit_profile_skips_both_environment_sources() {
    let fixture = Fixture::new(
        "[team]\nclient_id=file-id\nclient_secret=file-secret\nvcdn_api_key=file-key\n",
    );
    let source = Map::default()
        .with("GREENNODE_ACCESS_TOKEN", "token")
        .with("VNGCLOUD_VCDN_API_KEY", "env-key");
    let explicit = SettingsOptions {
        profile: Some("team".into()),
        ..SettingsOptions::default()
    };
    let selected = select(
        &fixture.selection(Mode::Greennode),
        &settings("team"),
        &explicit,
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert!(selected.is_ok());
    let selected = selected.unwrap();
    assert_eq!(selected.source, Source::File);
    assert!(matches!(selected.kind, Kind::ServiceAccount(_)));
    assert!(selected.cdn_key.unwrap().expose_secret() == "file-key");
}

#[test]
fn options_environment_and_file_order_in_both_modes() {
    let fixture = Fixture::new("[default]\nclient_id=toy\nclient_secret=toy\n");
    for mode in [Mode::Greennode, Mode::Legacy] {
        let source = Map::default()
            .with("GREENNODE_ACCESS_TOKEN", "toy")
            .with("VNGCLOUD_ACCESS_TOKEN", "toy");
        let options = Options {
            token: Some(Secret::new("toy")),
            service_account: Some(ServiceAccount {
                client_id: Secret::new("toy"),
                client_secret: Secret::new("toy"),
            }),
            ..Options::default()
        };
        let chosen = select(
            &fixture.selection(mode),
            &settings("default"),
            &SettingsOptions::default(),
            &options,
            &source,
            Some(fixture.file()),
        )
        .unwrap();
        assert_eq!(chosen.source, Source::Options);
        assert!(matches!(chosen.kind, Kind::StaticToken(_)));
        let chosen = select(
            &fixture.selection(mode),
            &settings("default"),
            &SettingsOptions::default(),
            &Options::default(),
            &source,
            Some(fixture.file()),
        )
        .unwrap();
        assert_eq!(chosen.source, Source::GreennodeEnvironment);
        let source = Map::default().with("VNGCLOUD_ACCESS_TOKEN", "toy");
        let chosen = select(
            &fixture.selection(mode),
            &settings("default"),
            &SettingsOptions::default(),
            &Options::default(),
            &source,
            Some(fixture.file()),
        )
        .unwrap();
        assert_eq!(chosen.source, Source::LegacyEnvironment);
        let chosen = select(
            &fixture.selection(mode),
            &settings("default"),
            &SettingsOptions::default(),
            &Options::default(),
            &Map::default(),
            Some(fixture.file()),
        )
        .unwrap();
        assert_eq!(chosen.source, Source::File);
        assert!(matches!(chosen.kind, Kind::ServiceAccount(_)));
    }
}

#[test]
fn explicit_profile_skips_legacy_management_in_both_modes() {
    let fixture = Fixture::new("[team]\nclient_id=file-id\nclient_secret=file-secret\n");
    let source = Map::default().with("VNGCLOUD_ACCESS_TOKEN", "toy");
    let explicit = SettingsOptions {
        profile: Some("team".into()),
        ..SettingsOptions::default()
    };
    for mode in [Mode::Greennode, Mode::Legacy] {
        let result = select(
            &fixture.selection(mode),
            &settings("team"),
            &explicit,
            &Options::default(),
            &source,
            Some(fixture.file()),
        )
        .unwrap();
        assert_eq!(result.source, Source::File);
    }
}

#[test]
fn empty_file_values_fall_through_but_environment_whitespace_is_preserved() {
    let fixture = Fixture::new(
        "[default]\nclient_id=   \nclient_secret= \nroot_email=toy\nusername=toy\npassword=toy\n",
    );
    let file = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &Map::default(),
        Some(fixture.file()),
    );
    assert!(file.is_ok());
    let file = file.unwrap();
    assert!(matches!(file.kind, Kind::IamUser(_)));
    let env = Map::default().with("GREENNODE_ACCESS_TOKEN", "  ");
    let selected = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &env,
        Some(fixture.file()),
    )
    .unwrap();
    assert_eq!(selected.source, Source::GreennodeEnvironment);
    match selected.kind {
        Kind::StaticToken(token) => assert!(token.expose_secret() == "  "),
        _ => panic!("expected static token"),
    }
    let invalid = Map::default()
        .with("GREENNODE_CLIENT_ID", " ")
        .with("GREENNODE_CLIENT_SECRET", "toy");
    let selected = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &invalid,
        Some(fixture.file()),
    );
    assert!(selected.is_err());
    assert_eq!(
        selected.err().unwrap().class(),
        crate::ErrorClass::InvalidConfig
    );
}

#[test]
fn option_service_account_precedes_option_iam() {
    let fixture = Fixture::new("[default]\n");
    let options = Options {
        service_account: Some(ServiceAccount {
            client_id: Secret::new("toy"),
            client_secret: Secret::new("toy"),
        }),
        iam_user: Some(IamUser {
            root_email: Secret::new("toy"),
            username: Secret::new("toy"),
            password: Secret::new("toy"),
            totp_secret: None,
        }),
        ..Options::default()
    };
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &options,
        &Map::default(),
        Some(fixture.file()),
    );
    assert!(result.is_ok());
    assert!(matches!(result.unwrap().kind, Kind::ServiceAccount(_)));
    let options = Options {
        iam_user: Some(IamUser {
            root_email: Secret::new("toy"),
            username: Secret::new("toy"),
            password: Secret::new("toy"),
            totp_secret: None,
        }),
        ..Options::default()
    };
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &options,
        &Map::default(),
        Some(fixture.file()),
    );
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.source, Source::Options);
    assert!(matches!(result.kind, Kind::IamUser(_)));
}

#[test]
fn method_order_and_provider_reservation() {
    let fixture = Fixture::new("[default]\n");
    let source = Map::default()
        .with("GREENNODE_ACCESS_TOKEN", "token")
        .with("GREENNODE_CLIENT_ID", "id")
        .with("GREENNODE_CLIENT_SECRET", "secret");
    let selected = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert!(matches!(selected.unwrap().kind, Kind::StaticToken(_)));
    let opts = Options {
        provider: true,
        token: Some(Secret::new("other")),
        ..Options::default()
    };
    let selected = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &opts,
        &source,
        Some(fixture.file()),
    );
    assert!(matches!(selected.unwrap().kind, Kind::Provider));
}

#[test]
fn iam_and_cdn_key_can_come_from_opposite_prefixes() {
    let fixture = Fixture::new("[default]\n");
    let source = Map::default()
        .with("GREENNODE_ROOT_EMAIL", "toy")
        .with("GREENNODE_USERNAME", "toy")
        .with("GREENNODE_PASSWORD", "toy")
        .with("VNGCLOUD_VCDN_API_KEY", "toy-key");
    for mode in [Mode::Greennode, Mode::Legacy] {
        let selected = select(
            &fixture.selection(mode),
            &settings("default"),
            &SettingsOptions::default(),
            &Options::default(),
            &source,
            Some(fixture.file()),
        );
        assert!(selected.is_ok());
        let selected = selected.unwrap();
        assert_eq!(selected.source, Source::GreennodeEnvironment);
        assert!(matches!(selected.kind, Kind::IamUser(_)));
        assert!(selected.cdn_key.is_some());
    }
}

#[test]
fn independent_cdn_key_does_not_supply_management() {
    let fixture = Fixture::new("[default]\n");
    let source = Map::default().with("VNGCLOUD_VCDN_API_KEY", "key");
    let missing = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert_eq!(
        missing.err().unwrap().class(),
        crate::ErrorClass::NoCredentials
    );
    let source = source.with("GREENNODE_ACCESS_TOKEN", "token");
    let selected = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert!(selected.unwrap().cdn_key.is_some());
}

#[test]
fn partial_iam_cannot_fall_back_and_options_reserve_source() {
    let fixture = Fixture::new("[default]\nclient_id=file-id\nclient_secret=file-secret\n");
    let source = Map::default()
        .with("GREENNODE_TOTP_SECRET", "toy")
        .with("VNGCLOUD_ACCESS_TOKEN", "toy");
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert!(result.is_err());
    assert_eq!(
        result.err().unwrap().class(),
        crate::ErrorClass::NoCredentials
    );
    let options = Options {
        service_account: Some(ServiceAccount {
            client_id: Secret::new(""),
            client_secret: Secret::new(""),
        }),
        ..Options::default()
    };
    let fallback = Map::default().with("VNGCLOUD_ACCESS_TOKEN", "toy");
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &options,
        &fallback,
        Some(fixture.file()),
    );
    assert!(result.is_err());
    assert_eq!(
        result.err().unwrap().class(),
        crate::ErrorClass::NoCredentials
    );
    let source = Map::default()
        .with("GREENNODE_ROOT_EMAIL", "toy")
        .with("GREENNODE_USERNAME", "toy")
        .with("GREENNODE_PASSWORD", "toy");
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert!(matches!(result.unwrap().kind, Kind::IamUser(_)));
}

#[test]
fn invalid_winning_cdn_key_never_falls_back() {
    let fixture = Fixture::new("[default]\nvcdn_api_key=file-key\n");
    let source = Map::default()
        .with("GREENNODE_ACCESS_TOKEN", "token")
        .with("GREENNODE_VCDN_API_KEY", "bad key")
        .with("VNGCLOUD_VCDN_API_KEY", "good-key");
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    );
    assert!(result.is_err());
    assert_eq!(
        result.err().unwrap().class(),
        crate::ErrorClass::InvalidConfig
    );
    let options = Options {
        cdn_key: Some(Secret::new("option-key")),
        ..Options::default()
    };
    let result = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &options,
        &source,
        Some(fixture.file()),
    );
    assert!(result.is_ok());
}

#[test]
fn incomplete_pair_refuses_before_credentials_selection() {
    let fixture = Fixture::new("[default]\nclient_id=toy\nclient_secret=toy\n");
    let green = fixture.0.join(".greennode");
    let legacy = fixture.0.join(".vngcloud");
    std::fs::create_dir(&green).unwrap();
    std::fs::create_dir(&legacy).unwrap();
    std::fs::rename(fixture.0.join("credentials"), green.join("credentials")).unwrap();
    std::fs::write(legacy.join("config"), "[default]\n").unwrap();
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let source = Map::default()
        .with(home, fixture.0.to_str().unwrap())
        .with("GREENNODE_ACCESS_TOKEN", "toy");
    let selected = crate::config::mode::select(
        None,
        None,
        &source,
        Some(&fixture.0),
        crate::config::mode::probe,
    );
    assert!(selected.is_err());
    assert_eq!(
        selected.err().unwrap().class(),
        crate::ErrorClass::InvalidConfig
    );
    std::fs::write(green.join("config"), "[default]\n").unwrap();
    assert!(
        crate::config::mode::select(
            None,
            None,
            &source,
            Some(&fixture.0),
            crate::config::mode::probe
        )
        .is_ok()
    );
}

#[test]
fn malformed_file_and_debug_do_not_disclose_values() {
    let fixture = Fixture::new("[default]\npassword=toy-canary\ntoy-canary\n");
    let error = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &Map::default(),
        Some(fixture.file()),
    )
    .err()
    .unwrap();
    assert_eq!(error.class(), crate::ErrorClass::CredentialsFile);
    assert!(error.has_identity(crate::ErrorClass::InvalidConfig));
    let detail = error.detail::<FileDiagnostic>().unwrap();
    assert_eq!(detail.source(), "~/.greennode");
    assert!(detail.line() == Some(3));
    assert!(!format!("{error:?} {error} {detail:?}").contains("toy-canary"));
    std::fs::write(fixture.0.join("credentials"), "[default]\n").unwrap();
    let source = Map::default().with("GREENNODE_ACCESS_TOKEN", "toy-canary");
    let selected = select(
        &fixture.selection(Mode::Greennode),
        &settings("default"),
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    )
    .unwrap();
    assert!(!format!("{selected:?}").contains("toy-canary"));
}

#[test]
fn diagnostic_rejects_untrusted_values() {
    let value = FileDiagnostic::from_text(
        crate::FileSlot::Credentials,
        Some("/toy-path"),
        "credentials:3: /toy-secret",
    );
    assert_eq!(value.source(), "unknown");
    assert_eq!(value.reason(), "unknown error");
    assert_eq!(value.line(), Some(3));
    assert!(!format!("{value:?}").contains("toy"));
}
