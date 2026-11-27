use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, value::RawValue};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::wire::product::evidence::Evidence;

const REDACTED: &str = "<redacted>";

/// A typed field that can withhold a reflected credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field<T> {
    /// The response value was retained.
    Value(T),
    /// The response value matched credential material.
    Redacted,
}

impl<T: Default> Default for Field<T> {
    fn default() -> Self {
        Self::Value(T::default())
    }
}

impl<T: Serialize> Serialize for Field<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Redacted => serializer.serialize_str(REDACTED),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Value)
    }
}

impl<T: Serialize> Field<T> {
    pub(super) fn redact(&mut self, evidence: &Evidence) {
        if let Self::Value(value) = self
            && serde_json::to_value(value).map_or(true, |value| evidence.output_reflects(&value))
        {
            *self = Self::Redacted;
        }
    }
}

/// A timestamp with its offset and nanosecond precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp(pub OffsetDateTime);

impl Default for Timestamp {
    fn default() -> Self {
        Self(
            OffsetDateTime::from_unix_timestamp(-62_135_596_800)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        )
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = self.0.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        OffsetDateTime::parse(&text, &Rfc3339)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

pub(super) fn redact_text(text: &mut String, evidence: &Evidence) {
    if evidence.text_reflects(text) {
        *text = REDACTED.to_owned();
    }
}

pub(super) fn redact_id(text: &mut String, evidence: &Evidence) {
    if evidence.id_reflects(text) {
        *text = REDACTED.to_owned();
    }
}

fn redact_raw(raw: &mut Option<Box<RawValue>>, evidence: &Evidence) {
    let Some(text) = raw.as_ref() else { return };
    let raw_reflects = evidence.peer_body_reflects(text.get().as_bytes()) != Some(false);
    match serde_json::from_str::<Value>(text.get()) {
        Ok(mut value) if evidence.output_reflects(&value) => {
            *raw = evidence
                .redact(&mut value)
                .then(|| serde_json::value::to_raw_value(&value).ok())
                .flatten();
        }
        Ok(_) if !raw_reflects => {}
        _ => *raw = None,
    }
}

fn rule_from_wire<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(deserializer).map(Some)
}

fn serialize_rule<S: Serializer>(
    rule: &Option<Box<RawValue>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match rule {
        Some(rule) => rule.serialize(serializer),
        None => <[Value; 0]>::default().serialize(serializer),
    }
}

pub(super) fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

pub(super) fn flex_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let raw = Box::<RawValue>::deserialize(deserializer)?;
    let token = raw.get();
    match token.as_bytes().first() {
        Some(b'"') => serde_json::from_str(token).map_err(serde::de::Error::custom),
        Some(b'-' | b'0'..=b'9') => Ok(token.to_owned()),
        _ if token == "null" => Ok(String::new()),
        _ => Err(serde::de::Error::custom(
            "CDN identifier must be a string or number",
        )),
    }
}

/// Returns the name corresponding to a Web Accelerator status code.
pub fn status_name(status: i64) -> String {
    match status {
        0 => "DISABLED".into(),
        1 => "ACTIVE".into(),
        3 => "DEPLOYING".into(),
        4 => "DELETING".into(),
        5 => "DISABLING".into(),
        _ => format!("UNKNOWN({status})"),
    }
}

