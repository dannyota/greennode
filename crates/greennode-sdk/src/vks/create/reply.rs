use super::{CreateClusterOutput, valid_id};
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{Operation, Output},
    response::sanitize_error_text,
    sender::Response,
};
use crate::wire::strict_json::{Shape, ShapeField, field_matches, validate_shape};
use crate::{ApiError, Error};
use http::StatusCode;
use regex_lite::Regex;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use std::{fmt, sync::LazyLock};

const WITHHELD: &str = "provider response withheld";
const MESSAGE_LIMIT: usize = 512;

#[derive(Default)]
pub(super) enum Outcome {
    #[default]
    Unknown,
    Accepted(CreateClusterOutput),
    Unconfirmed(CreateClusterOutput),
    Refused(Option<String>),
}

#[derive(Default, Deserialize)]
pub(super) struct CreateReply {
    #[serde(skip)]
    pub(super) outcome: Outcome,
    #[serde(skip)]
    pub(super) acknowledged: Option<crate::wire::product::guard::AcknowledgedId>,
}

impl Output for CreateReply {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "id",
            shape: Shape::Any,
        },
        ShapeField {
            name: "status",
            shape: Shape::Any,
        },
    ]);

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let receipt = response.clean_receipt();
        let outcome = outcome(
            operation,
            response.status,
            receipt.as_ref().map_or(&[][..], |receipt| receipt.body()),
        );
        let acknowledged = match &outcome {
            Outcome::Accepted(output) | Outcome::Unconfirmed(output) if !output.id.is_empty() => {
                receipt.map(|receipt| receipt.bind_id(output.id.clone()))
            }
            _ => None,
        };
        Ok(Self {
            outcome,
            acknowledged,
        })
    }
}

fn outcome(operation: &Operation, status: StatusCode, body: &[u8]) -> Outcome {
    if operation.success.contains(&status) {
        return decode_accepted(body, operation.cap);
    }
    if status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT {
        return Outcome::Refused(rejection_reason(body));
    }
    Outcome::Unknown
}

#[derive(Default)]
struct Accepted {
    output: CreateClusterOutput,
    wrong_type: bool,
}

impl<'de> Deserialize<'de> for Accepted {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Accepted;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a cluster object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Accepted, M::Error> {
                let mut reply = Accepted::default();
                while let Some((key, value)) = map.next_entry::<String, &RawValue>()? {
                    let field = if field_matches("id", &key) {
                        &mut reply.output.id
                    } else if field_matches("status", &key) {
                        &mut reply.output.status
                    } else {
                        continue;
                    };
                    match serde_json::from_str::<Option<String>>(value.get()) {
                        Ok(Some(value)) => *field = value,
                        Ok(None) => {}
                        Err(_) => reply.wrong_type = true,
                    }
                }
                Ok(reply)
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

fn decode_accepted(body: &[u8], limit: usize) -> Outcome {
    if validate_shape(body, &CreateReply::SCHEMA, limit).is_err() {
        return Outcome::Unconfirmed(CreateClusterOutput::default());
    }
    let Ok(Accepted {
        mut output,
        wrong_type,
    }) = serde_json::from_slice(body)
    else {
        return Outcome::Unconfirmed(CreateClusterOutput::default());
    };
    let invalid_status =
        !output.status.is_empty() && !super::super::write::valid_status(&output.status);
    if !valid_id(&output.id) {
        output.id.clear();
        if wrong_type || invalid_status {
            output.status.clear();
        }
        return Outcome::Unconfirmed(output);
    }
    if wrong_type || invalid_status {
        output.status.clear();
        return Outcome::Unconfirmed(output);
    }
    Outcome::Accepted(output)
}

fn rejection_reason(body: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Envelope {
        error: ProviderError,
    }

    #[derive(Deserialize)]
    struct ProviderError {
        message: String,
    }

    static UNSAFE: LazyLock<Result<Regex, regex_lite::Error>> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)bearer|token|password|secret|credential|authorization|api[ _-]?key|",
            r"\b(?:cookie|session)\b\s*(?:[:=]|\bis\b)\s*[^\s,;]+|",
            r"\b[a-z][a-z0-9]*-[a-z0-9]*[0-9][a-z0-9-]*\b|\b[0-9]{8,}\b|",
            r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|",
            r"[\w.+-]+@[\w.-]+\.[a-z]{2,}",
        ))
    });
    let envelope: Envelope = serde_json::from_slice(body).ok()?;
    let message = envelope.error.message.trim();
    if message.is_empty()
        || message.len() > MESSAGE_LIMIT
        || !message.bytes().all(|byte| (32..=126).contains(&byte))
        || UNSAFE.as_ref().ok()?.is_match(message)
    {
        return None;
    }
    Some(message.to_owned())
}

