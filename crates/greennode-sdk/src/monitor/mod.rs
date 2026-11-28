//! Account-level monitoring reads.

#![allow(
    clippy::result_large_err,
    reason = "SDK methods return Failure with the typed output"
)]

mod check_alarm;
mod dashboards;
mod quota;
mod response_times;
mod wire;
pub(crate) use wire::response_read_failure;

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Config;
use crate::config::Session;
use crate::model::wire::{Field, WireField};
use crate::wire::product::evidence::Evidence;

pub use check_alarm::*;
pub use dashboards::*;
pub use quota::*;
pub use response_times::*;

/// Monitor client with authentication shared by the configuration.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

impl Client {
    /// Creates a Monitor client without sending a request.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
        }
    }
}

/// A typed response field whose value may be a reflected credential.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResponseField<T> {
    /// The response value was safe to retain.
    Value(T),
    /// The response value matched credential material.
    Redacted,
}

crate::error::debug_record!(ResponseField<T>; Value, Redacted);

impl<T: crate::wire::product::output::PublicOutput> crate::wire::product::output::PublicOutput
    for ResponseField<T>
{
    fn public_reflects(
        &self,
        scan: &crate::wire::product::output::FieldScan<'_>,
        identifier: bool,
    ) -> bool {
        match self {
            Self::Value(value) => value.public_reflects(scan, identifier),
            Self::Redacted => false,
        }
    }
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
