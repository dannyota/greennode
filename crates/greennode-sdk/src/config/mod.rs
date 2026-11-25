#![allow(dead_code)]

mod credentials;
mod file;
mod ini;
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
#[derive(Default)]
pub struct ConfigOptions {
    profile: Option<String>,
    config_file: Option<std::path::PathBuf>,
    credentials_file: Option<std::path::PathBuf>,
}

impl ConfigOptions {
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
    _private: (),
}

/// The settings and file provenance selected by an offline lookup.
pub struct ResolvedProfile {
    name: String,
    settings: std::collections::BTreeMap<String, String>,
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
        if let Some(end) = ctx.check(&crate::SystemClock) {
            return Err(end.into());
        }
        let cwd = std::path::absolute(".").ok();
        let selection = mode::select(
            options.config_file.as_deref(),
            options.credentials_file.as_deref(),
            source,
            cwd.as_deref(),
            mode::probe,
        )
        .map_err(|error| *error)?;
        let file = file::open_settings(&selection).map_err(|error| *error)?;
        let local_options = settings::Options {
            profile: options.profile.clone(),
            ..Default::default()
        };
        let resolved = settings::resolve(
            &selection,
            file,
            &local_options,
            source,
            settings::Policy::Read,
        )
        .map_err(|error| match error {
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
            config_source: selection.config_source,
            credentials_source: selection.credentials_source,
        })
    }
}