pub(super) fn refusal<'a>(
    status: StatusCode,
    reason: Option<String>,
    ids: impl Iterator<Item = &'a str>,
    evidence: &Evidence,
) -> Error {
    let credentials = evidence.credentials();
    let message = reason.and_then(|mut message| {
        if sanitize_error_text(&message, credentials).as_str() != message {
            return None;
        }
        for id in ids {
            message = message.replace(id, "[redacted]");
            if message.len() > MESSAGE_LIMIT {
                return None;
            }
        }
        let sanitized = sanitize_error_text(&message, credentials);
        (sanitized.as_str() == message).then_some(sanitized)
    });
    Error::from_api(ApiError::new(
        super::CREATE,
        Some(status.as_u16()),
        SafeText::new("", &[]),
        message.unwrap_or_else(|| SafeText::new(WITHHELD, &[])),
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorClass, Secret};

    #[test]
    fn recovers_only_valid_fields_from_malformed_acceptance() {
        let limit = super::super::create_operation().cap;
        for (body, id, status) in [
            (&br#"{"id":"cluster-1","status":123}"#[..], "cluster-1", ""),
            (br#"{"status":123,"Id":"cluster-1"}"#, "cluster-1", ""),
            (
                br#"{"id":"cluster-1","status":"unsafe status"}"#,
                "cluster-1",
                "",
            ),
            (br#"{"id":"bad/path","status":123}"#, "", ""),
            (br#"{"id":"cluster-1","status":}"#, "", ""),
            (br#"{"id":"bad/path","status":"CREATING"}"#, "", "CREATING"),
            (br#"{"id":"bad/path","status":"unsafe status"}"#, "", ""),
        ] {
            let Outcome::Unconfirmed(output) = decode_accepted(body, limit) else {
                panic!("expected an unconfirmed cluster order");
            };
            assert_eq!((output.id.as_str(), output.status.as_str()), (id, status));
        }
        let Outcome::Accepted(output) =
            decode_accepted(br#"{"id":"cluster-1","status":null}"#, limit)
        else {
            panic!("acceptance with no status should keep the ID");
        };
        assert_eq!(output.id, "cluster-1");
        assert!(output.status.is_empty());
    }

    #[test]
    fn duplicate_cluster_identifiers_leave_no_id_to_poll() {
        let operation = super::super::create_operation();
        for body in [
            &br#"{"id":"cluster-1","ID":"cluster-2","status":"ACTIVE"}"#[..],
            br#"{"id":"cluster-1","id":"cluster-1"}"#,
            br#"{"id":"cluster-1","id":null}"#,
        ] {
            let Outcome::Unconfirmed(output) = outcome(&operation, StatusCode::ACCEPTED, body)
            else {
                panic!("duplicate identifier must leave the order unconfirmed");
            };
            assert!(output.id.is_empty());
        }
    }

    #[test]
    fn refusals_withhold_encoded_credentials_before_and_after_id_redaction() {
        let operation = super::super::create_operation();
        let mut evidence = Evidence::new();
        for value in ["mock+abc", "toy/value", "some/value in [redacted]"] {
            evidence.add_secret(&Secret::new(value));
        }
        for message in [
            "Bad value mock%2Babc",
            "Bad value toy%252Fvalue",
            "Bad value some%2Fvalue in vpc",
        ] {
            let body =
                serde_json::to_vec(&serde_json::json!({"error": {"message": message}})).unwrap();
            let status = StatusCode::BAD_REQUEST;
            let Outcome::Refused(reason) = outcome(&operation, status, &body) else {
                panic!("expected a definite refusal");
            };
            let error = refusal(status, reason, ["vpc"].into_iter(), &evidence);
            assert!(error.api_error().unwrap().message() == WITHHELD);
        }
    }

    #[test]
    fn distinguishes_definite_refusals_from_uncertain_statuses() {
        let operation = super::super::create_operation();
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new("toy-reflection"));
        for status in [302, 408, 500, 204] {
            assert!(matches!(
                outcome(&operation, StatusCode::from_u16(status).unwrap(), b""),
                Outcome::Unknown,
            ));
        }
        for (status, class) in [
            (400, ErrorClass::Api),
            (401, ErrorClass::Auth),
            (403, ErrorClass::PermissionDenied),
            (404, ErrorClass::NotFound),
            (429, ErrorClass::RateLimited),
        ] {
            let status = StatusCode::from_u16(status).unwrap();
            let Outcome::Refused(reason) = outcome(&operation, status, b"") else {
                panic!("expected a definite refusal");
            };
            let error = refusal(status, reason, std::iter::empty(), &evidence);
            assert_eq!(error.class(), class);
            let api = error.api_error().unwrap();
            assert_eq!(api.status(), Some(status.as_u16()));
            assert_eq!(api.message(), WITHHELD);
            assert!(!api.retryable());
        }
        for (message, expected) in [
            (
                "Disk type does not match the subnet zone.",
                "Disk type does not match the subnet zone.",
            ),
            (
                "Subnet vpc is in the wrong zone.",
                "Subnet [redacted] is in the wrong zone.",
            ),
            ("Authorization: anything", WITHHELD),
            ("Contact person@example.test for help", WITHHELD),
            ("Order order-123 is invalid", WITHHELD),
            ("Bad value toy-reflection", WITHHELD),
        ] {
            let body =
                serde_json::to_vec(&serde_json::json!({"error": {"message": message}})).unwrap();
            let reason = rejection_reason(&body);
            let error = refusal(
                StatusCode::BAD_REQUEST,
                reason,
                ["vpc"].into_iter(),
                &evidence,
            );
            assert!(error.api_error().unwrap().message() == expected);
        }
    }
}
