use super::mode::Selection;
use super::settings::{Options as SettingsOptions, Settings};
use crate::{ConfigSource, Error, Secret};
use std::fs::File;

#[derive(Default)]
pub(super) struct Options {
    pub provider: bool,
    pub token: Option<Secret>,
    pub service_account: Option<ServiceAccount>,
    pub iam_user: Option<IamUser>,
    pub cdn_key: Option<Secret>,
}

#[derive(Debug)]
pub(super) struct ServiceAccount {
    pub client_id: Secret,
    pub client_secret: Secret,
}

#[derive(Debug)]
pub(super) struct IamUser {
    pub root_email: Secret,
    pub username: Secret,
    pub password: Secret,
    pub totp_secret: Option<Secret>,
}

#[derive(Debug)]
pub(super) enum Kind {
    Provider,
    StaticToken(Secret),
    ServiceAccount(ServiceAccount),
    IamUser(IamUser),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Source {
    Options,
    GreennodeEnvironment,
    LegacyEnvironment,
    File,
}

#[derive(Debug)]
pub(super) struct Selected {
    pub source: Source,
    pub kind: Kind,
    pub cdn_key: Option<Secret>,
}

pub(super) use crate::FileDiagnostic;

#[derive(Default)]
struct Fields {
    token: Option<Secret>,
    client_id: Option<Secret>,
    client_secret: Option<Secret>,
    root_email: Option<Secret>,
    username: Option<Secret>,
    password: Option<Secret>,
    totp: Option<Secret>,
}

fn present(value: &Option<Secret>) -> bool {
    value
        .as_ref()
        .is_some_and(|v| !v.expose_secret().is_empty())
}

fn failure(class: crate::ErrorClass) -> Box<Error> {
    Box::new(Error::new(class))
}

fn service_account(
    id: Option<Secret>,
    secret: Option<Secret>,
) -> Result<ServiceAccount, Box<Error>> {
    let id = id
        .filter(|v| !v.expose_secret().is_empty())
        .ok_or_else(|| failure(crate::ErrorClass::NoCredentials))?;
    let secret = secret
        .filter(|v| !v.expose_secret().is_empty())
        .ok_or_else(|| failure(crate::ErrorClass::NoCredentials))?;
    let invalid = |s: &Secret| {
        s.expose_secret().len() > 4096
            || s.expose_secret()
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
    };
    if invalid(&id) || invalid(&secret) || id.expose_secret().contains(':') {
        return Err(failure(crate::ErrorClass::InvalidConfig));
    }
    Ok(ServiceAccount {
        client_id: id,
        client_secret: secret,
    })
}

fn iam_user(
    root: Option<Secret>,
    username: Option<Secret>,
    password: Option<Secret>,
    totp: Option<Secret>,
) -> Result<IamUser, Box<Error>> {
    let required = |v: Option<Secret>| {
        v.filter(|s| !s.expose_secret().is_empty())
            .ok_or_else(|| failure(crate::ErrorClass::NoCredentials))
    };
    Ok(IamUser {
        root_email: required(root)?,
        username: required(username)?,
        password: required(password)?,
        totp_secret: totp.filter(|s| !s.expose_secret().is_empty()),
    })
}

fn choose(fields: Fields) -> Result<Option<Kind>, Box<Error>> {
    if present(&fields.token) {
        return Ok(fields.token.map(Kind::StaticToken));
    }
    if present(&fields.client_id) || present(&fields.client_secret) {
        return service_account(fields.client_id, fields.client_secret)
            .map(|v| Some(Kind::ServiceAccount(v)));
    }
    if present(&fields.root_email)
        || present(&fields.username)
        || present(&fields.password)
        || present(&fields.totp)
    {
        return iam_user(
            fields.root_email,
            fields.username,
            fields.password,
            fields.totp,
        )
        .map(|v| Some(Kind::IamUser(v)));
    }
    Ok(None)
}

fn env_value(source: &dyn ConfigSource, name: &str) -> Result<Option<Secret>, Box<Error>> {
    source
        .var_os(std::ffi::OsStr::new(name))
        .filter(|s| !s.is_empty())
        .map(|v| {
            v.into_string()
                .map(Secret::new)
                .map_err(|_| failure(crate::ErrorClass::InvalidConfig))
        })
        .transpose()
}

fn env_fields(source: &dyn ConfigSource, names: [&str; 7]) -> Result<Fields, Box<Error>> {
    let token = env_value(source, names[0])?;
    if present(&token) {
        return Ok(Fields {
            token,
            ..Fields::default()
        });
    }
    let client_id = env_value(source, names[1])?;
    let client_secret = env_value(source, names[2])?;
    if present(&client_id) || present(&client_secret) {
        return Ok(Fields {
            client_id,
            client_secret,
            ..Fields::default()
        });
    }
    Ok(Fields {
        root_email: env_value(source, names[3])?,
        username: env_value(source, names[4])?,
        password: env_value(source, names[5])?,
        totp: env_value(source, names[6])?,
        ..Fields::default()
    })
}

pub(super) fn select(
    selection: &Selection,
    settings: &Settings,
    settings_options: &SettingsOptions,
    options: &Options,
    source: &dyn ConfigSource,
    file: Option<File>,
) -> Result<Selected, Box<Error>> {
    let mut ini = if selection.credentials_present {
        let file = file.ok_or_else(|| failure(crate::ErrorClass::NoCredentials))?;
        super::ini::parse_secrets(file).map_err(|diagnostic| {
            Box::new(Error::new(crate::ErrorClass::CredentialsFile).with_detail(
                FileDiagnostic::from_text(
                    crate::FileSlot::Credentials,
                    selection.credentials_source,
                    &diagnostic,
                ),
            ))
        })?
    } else {
        if selection.credentials_named || file.is_some() {
            return Err(failure(crate::ErrorClass::NoCredentials));
        }
        super::ini::SecretsIni::default()
    };
    if settings.profile != "default"
        && !settings.section_present
        && !ini.0.contains_key(&settings.profile)
    {
        return Err(failure(crate::ErrorClass::InvalidConfig));
    }
    let mut section = ini.0.remove(&settings.profile).unwrap_or_default();
    let file_cdn = section.remove("vcdn_api_key");
    let file_fields = Fields {
        client_id: section.remove("client_id"),
        client_secret: section.remove("client_secret"),
        root_email: section.remove("root_email"),
        username: section.remove("username"),
        password: section.remove("password"),
        totp: section.remove("totp_secret"),
        ..Fields::default()
    };
    let explicit = settings_options
        .profile
        .as_ref()
        .is_some_and(|p| !p.is_empty());
    let chosen = if options.provider {
        Some((Source::Options, Kind::Provider))
    } else if present(&options.token) {
        options
            .token
            .clone()
            .map(|v| (Source::Options, Kind::StaticToken(v)))
    } else if let Some(value) = &options.service_account {
        Some((
            Source::Options,
            Kind::ServiceAccount(service_account(
                Some(value.client_id.clone()),
                Some(value.client_secret.clone()),
            )?),
        ))
    } else if let Some(value) = &options.iam_user {
        Some((
            Source::Options,
            Kind::IamUser(iam_user(
                Some(value.root_email.clone()),
                Some(value.username.clone()),
                Some(value.password.clone()),
                value.totp_secret.clone(),
            )?),
        ))
    } else {
        None
    };
    let chosen = if chosen.is_some() || explicit {
        chosen
    } else {
        let green = env_fields(
            source,
            [
                "GREENNODE_ACCESS_TOKEN",
                "GREENNODE_CLIENT_ID",
                "GREENNODE_CLIENT_SECRET",
                "GREENNODE_ROOT_EMAIL",
                "GREENNODE_USERNAME",
                "GREENNODE_PASSWORD",
                "GREENNODE_TOTP_SECRET",
            ],
        )?;
        match choose(green)? {
            Some(value) => Some((Source::GreennodeEnvironment, value)),
            None => {
                let legacy = env_fields(
                    source,
                    [
                        "VNGCLOUD_ACCESS_TOKEN",
                        "VNGCLOUD_CLIENT_ID",
                        "VNGCLOUD_CLIENT_SECRET",
                        "VNGCLOUD_ROOT_EMAIL",
                        "VNGCLOUD_USERNAME",
                        "VNGCLOUD_PASSWORD",
                        "VNGCLOUD_TOTP_SECRET",
                    ],
                )?;
                choose(legacy)?.map(|value| (Source::LegacyEnvironment, value))
            }
        }
    };
    let (source_kind, kind) = match chosen {
        Some(value) => value,
        None => (
            Source::File,
            choose(file_fields)?.ok_or_else(|| failure(crate::ErrorClass::NoCredentials))?,
        ),
    };
    let mut cdn_key = options
        .cdn_key
        .clone()
        .filter(|v| !v.expose_secret().is_empty());
    if cdn_key.is_none() && !explicit {
        cdn_key = env_value(source, "GREENNODE_VCDN_API_KEY")?;
        if cdn_key.is_none() {
            cdn_key = env_value(source, "VNGCLOUD_VCDN_API_KEY")?;
        }
    }
    if cdn_key.is_none() {
        cdn_key = file_cdn.filter(|v| !v.expose_secret().is_empty());
    }
    if cdn_key.as_ref().is_some_and(|v| {
        v.expose_secret().len() > 4096
            || v.expose_secret()
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
    }) {
        return Err(failure(crate::ErrorClass::InvalidConfig));
    }
    Ok(Selected {
        source: source_kind,
        kind,
        cdn_key,
    })
}

#[cfg(test)]
mod tests;
