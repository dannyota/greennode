use super::mode::Selection;
use crate::ConfigSource;

#[derive(Default)]
pub(super) struct Options {
    pub profile: Option<String>,
    pub region: Option<String>,
    pub project_id: Option<String>,
    pub read_only: bool,
    pub output: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Policy {
    Read,
    Write,
    S3Read,
    S3Write,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SettingsError {
    InvalidConfig,
    ReadOnly(&'static str),
    Parse {
        mode: super::mode::Mode,
        source: Option<&'static str>,
        diagnostic: String,
    },
}

#[derive(PartialEq, Eq)]
pub(super) struct Settings {
    pub profile: String,
    pub region: Option<String>,
    pub project_id: Option<String>,
    pub read_only_source: Option<&'static str>,
    pub file_read_only: Option<String>,
    pub output: String,
    pub section_present: bool,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Settings([REDACTED])")
    }
}

fn parse_latch(value: Option<&str>, validate: bool) -> Result<bool, SettingsError> {
    match value.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ if !validate => Ok(false),
        _ => Err(SettingsError::InvalidConfig),
    }
}

pub(super) fn preflight(
    options: &Options,
    source: &dyn ConfigSource,
    policy: Policy,
) -> Result<Option<&'static str>, SettingsError> {
    use std::ffi::OsStr;
    let latch = |name: &str| {
        source
            .var_os(OsStr::new(name))
            .map(|v| v.to_string_lossy().into_owned())
    };
    let green = latch("GREENNODE_READ_ONLY");
    let legacy = latch("VNGCLOUD_READ_ONLY");
    let green_on = parse_latch(green.as_deref(), policy != Policy::Read)?;
    let legacy_on = parse_latch(legacy.as_deref(), policy != Policy::Read)?;
    let enabled = if options.read_only {
        Some("option")
    } else if green_on {
        Some("GREENNODE_READ_ONLY")
    } else if legacy_on {
        Some("VNGCLOUD_READ_ONLY")
    } else {
        None
    };
    if matches!(policy, Policy::Write | Policy::S3Write)
        && let Some(source) = enabled
    {
        return Err(SettingsError::ReadOnly(source));
    }
    Ok(enabled)
}

