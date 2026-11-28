//! Unofficial Rust SDK for GreenNode, formerly VNG Cloud.

#![warn(missing_docs)]

pub mod backup;
pub mod billing;
mod cache;
#[allow(clippy::result_large_err)]
pub mod cdn;
mod clock;
pub mod compute;
mod config;
pub mod containerregistry;
mod context;
pub mod database;
mod discovery;
pub mod dns;
mod endpoint;
mod error;
mod event;
mod file_diagnostic;
pub mod globalloadbalancer;
mod go_json;
mod go_text;
pub mod iam;
pub mod loadbalancer;
mod model;
pub mod monitor;
pub mod network;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod output_number;
pub mod portal;
pub mod pricing;
pub mod project;
mod provider;
mod quote;
mod secret;
mod session;
mod source;
pub mod tagging;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[allow(missing_docs)]
pub mod testing;
mod token;
pub mod vks;
pub mod volume;
mod wire;

pub use clock::*;
pub use config::{
    Config, ConfigOptions, EndpointOverrides, IamUserAuth, Mode, Paths, ProfileFile,
    ResolvedProfile, SecretTotp, Selection, ServiceAccountAuth, TotpFn, TotpProvider, read_slot,
    select,
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

pub use wire::product::absence::confirms_absence;

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