/// A certificate and its public PEM chain.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Certificate {
    /// Resource identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "cdnSslcertificateId"),
        deserialize_with = "nullable"
    )]
    pub id: String,
    /// CommonName returned by the service.
    #[serde(
        rename(serialize = "CommonName", deserialize = "commonName"),
        deserialize_with = "nullable"
    )]
    pub common_name: String,
    /// CName returned by the service.
    #[serde(
        rename(serialize = "CName", deserialize = "cname"),
        deserialize_with = "nullable"
    )]
    pub c_name: String,
    /// Issuer returned by the service.
    #[serde(
        rename(serialize = "Issuer", deserialize = "issuer"),
        deserialize_with = "nullable"
    )]
    pub issuer: String,
    /// ValidFrom returned by the service.
    #[serde(
        rename(serialize = "ValidFrom", deserialize = "validFrom"),
        deserialize_with = "nullable"
    )]
    pub valid_from: String,
    /// ExpiresOn returned by the service.
    #[serde(
        rename(serialize = "ExpiresOn", deserialize = "expiresOn"),
        deserialize_with = "nullable"
    )]
    pub expires_on: String,
    /// Status returned by the service.
    #[serde(
        rename(serialize = "Status", deserialize = "status"),
        deserialize_with = "nullable"
    )]
    pub status: Field<i64>,
    /// Number of CDNs using the certificate.
    #[serde(
        rename(serialize = "CDNUsing", deserialize = "cdnUsing"),
        deserialize_with = "nullable"
    )]
    pub cdn_using: Field<i64>,
    /// CreatedTime returned by the service.
    #[serde(
        rename(serialize = "CreatedTime", deserialize = "createdTime"),
        deserialize_with = "nullable"
    )]
    pub created_time: String,
    /// Public certificate in PEM format.
    #[serde(
        rename(serialize = "Certificate", deserialize = "certificate"),
        deserialize_with = "nullable"
    )]
    pub certificate: String,
    /// Public certificate authority chain in PEM format.
    #[serde(
        rename(serialize = "CARoot", deserialize = "caRoot"),
        deserialize_with = "nullable"
    )]
    pub ca_root: String,
}

impl Certificate {
    pub(super) fn redact(&mut self, evidence: &Evidence) {
        redact_id(&mut self.id, evidence);
        redact_text(&mut self.common_name, evidence);
        redact_text(&mut self.c_name, evidence);
        redact_text(&mut self.issuer, evidence);
        redact_text(&mut self.valid_from, evidence);
        redact_text(&mut self.expires_on, evidence);
        self.status.redact(evidence);
        self.cdn_using.redact(evidence);
        redact_text(&mut self.created_time, evidence);
        redact_text(&mut self.certificate, evidence);
        redact_text(&mut self.ca_root, evidence);
    }
}

/// Metadata for an API key.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ApiKey {
    /// Resource identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "apiKeyId"),
        deserialize_with = "nullable"
    )]
    pub id: Field<i64>,
    /// ExpiresAt returned by the service.
    #[serde(
        rename(serialize = "ExpiresAt", deserialize = "expiredDate"),
        deserialize_with = "nullable"
    )]
    pub expires_at: Field<Timestamp>,
    /// CreateTime returned by the service.
    #[serde(
        rename(serialize = "CreateTime", deserialize = "createTime"),
        deserialize_with = "nullable"
    )]
    pub create_time: Field<Timestamp>,
    /// UpdateTime returned by the service.
    #[serde(
        rename(serialize = "UpdateTime", deserialize = "updateTime"),
        deserialize_with = "nullable"
    )]
    pub update_time: Field<Timestamp>,
    /// AllowOriginHeader returned by the service.
    #[serde(
        rename(serialize = "AllowOriginHeader", deserialize = "allowOriginHeader"),
        deserialize_with = "nullable"
    )]
    pub allow_origin_header: String,
    /// Whether this key authenticated the request.
    #[serde(rename = "Current", skip_deserializing)]
    pub current: bool,
}

impl ApiKey {
    pub(super) fn redact(&mut self, evidence: &Evidence) {
        self.id.redact(evidence);
        self.expires_at.redact(evidence);
        self.create_time.redact(evidence);
        self.update_time.redact(evidence);
        redact_text(&mut self.allow_origin_header, evidence);
    }
}

/// A Web Accelerator in the account inventory.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WebAcceleratorSummary {
    /// Web Accelerator identifier.
    #[serde(
        rename(serialize = "CDNID", deserialize = "cdnId"),
        deserialize_with = "flex_id"
    )]
    pub cdn_id: String,
    /// DomainName returned by the service.
    #[serde(
        rename(serialize = "DomainName", deserialize = "domainName"),
        deserialize_with = "nullable"
    )]
    pub domain_name: String,
    /// Generated CDN domain.
    #[serde(
        rename(serialize = "CDNDomain", deserialize = "cdnDomain"),
        deserialize_with = "nullable"
    )]
    pub cdn_domain: String,
    /// Customer CNAMEs assigned to the CDN.
    #[serde(
        rename(serialize = "CNames", deserialize = "cName"),
        deserialize_with = "nullable"
    )]
    pub c_names: Vec<String>,
    /// Status returned by the service.
    #[serde(
        rename(serialize = "Status", deserialize = "status"),
        deserialize_with = "nullable"
    )]
    pub status: Field<i64>,
    /// Human-readable status derived from the status code.
    #[serde(rename = "StatusName", skip_deserializing)]
    pub status_name: String,
}

