use serde::{
    Serialize, Serializer,
    ser::{SerializeSeq, SerializeStruct},
};
use serde_json::Value;

use crate::model::wire::WireField;
use crate::wire::product::evidence::Evidence;

use super::Listener;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ListenerProjection(Value);

impl ListenerProjection {
    pub(super) fn retain(listener: &mut Listener, evidence: &Evidence) -> Result<Option<Self>, ()> {
        if evidence.text_reflects("Listener") {
            return Err(());
        }
        let mut value = serde_json::to_value(&*listener).map_err(|_| ())?;
        if !evidence.output_reflects(&value) {
            return Ok(None);
        }
        if !evidence.redact(&mut value) {
            return Err(());
        }
        scrub_listener(listener, evidence);
        let typed = serde_json::to_value(&*listener).map_err(|_| ())?;
        if evidence.output_reflects(&typed) {
            return Err(());
        }
        Ok(Some(Self(value)))
    }
}

impl Serialize for ListenerProjection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Row(&self.0, LISTENER).serialize(serializer)
    }
}

const LISTENER: &[&str] = &[
    "UUID",
    "Name",
    "Description",
    "Protocol",
    "ProtocolPort",
    "ConnectionLimit",
    "DefaultPoolID",
    "DefaultPoolName",
    "TimeoutClient",
    "TimeoutMember",
    "TimeoutConnection",
    "AllowedCIDRs",
    "CertificateAuthorities",
    "DisplayStatus",
    "CreatedAt",
    "UpdatedAt",
    "DefaultCertificateAuthority",
    "ClientCertificateAuthentication",
    "ProgressStatus",
    "InsertHeaders",
];
const HEADER: &[&str] = &["HeaderName", "HeaderValue"];

struct Row<'a>(&'a Value, &'static [&'static str]);

impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(fields) = self.0.as_object() else {
            return self.0.serialize(serializer);
        };
        let mut row = serializer.serialize_struct("ListenerFields", fields.len())?;
        for &key in self.1 {
            if let Some(value) = fields.get(key) {
                match key {
                    "InsertHeaders" => row.serialize_field(key, &Headers(value))?,
                    _ => row.serialize_field(key, value)?,
                }
            }
        }
        row.end()
    }
}

struct Headers<'a>(&'a Value);

impl Serialize for Headers<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(items) = self.0.as_array() else {
            return self.0.serialize(serializer);
        };
        let mut sequence = serializer.serialize_seq(Some(items.len()))?;
        for item in items {
            sequence.serialize_element(&Row(item, HEADER))?;
        }
        sequence.end()
    }
}

fn scrub_listener(listener: &mut Listener, evidence: &Evidence) {
    for text in [
        &mut listener.uuid,
        &mut listener.name,
        &mut listener.description,
        &mut listener.protocol,
        &mut listener.default_pool_id,
        &mut listener.default_pool_name,
        &mut listener.allowed_cidrs,
        &mut listener.display_status,
        &mut listener.created_at,
        &mut listener.updated_at,
        &mut listener.progress_status,
    ] {
        text.redact(evidence);
    }
    for text in [
        &mut listener.default_certificate_authority,
        &mut listener.client_certificate_authentication,
    ] {
        text.redact(evidence);
    }
    for certificate in &mut listener.certificate_authorities {
        certificate.redact(evidence);
    }
    for header in &mut listener.insert_headers {
        header.header_name.redact(evidence);
        header.header_value.redact(evidence);
    }
    for number in [
        &mut listener.protocol_port,
        &mut listener.connection_limit,
        &mut listener.timeout_client,
        &mut listener.timeout_member,
        &mut listener.timeout_connection,
    ] {
        if evidence.output_reflects(&Value::from(*number)) {
            *number = 0;
        }
    }
}
