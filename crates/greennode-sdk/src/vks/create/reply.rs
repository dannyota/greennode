use super::{CreateClusterOutput, valid_id};
use crate::error::SafeText;
use crate::wire::product::{
    operation::{Operation, Output},
    sender::{Response, SentCredentials},
};
use crate::wire::strict_json::Shape;
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
}

impl Output for CreateReply {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        Ok(Self {
            outcome: outcome(
                operation,
                response.status,
                &response.body,
                &response.credentials,
            ),
        })
    }
}

pub(super) fn outcome(
    operation: &Operation,
    status: StatusCode,
    body: &[u8],
    credentials: &SentCredentials,
) -> Outcome {
    if operation.success.contains(&status) {
        return decode_accepted(body);
    }
    if status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT {
        return Outcome::Refused(rejection_reason(body, credentials));
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
                    let field = if key.eq_ignore_ascii_case("id") {
                        &mut reply.output.id
                    } else if key.eq_ignore_ascii_case("status") {
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

fn decode_accepted(body: &[u8]) -> Outcome {
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

fn rejection_reason(body: &[u8], credentials: &SentCredentials) -> Option<String> {
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
        || credentials.reflects(message.as_bytes())
        || credentials.values().iter().any(|value| {
            let text = value.expose_secret();
            let escaped = serde_json::to_string(text).ok();
            escaped
                .as_deref()
                .and_then(|value| value.strip_prefix('"')?.strip_suffix('"'))
                .is_some_and(|value| !value.is_empty() && message.contains(value))
                || text
                    .lines()
                    .map(str::trim)
                    .any(|line| line.len() >= 8 && message.contains(line))
        })
    {
        return None;
    }
    Some(message.to_owned())
}

pub(super) fn refusal<'a>(
    status: StatusCode,
    reason: Option<String>,
    ids: impl Iterator<Item = &'a str>,
    credentials: &SentCredentials,
) -> Error {
    let message = reason.and_then(|mut message| {
        for id in ids {
            message = message.replace(id, "[redacted]");
            if message.len() > MESSAGE_LIMIT {
                return None;
            }
        }
        (!credentials.reflects(message.as_bytes())).then_some(message)
    });
    Error::from_api(ApiError::new(
        super::CREATE,
        Some(status.as_u16()),
        SafeText::new("", &[]),
        SafeText::new(message.as_deref().unwrap_or(WITHHELD), &[]),
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorClass, Secret};

    #[test]
    fn recovers_only_valid_fields_from_malformed_acceptance() {
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
            let Outcome::Unconfirmed(output) = decode_accepted(body) else {
                panic!("expected an unconfirmed cluster order");
            };
            assert_eq!((output.id.as_str(), output.status.as_str()), (id, status));
        }
        let Outcome::Accepted(output) = decode_accepted(br#"{"id":"cluster-1","status":null}"#)
        else {
            panic!("acceptance with no status should keep the ID");
        };
        assert_eq!(output.id, "cluster-1");
        assert!(output.status.is_empty());
    }

    #[test]
    fn distinguishes_definite_refusals_from_uncertain_statuses() {
        let operation = super::super::create_operation();
        let mut credentials = SentCredentials::default();
        credentials.record(&Secret::new("toy-reflection"));
        for status in [302, 408, 500, 204] {
            assert!(matches!(
                outcome(
                    &operation,
                    StatusCode::from_u16(status).unwrap(),
                    b"",
                    &credentials
                ),
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
            let Outcome::Refused(reason) = outcome(&operation, status, b"", &credentials) else {
                panic!("expected a definite refusal");
            };
            let error = refusal(status, reason, std::iter::empty(), &credentials);
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
            let reason = rejection_reason(&body, &credentials);
            let error = refusal(
                StatusCode::BAD_REQUEST,
                reason,
                ["vpc"].into_iter(),
                &credentials,
            );
            assert!(error.api_error().unwrap().message() == expected);
        }
    }
}