impl WebAcceleratorSummary {
    pub(super) fn redact(&mut self, evidence: &Evidence) {
        if let Field::Value(status) = self.status {
            self.status_name = status_name(status);
        }
        redact_id(&mut self.cdn_id, evidence);
        redact_text(&mut self.domain_name, evidence);
        redact_text(&mut self.cdn_domain, evidence);
        for text in &mut self.c_names {
            redact_text(text, evidence);
        }
        self.status.redact(evidence);
        redact_text(&mut self.status_name, evidence);
    }
}

/// An origin configured on a Web Accelerator.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Upstream {
    /// Resource identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "cdnUpstreamId"),
        deserialize_with = "flex_id"
    )]
    pub id: String,
    /// Priority returned by the service.
    #[serde(
        rename(serialize = "Priority", deserialize = "priority"),
        deserialize_with = "nullable"
    )]
    pub priority: Field<i64>,
    /// IPAddress returned by the service.
    #[serde(
        rename(serialize = "IPAddress", deserialize = "ipaddress"),
        deserialize_with = "nullable"
    )]
    pub ip_address: String,
    /// Status returned by the service.
    #[serde(
        rename(serialize = "Status", deserialize = "status"),
        deserialize_with = "nullable"
    )]
    pub status: Field<i64>,
}

impl Upstream {
    pub(super) fn redact(&mut self, evidence: &Evidence) {
        redact_id(&mut self.id, evidence);
        self.priority.redact(evidence);
        redact_text(&mut self.ip_address, evidence);
        self.status.redact(evidence);
    }
}

/// A default action configured on a Web Accelerator.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct RuleAction {
    /// Resource identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "id"),
        deserialize_with = "flex_id"
    )]
    pub id: String,
    /// Name returned by the service.
    #[serde(
        rename(serialize = "Name", deserialize = "actionName"),
        deserialize_with = "nullable"
    )]
    pub name: String,
    /// Action value in the server’s string encoding.
    #[serde(
        rename(serialize = "Value", deserialize = "value"),
        deserialize_with = "nullable"
    )]
    pub value: String,
    /// Order returned by the service.
    #[serde(
        rename(serialize = "Order", deserialize = "order"),
        deserialize_with = "nullable"
    )]
    pub order: Field<i64>,
}

impl RuleAction {
    pub(super) fn redact(&mut self, evidence: &Evidence) {
        redact_id(&mut self.id, evidence);
        redact_text(&mut self.name, evidence);
        redact_text(&mut self.value, evidence);
        self.order.redact(evidence);
    }
}

