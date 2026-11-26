//! Unofficial Rust SDK for GreenNode, formerly VNG Cloud. No API is implemented yet.

#![warn(missing_docs)]

mod cache;
mod clock;
mod config;
mod context;
mod discovery;
mod endpoint;
mod error;
mod event;
mod file_diagnostic;
mod model;
mod provider;
mod secret;
mod session;
mod source;
mod token;
mod wire;

pub use clock::*;
pub use config::{
    Config, ConfigOptions, EndpointOverrides, IamUserAuth, ResolvedProfile, SecretTotp,
    ServiceAccountAuth, TotpFn, TotpProvider,
};
pub use context::*;
pub use error::*;
pub use event::*;
pub use file_diagnostic::{FileDiagnostic, FileSlot};
pub use model::*;
pub use provider::CredentialsProvider;
pub use secret::Secret;
pub use source::*;
pub use token::Token;

#[cfg(test)]
mod iam_entry_tests {
    use super::{
        Clock, Secret, SystemClock,
        wire::credential::iam_user::flow::{Login, run},
    };
    use std::time::Duration;

    #[tokio::test]
    async fn iam_entry_requires_no_wire_client() {
        let origin = url::Url::parse("http://localhost/").unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/").unwrap();
        let token_url = url::Url::parse("https://token.test/token").unwrap();
        let password = Secret::new("toy");
        let login = Login {
            origin: &origin,
            dashboard: &dashboard,
            root_email: "root",
            username: "user",
            password: &password,
            totp: None,
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
        };
        let result = run(login, &token_url).await;
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().reason(), "signin page");
    }
}