pub(super) fn resolve(
    paths: &Selection,
    config_file: Option<std::fs::File>,
    options: &Options,
    source: &dyn ConfigSource,
    policy: Policy,
) -> Result<Settings, SettingsError> {
    use super::{Slot, ini};
    use std::ffi::OsStr;
    let mut read_only_source = preflight(options, source, policy)?;
    let variable = |name: &str| -> Result<Option<String>, SettingsError> {
        source
            .var_os(OsStr::new(name))
            .filter(|s| !s.is_empty())
            .map(|s| s.into_string().map_err(|_| SettingsError::InvalidConfig))
            .transpose()
    };
    let pair = |first: &str, second: &str| -> Result<Option<String>, SettingsError> {
        match variable(first)? {
            Some(value) => Ok(Some(value)),
            None => variable(second),
        }
    };
    let nonempty = |value: &Option<String>| value.as_ref().filter(|s| !s.is_empty()).cloned();
    let explicit = nonempty(&options.profile);
    let profile = match explicit.clone() {
        Some(value) => value,
        None => pair("GREENNODE_PROFILE", "VNGCLOUD_PROFILE")?.unwrap_or_else(|| "default".into()),
    };
    if profile.trim() != profile || profile.contains(['\r', '\n', '[', ']']) {
        return Err(SettingsError::InvalidConfig);
    }
    let ini = match &paths.config {
        None if matches!(policy, Policy::S3Read | Policy::S3Write) => {
            return Err(SettingsError::InvalidConfig);
        }
        None => ini::Ini::default(),
        Some(_) if !paths.config_present && paths.config_named => {
            return Err(SettingsError::InvalidConfig);
        }
        Some(_) if !paths.config_present => ini::Ini::default(),
        Some(_) => ini::parse(
            Slot::Config,
            config_file.ok_or(SettingsError::InvalidConfig)?,
        )
        .map_err(|diagnostic| SettingsError::Parse {
            mode: paths.mode,
            source: paths.config_source,
            diagnostic,
        })?,
    };
    let section_name = if profile == "default" {
        profile.clone()
    } else {
        format!("profile {profile}")
    };
    let section = ini.get(&section_name);
    if matches!(policy, Policy::S3Read | Policy::S3Write)
        && profile != "default"
        && section.is_none()
    {
        return Err(SettingsError::InvalidConfig);
    }
    let setting = |name: &str| {
        section
            .and_then(|s| s.get(name))
            .filter(|s| !s.is_empty())
            .cloned()
    };
    if setting("client_id").is_some() || setting("client_secret").is_some() {
        return Err(SettingsError::InvalidConfig);
    }
    let file_region = setting("region");
    let region = match nonempty(&options.region) {
        Some(value) => Some(value),
        None => pair("GREENNODE_REGION", "VNGCLOUD_REGION")?.or(file_region.clone()),
    };
    let mut project_id = nonempty(&options.project_id);
    if project_id.is_none() && explicit.is_none() {
        project_id = pair("GREENNODE_PROJECT_ID", "VNGCLOUD_PROJECT_ID")?;
    }
    if project_id.is_none() && (file_region.is_none() || file_region == region) {
        project_id = setting("project_id");
    }
    let file_latch = section.and_then(|values| values.get("read_only")).cloned();
    let validate_file_latch = matches!(policy, Policy::S3Read | Policy::S3Write);
    if parse_latch(file_latch.as_deref(), validate_file_latch)? && read_only_source.is_none() {
        read_only_source = Some("config");
    }
    if policy == Policy::S3Write
        && let Some(source) = read_only_source
    {
        return Err(SettingsError::ReadOnly(source));
    }
    let output = nonempty(&options.output)
        .or_else(|| setting("output"))
        .unwrap_or_else(|| "json".into());
    Ok(Settings {
        profile,
        region,
        project_id,
        read_only_source,
        file_read_only: file_latch,
        output,
        section_present: section.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::mode::{Mode, Paths};
    use std::collections::HashMap;
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn resolve(
        paths: &Selection,
        options: &Options,
        source: &dyn ConfigSource,
        policy: Policy,
    ) -> Result<Settings, SettingsError> {
        preflight(options, source, policy)?;
        let config = if paths.config_present {
            let path = paths.config.as_ref().ok_or(SettingsError::InvalidConfig)?;
            Some(
                crate::config::file::open(path, crate::config::Slot::Config)
                    .map_err(|_| SettingsError::InvalidConfig)?,
            )
        } else {
            None
        };
        super::resolve(paths, config, options, source, policy)
    }

    #[derive(Default)]
    struct Source {
        vars: HashMap<OsString, OsString>,
        reads: Mutex<Vec<OsString>>,
    }
    impl ConfigSource for Source {
        fn var_os(&self, name: &OsStr) -> Option<OsString> {
            self.reads.lock().unwrap().push(name.to_owned());
            self.vars.get(name).cloned()
        }
    }
    impl Source {
        fn with(mut self, name: &str, value: &str) -> Self {
            self.vars.insert(name.into(), value.into());
            self
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(content: &str) -> Self {
            let dir = env::temp_dir().join(format!(
                "greennode-settings-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("config"), content).unwrap();
            Self(dir)
        }
        fn paths(&self) -> Selection {
            Selection {
                paths: Paths {
                    mode: Mode::Greennode,
                    config: Some(self.0.join("config")),
                    credentials: Some(self.0.join("credentials")),
                    config_named: false,
                    credentials_named: false,
                },
                config_present: true,
                credentials_present: false,
                config_source: Some("~/.greennode"),
                credentials_source: Some("~/.greennode"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn reads_preopened_config_not_replacement() {
        let fixture = Fixture::new("[default]\nregion=first\n");
        let selection = fixture.paths();
        let opened = crate::config::file::open(
            selection.config.as_ref().unwrap(),
            crate::config::Slot::Config,
        )
        .unwrap();
        let replacement = fixture.0.join("replacement");
        std::fs::write(&replacement, "[default]\nregion=second\n").unwrap();
        std::fs::rename(replacement, fixture.0.join("config")).unwrap();
        let result = super::resolve(
            &selection,
            Some(opened),
            &Options::default(),
            &Source::default(),
            Policy::Read,
        )
        .unwrap();
        assert_eq!(result.region.as_deref(), Some("first"));
    }

    #[test]
    fn observed_config_cannot_disappear_before_open() {
        let fixture = Fixture::new("[default]\nregion=file\n");
        let paths = fixture.paths();
        let source = Source::default();
        let resolved = resolve(&paths, &Options::default(), &source, Policy::Read).unwrap();
        assert_eq!(resolved.output, "json");
        std::fs::remove_file(fixture.0.join("config")).unwrap();
        assert_eq!(
            resolve(&paths, &Options::default(), &source, Policy::Read),
            Err(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn management_write_carries_malformed_file_latch() {
        let fixture = Fixture::new("[default]\nread_only=maybe\n");
        let settings = resolve(
            &fixture.paths(),
            &Options::default(),
            &Source::default(),
            Policy::Write,
        );
        assert!(settings.is_ok());
        let settings = settings.unwrap();
        assert_eq!(settings.file_read_only.as_deref(), Some("maybe"));
        assert_eq!(
            resolve(
                &fixture.paths(),
                &Options::default(),
                &Source::default(),
                Policy::S3Read
            )
            .err(),
            Some(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn preflight_orchestrates_before_mode_and_profile() {
        fn run(options: &Options, source: &Source) -> Result<Settings, SettingsError> {
            preflight(options, source, Policy::Write)?;
            let paths = crate::config::mode::select(None, None, source, None, |_| {
                crate::config::mode::Presence::Absent
            })
            .map_err(|_| SettingsError::InvalidConfig)?;
            resolve(&paths, options, source, Policy::Write)
        }
        let source = Source::default().with("GREENNODE_PROFILE", "bad[profile");
        let options = Options {
            read_only: true,
            ..Options::default()
        };
        assert_eq!(
            run(&options, &source).err(),
            Some(SettingsError::ReadOnly("option"))
        );
        assert!(source.reads.lock().unwrap().iter().all(|name| matches!(
            name.to_str(),
            Some("GREENNODE_READ_ONLY" | "VNGCLOUD_READ_ONLY")
        )));
        let source = Source::default().with("GREENNODE_PROFILE", "bad[profile");
        assert_eq!(
            run(&Options::default(), &source).err(),
            Some(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn read_only_precedes_invalid_profile() {
        let fixture = Fixture::new("[default]\nregion=one\n");
        let source = Source::default().with("GREENNODE_PROFILE", "bad[profile");
        let options = Options {
            read_only: true,
            ..Options::default()
        };
        assert_eq!(
            resolve(&fixture.paths(), &options, &source, Policy::Write).err(),
            Some(SettingsError::ReadOnly("option"))
        );
        assert_eq!(
            resolve(&fixture.paths(), &Options::default(), &source, Policy::Read).err(),
            Some(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn parse_error_reports_default_or_override_source_without_path() {
        use crate::config::mode::{probe, select};
        let fixture = Fixture::new("[default]\nregion=x\nmalformed\n");
        let home_name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = fixture.0.to_str().unwrap();
        std::fs::create_dir(fixture.0.join(".greennode")).unwrap();
        std::fs::write(
            fixture.0.join(".greennode/config"),
            "[default]\nregion=x\nmalformed\n",
        )
        .unwrap();
        let default_source = Source::default().with(home_name, home);
        let default_paths = select(None, None, &default_source, Some(&fixture.0), probe).unwrap();
        let default_error = resolve(
            &default_paths,
            &Options::default(),
            &default_source,
            Policy::Read,
        )
        .err()
        .unwrap();
        let override_path = fixture.0.join("config");
        let override_source = Source::default()
            .with(home_name, home)
            .with("GREENNODE_CONFIG_FILE", override_path.to_str().unwrap());
        let override_paths = select(None, None, &override_source, Some(&fixture.0), probe).unwrap();
        let override_error = resolve(
            &override_paths,
            &Options::default(),
            &override_source,
            Policy::Read,
        )
        .err()
        .unwrap();
        assert!(matches!(
            &default_error,
            SettingsError::Parse {
                source: Some("~/.greennode"),
                ..
            }
        ));
        assert!(matches!(
            &override_error,
            SettingsError::Parse {
                source: Some("GREENNODE_CONFIG_FILE"),
                ..
            }
        ));
        assert!(!format!("{default_error:?} {override_error:?}").contains(home));
    }

    #[test]
    fn malformed_config_keeps_safe_diagnostic() {
        let fixture = Fixture::new("[default]\nregion=one\ncanary without equals\n");
        let error = resolve(
            &fixture.paths(),
            &Options::default(),
            &Source::default(),
            Policy::Read,
        )
        .err()
        .unwrap();
        assert!(matches!(&error, SettingsError::Parse { .. }));
        match error {
            SettingsError::Parse {
                mode, diagnostic, ..
            } => {
                assert_eq!(mode, Mode::Greennode);
                assert!(diagnostic.starts_with("config:3:"));
                assert!(!diagnostic.contains("canary"));
                assert!(!diagnostic.contains(&fixture.0.to_string_lossy().to_string()));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn legacy_sources_remain_eligible_in_each_mode() {
        let fixture = Fixture::new("[profile team]\nregion=file-region\nproject_id=file-project\n");
        for mode in [Mode::Greennode, Mode::Legacy] {
            let mut paths = fixture.paths();
            paths.paths.mode = mode;
            let source = Source::default()
                .with("VNGCLOUD_PROFILE", "team")
                .with("VNGCLOUD_REGION", "legacy-region")
                .with("VNGCLOUD_PROJECT_ID", "legacy-project")
                .with("VNGCLOUD_READ_ONLY", "true");
            let inherited = resolve(&paths, &Options::default(), &source, Policy::S3Read).unwrap();
            assert_eq!(inherited.region.as_deref(), Some("legacy-region"));
            assert_eq!(inherited.project_id.as_deref(), Some("legacy-project"));
            assert_eq!(inherited.read_only_source, Some("VNGCLOUD_READ_ONLY"));
            let explicit = Options {
                profile: Some("team".into()),
                region: Some("file-region".into()),
                ..Options::default()
            };
            let resolved = resolve(&paths, &explicit, &source, Policy::S3Read).unwrap();
            assert_eq!(resolved.project_id.as_deref(), Some("file-project"));
        }
    }

    #[test]
    fn management_write_defers_profile_latch_to_full_load() {
        let fixture = Fixture::new("[default]\nread_only=true\noutput=text\n");
        let result = resolve(
            &fixture.paths(),
            &Options::default(),
            &Source::default(),
            Policy::Write,
        );
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(resolved.read_only_source, Some("config"));
        assert_eq!(resolved.output, "text");
    }

    #[test]
    fn prefixes_profile_region_project_and_output_follow_precedence() {
        let fixture = Fixture::new(
            "[profile team]\nregion=file-region\nproject_id=file-project\noutput=table\n",
        );
        let source = Source::default()
            .with("GREENNODE_PROFILE", "")
            .with("VNGCLOUD_PROFILE", "team")
            .with("GREENNODE_REGION", "green-region")
            .with("VNGCLOUD_REGION", "legacy-region")
            .with("GREENNODE_PROJECT_ID", "green-project")
            .with("VNGCLOUD_PROJECT_ID", "legacy-project");
        let resolved = resolve(
            &fixture.paths(),
            &Options::default(),
            &source,
            Policy::S3Read,
        )
        .unwrap();
        assert_eq!(resolved.profile, "team");
        assert_eq!(resolved.region.as_deref(), Some("green-region"));
        assert_eq!(resolved.project_id.as_deref(), Some("green-project"));
        assert_eq!(resolved.output, "table");
        let options = Options {
            profile: Some("team".into()),
            region: Some("option-region".into()),
            project_id: Some("option-project".into()),
            output: Some("text".into()),
            ..Options::default()
        };
        let resolved = resolve(&fixture.paths(), &options, &source, Policy::S3Read).unwrap();
        assert_eq!(resolved.region.as_deref(), Some("option-region"));
        assert_eq!(resolved.project_id.as_deref(), Some("option-project"));
        assert_eq!(resolved.output, "text");
    }

    #[cfg(unix)]
    #[test]
    fn management_read_ignores_non_unicode_latch() {
        use std::os::unix::ffi::OsStringExt;
        let fixture = Fixture::new("[default]\nregion=one\n");
        let mut source = Source::default();
        source
            .vars
            .insert("GREENNODE_READ_ONLY".into(), OsString::from_vec(vec![255]));
        assert!(resolve(&fixture.paths(), &Options::default(), &source, Policy::Read).is_ok());
        assert_eq!(
            resolve(
                &fixture.paths(),
                &Options::default(),
                &source,
                Policy::S3Read
            ),
            Err(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn latch_validates_both_variables_and_names_first_on_source() {
        let fixture = Fixture::new("[default]\nread_only=true\n");
        let source = Source::default()
            .with("GREENNODE_READ_ONLY", " TrUe ")
            .with("VNGCLOUD_READ_ONLY", "1");
        let settings = resolve(
            &fixture.paths(),
            &Options::default(),
            &source,
            Policy::S3Read,
        )
        .unwrap();
        assert_eq!(settings.read_only_source, Some("GREENNODE_READ_ONLY"));
        let options = Options {
            read_only: true,
            ..Options::default()
        };
        assert_eq!(
            resolve(&fixture.paths(), &options, &source, Policy::Write),
            Err(SettingsError::ReadOnly("option"))
        );
        let invalid = Source::default()
            .with("GREENNODE_READ_ONLY", "true")
            .with("VNGCLOUD_READ_ONLY", "bad");
        assert_eq!(
            resolve(&fixture.paths(), &options, &invalid, Policy::Write),
            Err(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn explicit_profile_disables_only_inherited_project() {
        let fixture = Fixture::new("[profile work]\nregion=file\nproject_id=file-project\n");
        let source = Source::default()
            .with("GREENNODE_PROFILE", "work")
            .with("GREENNODE_REGION", "file")
            .with("GREENNODE_PROJECT_ID", "env-project");
        let ordinary = resolve(
            &fixture.paths(),
            &Options::default(),
            &source,
            Policy::S3Read,
        )
        .unwrap();
        assert_eq!(ordinary.profile, "work");
        assert_eq!(ordinary.project_id.as_deref(), Some("env-project"));
        let options = Options {
            profile: Some("work".into()),
            ..Options::default()
        };
        let explicit = resolve(&fixture.paths(), &options, &source, Policy::S3Read).unwrap();
        assert_eq!(explicit.region.as_deref(), Some("file"));
        assert_eq!(explicit.project_id.as_deref(), Some("file-project"));
    }

    #[test]
    fn file_project_is_scoped_to_file_region() {
        let fixture = Fixture::new("[default]\nregion=one\nproject_id=file-project\n");
        let source = Source::default();
        let settings = resolve(
            &fixture.paths(),
            &Options::default(),
            &source,
            Policy::S3Read,
        )
        .unwrap();
        assert_eq!(settings.project_id.as_deref(), Some("file-project"));
        let options = Options {
            region: Some("other".into()),
            ..Options::default()
        };
        let settings = resolve(&fixture.paths(), &options, &source, Policy::S3Read).unwrap();
        assert_eq!(settings.project_id, None);
        let inherited = Source::default().with("VNGCLOUD_PROJECT_ID", "inherited");
        let settings = resolve(&fixture.paths(), &options, &inherited, Policy::S3Read).unwrap();
        assert_eq!(settings.project_id.as_deref(), Some("inherited"));
        std::fs::write(fixture.0.join("config"), "[default]\nproject_id=unscoped\n").unwrap();
        let settings = resolve(&fixture.paths(), &options, &source, Policy::S3Read).unwrap();
        assert_eq!(settings.project_id.as_deref(), Some("unscoped"));
    }

    #[test]
    fn latch_and_s3_policy_are_checked_before_reads() {
        let fixture = Fixture::new("[default]\nread_only=1\n");
        let options = Options::default();
        let source = Source::default();
        assert_eq!(
            resolve(&fixture.paths(), &options, &source, Policy::S3Read)
                .unwrap()
                .read_only_source,
            Some("config")
        );
        assert_eq!(
            resolve(&fixture.paths(), &options, &source, Policy::S3Write),
            Err(SettingsError::ReadOnly("config"))
        );
        let source = Source::default().with("GREENNODE_READ_ONLY", "true");
        let mut paths = fixture.paths();
        paths.paths.config = Some(fixture.0.join("missing"));
        paths.paths.config_named = true;
        assert_eq!(
            resolve(&paths, &options, &source, Policy::Write),
            Err(SettingsError::ReadOnly("GREENNODE_READ_ONLY"))
        );
        let source = Source::default().with("GREENNODE_READ_ONLY", "nonsense");
        assert_eq!(
            resolve(&fixture.paths(), &options, &source, Policy::S3Read),
            Err(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn s3_requires_named_section_and_valid_latch() {
        let fixture = Fixture::new("[default]\nread_only=maybe\n[profile existing]\nread_only=0\n");
        let source = Source::default();
        assert!(resolve(&fixture.paths(), &Options::default(), &source, Policy::Read).is_ok());
        assert_eq!(
            resolve(
                &fixture.paths(),
                &Options::default(),
                &source,
                Policy::S3Read
            ),
            Err(SettingsError::InvalidConfig)
        );
        let existing = Options {
            profile: Some("existing".into()),
            ..Options::default()
        };
        assert!(resolve(&fixture.paths(), &existing, &source, Policy::S3Read).is_ok());
        let missing = Options {
            profile: Some("missing".into()),
            ..Options::default()
        };
        assert_eq!(
            resolve(&fixture.paths(), &missing, &source, Policy::S3Read),
            Err(SettingsError::InvalidConfig)
        );
        let mut paths = fixture.paths();
        paths.paths.config = None;
        assert_eq!(
            resolve(&paths, &Options::default(), &source, Policy::S3Read),
            Err(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn settings_debug_omits_values() {
        let fixture = Fixture::new(
            "[default]\nregion=canary-region\nproject_id=canary-project\noutput=canary-output\n",
        );
        let settings = resolve(
            &fixture.paths(),
            &Options::default(),
            &Source::default(),
            Policy::Read,
        )
        .unwrap();
        assert!(!format!("{settings:?}").contains("canary"));
    }

    #[test]
    fn reserved_config_credentials_are_refused() {
        let fixture = Fixture::new("[default]\nclient_id=\nregion=one\n");
        let source = Source::default();
        assert!(
            resolve(
                &fixture.paths(),
                &Options::default(),
                &source,
                Policy::S3Read
            )
            .is_ok()
        );
        std::fs::write(fixture.0.join("config"), "[default]\nclient_secret=toy\n").unwrap();
        assert_eq!(
            resolve(
                &fixture.paths(),
                &Options::default(),
                &source,
                Policy::S3Read
            ),
            Err(SettingsError::InvalidConfig)
        );
    }

    #[test]
    fn settings_lookup_never_queries_credentials() {
        let fixture = Fixture::new("[default]\nregion=one\n");
        let source = Source::default();
        let settings = resolve(
            &fixture.paths(),
            &Options::default(),
            &source,
            Policy::S3Read,
        )
        .unwrap();
        assert_eq!(settings.region.as_deref(), Some("one"));
        let names = source.reads.lock().unwrap();
        for name in names.iter() {
            let name = name.to_string_lossy();
            assert!(matches!(
                name.as_ref(),
                "GREENNODE_PROFILE"
                    | "VNGCLOUD_PROFILE"
                    | "GREENNODE_REGION"
                    | "VNGCLOUD_REGION"
                    | "GREENNODE_PROJECT_ID"
                    | "VNGCLOUD_PROJECT_ID"
                    | "GREENNODE_READ_ONLY"
                    | "VNGCLOUD_READ_ONLY"
            ));
        }
    }
}