/// The configuration and state of a Web Accelerator.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct WebAccelerator {
    /// Web Accelerator identifier.
    #[serde(
        rename(serialize = "CDNID", deserialize = "cdnId"),
        deserialize_with = "flex_id"
    )]
    pub cdn_id: String,
    /// CDN resource type.
    #[serde(
        rename(serialize = "Type", deserialize = "type"),
        deserialize_with = "nullable"
    )]
    pub kind: String,
    /// DomainName returned by the service.
    #[serde(
        rename(serialize = "DomainName", deserialize = "domainName"),
        deserialize_with = "nullable"
    )]
    pub domain_name: String,
    /// Generated CDN domain.
    #[serde(
        rename(serialize = "CDNDomain", deserialize = "cdnDomain"),
        deserialize_with = "nullable"
    )]
    pub cdn_domain: String,
    /// Customer CNAMEs assigned to the CDN.
    #[serde(
        rename(serialize = "CNames", deserialize = "cName"),
        deserialize_with = "nullable"
    )]
    pub c_names: Vec<String>,
    /// Status returned by the service.
    #[serde(
        rename(serialize = "Status", deserialize = "status"),
        deserialize_with = "nullable"
    )]
    pub status: Field<i64>,
    /// Human-readable status derived from the status code.
    #[serde(rename = "StatusName", skip_deserializing)]
    pub status_name: String,
    /// Identifier of the assigned certificate.
    #[serde(
        rename(serialize = "CertificateID", deserialize = "sslId"),
        deserialize_with = "nullable"
    )]
    pub certificate_id: String,
    /// Origin load-balancing policy.
    #[serde(
        rename(serialize = "LBType", deserialize = "lbType"),
        deserialize_with = "nullable"
    )]
    pub lb_type: String,
    /// OriginHostHeader returned by the service.
    #[serde(
        rename(serialize = "OriginHostHeader", deserialize = "originHostHeader"),
        deserialize_with = "nullable"
    )]
    pub origin_host_header: String,
    /// FailOverErrorCodes returned by the service.
    #[serde(
        rename(serialize = "FailOverErrorCodes", deserialize = "failOverErrorCode"),
        deserialize_with = "nullable"
    )]
    pub fail_over_error_codes: Vec<String>,
    /// UseSSL returned by the service.
    #[serde(
        rename(serialize = "UseSSL", deserialize = "useSsl"),
        deserialize_with = "nullable"
    )]
    pub use_ssl: bool,
    /// UseSmallFile returned by the service.
    #[serde(
        rename(serialize = "UseSmallFile", deserialize = "useSmallFile"),
        deserialize_with = "nullable"
    )]
    pub use_small_file: bool,
    /// EnableGzip returned by the service.
    #[serde(
        rename(serialize = "EnableGzip", deserialize = "enableGzip"),
        deserialize_with = "nullable"
    )]
    pub enable_gzip: bool,
    /// Upstreams returned by the service.
    #[serde(
        rename(serialize = "Upstreams", deserialize = "upstreams"),
        deserialize_with = "nullable"
    )]
    pub upstreams: Vec<Upstream>,
    /// DefaultRuleActions returned by the service.
    #[serde(
        rename(serialize = "DefaultRuleActions", deserialize = "defaultRuleAction"),
        deserialize_with = "nullable"
    )]
    pub default_rule_actions: Vec<RuleAction>,
    /// PageRules returned by the service.
    #[serde(
        rename(serialize = "PageRules", deserialize = "childrenRule"),
        deserialize_with = "rule_from_wire",
        serialize_with = "serialize_rule"
    )]
    pub page_rules: Option<Box<RawValue>>,
    /// AdvancedRule returned by the service.
    #[serde(
        rename(serialize = "AdvancedRule", deserialize = "childrenAdvanceRule"),
        deserialize_with = "rule_from_wire",
        serialize_with = "serialize_rule"
    )]
    pub advanced_rule: Option<Box<RawValue>>,
}

impl WebAccelerator {
    pub(super) fn redact(&mut self, evidence: &Evidence) {
        if let Field::Value(status) = self.status {
            self.status_name = status_name(status);
        }
        redact_id(&mut self.cdn_id, evidence);
        redact_text(&mut self.kind, evidence);
        redact_text(&mut self.domain_name, evidence);
        redact_text(&mut self.cdn_domain, evidence);
        for text in &mut self.c_names {
            redact_text(text, evidence);
        }
        self.status.redact(evidence);
        redact_text(&mut self.status_name, evidence);
        redact_id(&mut self.certificate_id, evidence);
        redact_text(&mut self.lb_type, evidence);
        redact_text(&mut self.origin_host_header, evidence);
        for text in &mut self.fail_over_error_codes {
            redact_text(text, evidence);
        }
        for item in &mut self.upstreams {
            item.redact(evidence);
        }
        for item in &mut self.default_rule_actions {
            item.redact(evidence);
        }
        redact_raw(&mut self.page_rules, evidence);
        redact_raw(&mut self.advanced_rule, evidence);
    }
}

#[cfg(test)]
mod screening_tests {
    use super::*;
    use crate::Secret;

    #[test]
    fn raw_cdn_rules_withhold_reflection_in_original_number_spelling() {
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new("1E06"));
        let mut rule = Some(RawValue::from_string(r#"{"score":1E06}"#.into()).unwrap());
        redact_raw(&mut rule, &evidence);
        assert!(
            rule.is_none()
                || rule
                    .as_ref()
                    .is_some_and(|value| !value.get().contains("1E06"))
        );
    }

    #[test]
    fn cdn_certificate_redacts_short_encoding_inside_identifier() {
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new("jkl"));
        let mut certificate = Certificate {
            id: "pre6A6B6Cpost".into(),
            ..Default::default()
        };
        certificate.redact(&evidence);
        assert!(certificate.id == REDACTED);
    }
}
