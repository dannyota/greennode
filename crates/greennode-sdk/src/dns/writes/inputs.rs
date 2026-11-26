use serde::Serialize;

use super::super::{GetHostedZoneOutput, GetRecordOutput, RecordValue};

/// Creates a private hosted zone and associates it with VPCs.
#[derive(Clone, Debug)]
pub struct CreateHostedZoneInput {
    /// Domain name of the hosted zone.
    pub domain_name: String,
    /// One or more associated VPC identifiers.
    pub vpc_ids: Vec<String>,
    /// Description to send when creating the zone.
    pub description: String,
    /// Returns the create response without waiting for activation.
    pub no_wait: bool,
}

impl CreateHostedZoneInput {
    /// Creates an input with the required domain name and VPC associations.
    pub fn new(domain_name: impl Into<String>, vpc_ids: Vec<String>) -> Self {
        Self {
            domain_name: domain_name.into(),
            vpc_ids,
            description: String::new(),
            no_wait: false,
        }
    }
}

/// The created private hosted zone.
pub type CreateHostedZoneOutput = GetHostedZoneOutput;

/// Replaces the description or VPC associations of a hosted zone.
#[derive(Clone, Debug)]
pub struct UpdateHostedZoneInput {
    /// Hosted zone identifier.
    pub hosted_zone_id: String,
    /// Replacement VPC identifiers. An empty list detaches all VPCs.
    pub vpc_ids: Option<Vec<String>>,
    /// Replacement description.
    pub description: Option<String>,
    /// Reads the zone once after the update without waiting for settlement.
    pub no_wait: bool,
}

impl UpdateHostedZoneInput {
    /// Selects a hosted zone. Set at least one replacement field before use.
    pub fn new(hosted_zone_id: impl Into<String>) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
            vpc_ids: None,
            description: None,
            no_wait: false,
        }
    }
}

/// The updated hosted zone.
pub type UpdateHostedZoneOutput = GetHostedZoneOutput;

/// Deletes a hosted zone.
#[derive(Clone, Debug)]
pub struct DeleteHostedZoneInput {
    /// Hosted zone identifier.
    pub hosted_zone_id: String,
    /// Returns once deletion is accepted.
    pub no_wait: bool,
}

impl DeleteHostedZoneInput {
    /// Selects a hosted zone and waits for its deletion by default.
    pub fn new(hosted_zone_id: impl Into<String>) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
            no_wait: false,
        }
    }
}

/// A hosted zone deletion acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeleteHostedZoneOutput {}

/// Creates a record inside a hosted zone.
#[derive(Clone, Debug)]
pub struct CreateRecordInput {
    /// Parent hosted zone identifier.
    pub hosted_zone_id: String,
    /// DNS record type.
    pub record_type: String,
    /// One or more record values.
    pub values: Vec<RecordValue>,
    /// Relative record name. An empty value selects the apex.
    pub sub_domain: String,
    /// Time to live in seconds. Zero selects 300.
    pub ttl: i64,
    /// Routing policy. An empty value selects `simple-routing`.
    pub routing_policy: String,
    /// Optional sticky-session setting.
    pub sticky_session: Option<bool>,
    /// Returns the create response without waiting for activation.
    pub no_wait: bool,
}

impl CreateRecordInput {
    /// Selects a zone, record type and values for a new record.
    pub fn new(
        hosted_zone_id: impl Into<String>,
        record_type: impl Into<String>,
        values: Vec<RecordValue>,
    ) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
            record_type: record_type.into(),
            values,
            sub_domain: String::new(),
            ttl: 0,
            routing_policy: String::new(),
            sticky_session: None,
            no_wait: false,
        }
    }
}

/// The created record.
pub type CreateRecordOutput = GetRecordOutput;

/// Updates the supplied fields of a DNS record.
#[derive(Clone, Debug)]
pub struct UpdateRecordInput {
    /// Parent hosted zone identifier.
    pub hosted_zone_id: String,
    /// Record identifier.
    pub record_id: String,
    /// Replacement relative record name. An empty value selects the apex.
    pub sub_domain: Option<String>,
    /// Replacement DNS record type.
    pub record_type: Option<String>,
    /// Replacement time to live in seconds.
    pub ttl: Option<i64>,
    /// Replacement routing policy.
    pub routing_policy: Option<String>,
    /// Replacement record values.
    pub values: Option<Vec<RecordValue>>,
    /// Replacement sticky-session setting.
    pub sticky_session: Option<bool>,
    /// Reads the record once after the update without waiting for settlement.
    pub no_wait: bool,
}

impl UpdateRecordInput {
    /// Selects a record. Set at least one replacement field before use.
    pub fn new(hosted_zone_id: impl Into<String>, record_id: impl Into<String>) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
            record_id: record_id.into(),
            sub_domain: None,
            record_type: None,
            ttl: None,
            routing_policy: None,
            values: None,
            sticky_session: None,
            no_wait: false,
        }
    }
}

/// The updated record.
pub type UpdateRecordOutput = GetRecordOutput;

/// Deletes one DNS record.
#[derive(Clone, Debug)]
pub struct DeleteRecordInput {
    /// Parent hosted zone identifier.
    pub hosted_zone_id: String,
    /// Record identifier.
    pub record_id: String,
    /// Returns once deletion is accepted.
    pub no_wait: bool,
}

impl DeleteRecordInput {
    /// Selects a record and waits for its deletion by default.
    pub fn new(hosted_zone_id: impl Into<String>, record_id: impl Into<String>) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
            record_id: record_id.into(),
            no_wait: false,
        }
    }
}

/// A record deletion acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeleteRecordOutput {}
