//! Account-level monitoring reads.

#![allow(
    clippy::result_large_err,
    reason = "SDK methods return Failure with the typed output"
)]

mod alarms;
mod channel_writes;
mod channels;
mod check_alarm;
mod check_writes;
mod checks;
mod dashboards;
mod history;
mod inventory;
mod log_projects;
mod mutation;
mod quota;
mod response_times;
mod wire;

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Config;
use crate::config::Session;
use crate::model::wire::{Field, WireField};
use crate::wire::product::evidence::Evidence;

pub use alarms::*;
pub use channel_writes::*;
pub use channels::*;
pub use check_alarm::*;
pub use check_writes::*;
pub use checks::*;
pub use dashboards::*;
pub use history::*;
pub use log_projects::*;
pub use mutation::WriteError;
pub use quota::*;
pub use response_times::*;

/// Monitor client with authentication shared by the configuration.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    check_gates: Arc<check_writes::CheckGates>,
}

impl Client {
    /// Creates a Monitor client without sending a request.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            check_gates: Arc::default(),
        }
    }
}

/// A typed response field whose value may be a reflected credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseField<T> {
    /// The response value was safe to retain.
    Value(T),
    /// The response value matched credential material.
    Redacted,
}

impl<T: Default> Default for ResponseField<T> {
    fn default() -> Self {
        Self::Value(T::default())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ResponseField<T> {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        T::deserialize(input).map(Self::Value)
    }
}

impl<T: Serialize> Serialize for ResponseField<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Redacted => serializer.serialize_str("<redacted>"),
        }
    }
}

impl<T: WireField> WireField for ResponseField<T> {
    fn read<'de, D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        Field::<T>::deserialize(input).map(|field| Self::Value(field.0))
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        if let Self::Value(value) = self
            && !value.redact(evidence)
        {
            *self = Self::Redacted;
        }
        !matches!(self, Self::Redacted) || !evidence.text_reflects("<redacted>")
    }
}
