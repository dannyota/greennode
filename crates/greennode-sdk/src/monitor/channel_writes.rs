mod ack;
mod privacy;

use http::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;

use super::inventory::get_input;
use super::mutation::{Mutation, SendGuard, WriteError, before_write, maybe_sent, not_sent};
use super::response_times::{safe_time, serialize_time};
use super::wire::{check_path_id, input_error, requires_failure};
use super::{Channel, ChannelHeader, Client};
use crate::model::wire::Field;
use crate::quote::request::monitor::privacy_limit;
use crate::wire::product::evidence::Evidence;
use crate::wire::product::redirect::Retry;
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext, Secret};
use ack::{CREATED, CreateAck};
use privacy::{channel_values, validate_channel};

const CHANNEL: &str = "notification-gateway/api/v1/notification";
const CREATE: Mutation = Mutation {
    retry: Retry::NonIdempotent,
    ..Mutation::standard("monitor.CreateChannel", CHANNEL, &Method::POST)
};
const UPDATE: Mutation = Mutation::standard("monitor.UpdateChannel", CHANNEL, &Method::PUT);
const DELETE: Mutation = Mutation {
    private: false,
    retry: Retry::Repeatable,
    ..Mutation::standard(
        "monitor.DeleteChannel",
        "notification-gateway/api/v1/notification/{}",
        &Method::DELETE,
    )
};
const SEND: Mutation = Mutation {
    retry: Retry::NonIdempotent,
    ..Mutation::standard(
        "monitor.SendChannelOTP",
        "notification-gateway/api/v1/notification/otps",
        &Method::POST,
    )
};
const VALIDATE_ROUTE: &str = "notification-gateway/api/v1/notification/otps/validate";

/// Configures a notification channel to create.
#[derive(Clone)]
pub struct CreateChannelInput {
    /// Channel display name.
    pub name: String,
    /// Notification type.
    pub r#type: String,
    /// Destination, which may carry channel credentials.
    pub address: String,
    /// Headers to send with notifications.
    pub headers: Vec<ChannelHeader>,
    /// OTP reference, when validating an OTP.
    pub otp_ref: String,
    /// OTP entered for the reference.
    pub otp: Secret,
}
impl CreateChannelInput {
    /// Supplies the required display name, type and address.
    pub fn new(
        name: impl Into<String>,
        r#type: impl Into<String>,
        address: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            r#type: r#type.into(),
            address: address.into(),
            headers: Vec::new(),
            otp_ref: String::new(),
            otp: Secret::new(""),
        }
    }
}

/// Fields to change on a notification channel.
#[derive(Clone)]
pub struct UpdateChannelInput {
    /// Channel identifier.
    pub channel_id: String,
    /// Replacement name, if supplied.
    pub name: Option<String>,
    /// Replacement destination, if supplied.
    pub address: Option<String>,
    /// Replacement headers, if supplied. An empty list clears them.
    pub headers: Option<Vec<ChannelHeader>>,
    /// OTP reference, when validating an OTP.
    pub otp_ref: String,
    /// OTP entered for the reference.
    pub otp: Secret,
}
impl UpdateChannelInput {
    /// Identifies the channel to update.
    pub fn new(channel_id: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            name: None,
            address: None,
            headers: None,
            otp_ref: String::new(),
            otp: Secret::new(""),
        }
    }
}
get_input!(
    /// Selects a channel to remove.
    DeleteChannelInput,
    /// Channel identifier.
    channel_id
);

/// Destination for an OTP challenge.
#[derive(Clone)]
pub struct SendChannelOtpInput {
    /// Notification type that accepts an OTP.
    pub r#type: String,
    /// Destination, which may carry channel credentials.
    pub address: String,
    /// Headers to include in the challenge.
    pub headers: Vec<ChannelHeader>,
}
impl SendChannelOtpInput {
    /// Supplies the type and destination of the challenge.
    pub fn new(r#type: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            r#type: r#type.into(),
            address: address.into(),
            headers: Vec::new(),
        }
    }
}

macro_rules! channel_output {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone,Debug,Serialize)]
        pub struct $name {
            /// The acknowledged notification channel.
            #[serde(rename="Channel")]
            pub channel: Channel,
        }
    };
}
channel_output!(
    /// The channel created by the request.
    CreateChannelOutput
);
channel_output!(
    /// The channel acknowledged by the update.
    UpdateChannelOutput
);

/// Acknowledgement that a channel was deleted.
#[derive(Clone, Debug, Serialize)]
pub struct DeleteChannelOutput {}

/// Reference and expiry of a sent OTP challenge.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SendChannelOtpOutput {
    /// Challenge reference used to validate the OTP.
    pub r#ref: String,
    /// Time when the challenge expires.
    #[serde(serialize_with = "serialize_time")]
    pub expires_at: OffsetDateTime,
}

const CODE: Shape<'static> = Shape::Object(&[ShapeField {
    name: "code",
    shape: Shape::Any,
}]);
const CHALLENGE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "ref",
        shape: Shape::Any,
    },
    ShapeField {
        name: "expiredAt",
        shape: Shape::Any,
    },
]);

impl Client {
    /// Creates a notification channel, validating an optional OTP first.
    pub async fn create_channel(
        &self,
        ctx: &RequestContext,
        input: CreateChannelInput,
    ) -> Result<CreateChannelOutput, Failure<CreateChannelOutput>> {
        required(
            CREATE.id,
            [
                ("Name", &input.name),
                ("Type", &input.r#type),
                ("Address", &input.address),
            ],
        )?;
        if !valid_type(&input.r#type, true) {
            return Err(not_sent(invalid_type(CREATE.id, true)));
        }
        check_otp(CREATE.id, &input.otp_ref, &input.otp)?;
        if input.r#type == "Webhook" && !input.otp.expose_secret().is_empty() {
            return Err(not_sent(invalid(
                CREATE.id,
                "Type Webhook takes no OTPRef or OTP",
            )));
        }
        let header = encode_headers(&input.headers).map_err(not_sent)?;
        let mut evidence = self
            .write_evidence(
                ctx,
                &[
                    &input.name,
                    &input.r#type,
                    &input.address,
                    &header,
                    &input.otp_ref,
                    input.otp.expose_secret(),
                ],
            )
            .map_err(not_sent)?;
        let mut privacy = channel_values(
            &input.address,
            &input.headers,
            &header,
            &input.otp_ref,
            &input.otp,
        )
        .map_err(|()| not_sent(privacy_limit(CREATE.id, &evidence)))?;
        let code = self
            .channel_code(
                ctx,
                &CREATE,
                Challenge {
                    address: &input.address,
                    header: &header,
                    reference: &input.otp_ref,
                    otp: &input.otp,
                },
                &mut evidence,
                &mut privacy,
            )
            .await?;
        let body = json!({"name":input.name,"type":input.r#type,"address":input.address,"header":header,"otpCode":code.expose_secret()});
        let reply = self
            .mutate_guarded(
                ctx,
                &CREATE,
                &[],
                Some(&body),
                &mut evidence,
                SendGuard {
                    privacy: Some(&privacy),
                    ..SendGuard::default()
                },
            )
            .await
            .map_err(|failure| screen_failure(failure, &privacy))?;
        let ack: CreateAck = reply
            .decode(&CREATE, &CREATED, &evidence)
            .map_err(maybe_sent)?;
        let mut channel = ack.into_channel();
        if channel.id.is_empty() {
            return Err(maybe_sent(CREATE.error(
                Some(reply.status),
                "",
                "create response had no id",
                &evidence,
            )));
        }
        channel.r#type = input.r#type;
        validate_channel(
            &CREATE,
            reply.status,
            &mut channel,
            &evidence,
            &privacy,
            (&input.otp, &code),
        )
        .map_err(maybe_sent)?;
        Ok(CreateChannelOutput { channel })
    }

    /// Updates supplied fields and resends the rest of the guarded channel snapshot.
    pub async fn update_channel(
        &self,
        ctx: &RequestContext,
        input: UpdateChannelInput,
    ) -> Result<UpdateChannelOutput, Failure<UpdateChannelOutput>> {
        check_path_id(UPDATE.id, "ChannelID", &input.channel_id).map_err(before_write)?;
        if input.name.is_none() && input.address.is_none() && input.headers.is_none() {
            return Err(not_sent(invalid(
                UPDATE.id,
                "requires at least one field to change",
            )));
        }
        check_otp(UPDATE.id, &input.otp_ref, &input.otp)?;
        let replacement_header = input
            .headers
            .as_ref()
            .map(|headers| encode_headers(headers))
            .transpose()
            .map_err(not_sent)?;
        let mut evidence = self
            .write_evidence(
                ctx,
                &[
                    input.name.as_deref().unwrap_or_default(),
                    input.address.as_deref().unwrap_or_default(),
                    replacement_header.as_deref().unwrap_or_default(),
                    &input.channel_id,
                    &input.otp_ref,
                    input.otp.expose_secret(),
                ],
            )
            .map_err(not_sent)?;
        let mut privacy = channel_values(
            input.address.as_deref().unwrap_or_default(),
            input.headers.as_deref().unwrap_or_default(),
            replacement_header.as_deref().unwrap_or_default(),
            &input.otp_ref,
            &input.otp,
        )
        .map_err(|()| not_sent(privacy_limit(UPDATE.id, &evidence)))?;
        let mut channel = self
            .find_channel(
                ctx,
                &input.channel_id,
                &mut evidence,
                Some((UPDATE.id, &privacy)),
            )
            .await
            .map_err(|error| not_sent(screen_error(error, &privacy)))?;
        let existing = channel_values(
            &channel.address,
            &channel.headers,
            &channel.raw_header,
            "",
            &Secret::new(""),
        )
        .map_err(|()| not_sent(privacy_limit(UPDATE.id, &evidence)))?;
        privacy.absorb(existing.credentials());
        if channel.r#type.is_empty() {
            return Err(not_sent(UPDATE.invalid(200, &evidence)));
        }
        validate_channel(
            &UPDATE,
            200,
            &mut channel,
            &evidence,
            &privacy,
            (&input.otp, &Secret::new("")),
        )
        .map_err(not_sent)?;
        if channel.r#type == "Webhook" && !input.otp.expose_secret().is_empty() {
            return Err(not_sent(invalid(
                UPDATE.id,
                "Webhook channels take no OTPRef or OTP",
            )));
        }
        if let Some(name) = input.name {
            channel.name = name;
        }
        if let Some(address) = input.address {
            channel.address = address;
        }
        if let Some(headers) = input.headers {
            channel.headers = headers;
        }
        if let Some(header) = replacement_header {
            channel.raw_header = header;
        }
        channel.updated_date.clear();
        validate_channel(
            &UPDATE,
            200,
            &mut channel,
            &evidence,
            &privacy,
            (&input.otp, &Secret::new("")),
        )
        .map_err(not_sent)?;
        let code = self
            .channel_code(
                ctx,
                &UPDATE,
                Challenge {
                    address: &channel.address,
                    header: &channel.raw_header,
                    reference: &input.otp_ref,
                    otp: &input.otp,
                },
                &mut evidence,
                &mut privacy,
            )
            .await?;
        let body = json!({"id":channel.id,"name":channel.name,"type":channel.r#type,"address":channel.address,"header":channel.raw_header,"otpCode":code.expose_secret()});
        let update = if code.expose_secret().is_empty() {
            Mutation {
                retry: Retry::Repeatable,
                ..UPDATE
            }
        } else {
            UPDATE
        };
        let reply = self
            .mutate_guarded(
                ctx,
                &update,
                &[],
                Some(&body),
                &mut evidence,
                SendGuard {
                    privacy: Some(&privacy),
                    ..SendGuard::default()
                },
            )
            .await
            .map_err(|failure| screen_failure(failure, &privacy))?;
        validate_channel(
            &UPDATE,
            reply.status,
            &mut channel,
            &evidence,
            &privacy,
            (&input.otp, &code),
        )
        .map_err(maybe_sent)?;
        Ok(UpdateChannelOutput { channel })
    }

    /// Deletes the selected notification channel.
    pub async fn delete_channel(
        &self,
        ctx: &RequestContext,
        input: DeleteChannelInput,
    ) -> Result<DeleteChannelOutput, Failure<DeleteChannelOutput>> {
        check_path_id(DELETE.id, "ChannelID", &input.channel_id).map_err(before_write)?;
        let mut evidence = self
            .write_evidence(ctx, &[&input.channel_id])
            .map_err(not_sent)?;
        self.mutate(ctx, &DELETE, &[&input.channel_id], None, &mut evidence)
            .await
            .map_err(|failure: Failure<DeleteChannelOutput>| {
                let (error, _, dispatch) = failure.into_parts();
                let matched = error.api_error().is_some_and(|api| {
                    let text = api.message().to_ascii_lowercase();
                    api.status() == Some(400)
                        && text.contains("notification with id")
                        && text.contains(&input.channel_id.to_ascii_lowercase())
                });
                if matched {
                    Failure::new(
                        Error::new(ErrorClass::NotFound).with_cause(error),
                        None,
                        dispatch,
                    )
                } else {
                    Failure::new(error, None, dispatch)
                }
            })?;
        Ok(DeleteChannelOutput {})
    }

    /// Sends one OTP challenge to the selected destination.
    pub async fn send_channel_otp(
        &self,
        ctx: &RequestContext,
        input: SendChannelOtpInput,
    ) -> Result<SendChannelOtpOutput, Failure<SendChannelOtpOutput>> {
        required(
            SEND.id,
            [("Type", &input.r#type), ("Address", &input.address)],
        )?;
        if !valid_type(&input.r#type, false) {
            return Err(not_sent(invalid_type(SEND.id, false)));
        }
        let header = encode_headers(&input.headers).map_err(not_sent)?;
        let mut evidence = self
            .write_evidence(ctx, &[&input.r#type, &input.address, &header])
            .map_err(not_sent)?;
        let privacy = channel_values(
            &input.address,
            &input.headers,
            &header,
            "",
            &Secret::new(""),
        )
        .map_err(|()| not_sent(privacy_limit(SEND.id, &evidence)))?;
        let body = json!({"type":input.r#type,"address":input.address,"header":header});
        let reply = self
            .mutate_guarded(
                ctx,
                &SEND,
                &[],
                Some(&body),
                &mut evidence,
                SendGuard {
                    privacy: Some(&privacy),
                    ..SendGuard::default()
                },
            )
            .await
            .map_err(|failure| screen_failure(failure, &privacy))?;
        #[derive(Deserialize)]
        struct ChallengeReply {
            #[serde(rename = "ref", default)]
            reference: Option<String>,
            #[serde(rename = "expiredAt", default)]
            expired: Option<Field<f64>>,
        }
        let ChallengeReply { reference, expired } = reply
            .decode(&SEND, &CHALLENGE, &evidence)
            .map_err(maybe_sent)?;
        let reference = reference.filter(|value| !value.is_empty()).ok_or_else(|| {
            maybe_sent(SEND.error(
                Some(reply.status),
                "",
                "send otp response had no ref",
                &evidence,
            ))
        })?;
        let millis = expired.map_or(0.0, |field| field.0);
        let time = OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis as i64) * 1_000_000)
            .map_err(|_| maybe_sent(SEND.invalid(reply.status, &evidence)))?;
        if evidence.id_reflects(&reference)
            || privacy.id_reflects(&reference)
            || !safe_time(time, &evidence)
            || !safe_time(time, &privacy)
        {
            return Err(maybe_sent(SEND.invalid(reply.status, &evidence)));
        }
        Ok(SendChannelOtpOutput {
            r#ref: reference,
            expires_at: time,
        })
    }

    async fn channel_code<T>(
        &self,
        ctx: &RequestContext,
        owner: &Mutation,
        challenge: Challenge<'_>,
        evidence: &mut Evidence,
        privacy: &mut Evidence,
    ) -> Result<Secret, Failure<T>> {
        if challenge.otp.expose_secret().is_empty() {
            return Ok(Secret::new(""));
        }
        let mutation = Mutation {
            retry: Retry::NonIdempotent,
            ..Mutation::standard(owner.id, VALIDATE_ROUTE, &Method::POST)
        };
        let body = json!({"otp":challenge.otp.expose_secret(),"address":challenge.address,"ref":challenge.reference,"header":challenge.header});
        let result = self
            .mutate_guarded::<T>(
                ctx,
                &mutation,
                &[],
                Some(&body),
                evidence,
                SendGuard {
                    privacy: Some(privacy),
                    ..SendGuard::default()
                },
            )
            .await
            .map_err(|failure| before_write(screen_failure(failure, privacy)))?;
        #[derive(Deserialize)]
        struct Validation {
            code: Option<String>,
        }
        let validation: Validation = result
            .decode(&mutation, &CODE, evidence)
            .map_err(not_sent)?;
        let code = validation
            .code
            .filter(|code| !code.is_empty())
            .ok_or_else(|| {
                not_sent(Error::new(ErrorClass::Api).with_outer_detail(WriteError::OtpRejected))
            })?;
        if evidence.text_reflects(&code) {
            return Err(not_sent(mutation.invalid(result.status, evidence)));
        }
        privacy.add_redaction(&Secret::new(&code));
        Ok(Secret::new(code))
    }
}

struct Challenge<'a> {
    address: &'a str,
    header: &'a str,
    reference: &'a str,
    otp: &'a Secret,
}
fn required<T, const N: usize>(op: &str, fields: [(&str, &String); N]) -> Result<(), Failure<T>> {
    if let Some((field, _)) = fields.into_iter().find(|(_, value)| value.is_empty()) {
        return Err(before_write(requires_failure(op, field)));
    }
    Ok(())
}
fn invalid(op: &str, message: &str) -> Error {
    input_error(format!("vngcloud: invalid input: {op}: {message}"))
}
fn invalid_type(op: &str, webhook: bool) -> Error {
    invalid(
        op,
        if webhook {
            "Type must be Email, Slack, SMS, Telegram, or Webhook"
        } else {
            "Type must be Email, Slack, SMS, or Telegram"
        },
    )
}
fn valid_type(value: &str, webhook: bool) -> bool {
    matches!(value, "Email" | "Slack" | "SMS" | "Telegram") || webhook && value == "Webhook"
}
fn check_otp<T>(op: &str, reference: &str, otp: &Secret) -> Result<(), Failure<T>> {
    match (reference.is_empty(), otp.expose_secret().is_empty()) {
        (true, false) => Err(not_sent(invalid(op, "OTPRef is required when OTP is set"))),
        (false, true) => Err(not_sent(invalid(op, "OTP is required when OTPRef is set"))),
        _ => Ok(()),
    }
}
fn encode_headers(headers: &[ChannelHeader]) -> Result<String, Error> {
    if headers.is_empty() {
        return Ok(String::new());
    }
    #[derive(Serialize)]
    struct Header<'a> {
        key: &'a str,
        value: &'a str,
    }
    let wire: Vec<_> = headers
        .iter()
        .map(|header| Header {
            key: &header.key,
            value: &header.value,
        })
        .collect();
    serde_json::to_string(&wire)
        .map(|text| {
            text.replace('<', "\\u003c")
                .replace('>', "\\u003e")
                .replace('&', "\\u0026")
                .replace('\u{2028}', "\\u2028")
                .replace('\u{2029}', "\\u2029")
        })
        .map_err(|_| input_error("monitor channel headers could not be encoded".into()))
}
fn screen_error(error: Error, privacy: &Evidence) -> Error {
    super::mutation::screen_details(error, privacy)
}
fn screen_failure<T>(failure: Failure<T>, privacy: &Evidence) -> Failure<T> {
    let (error, partial, dispatch) = failure.into_parts();
    Failure::new(screen_error(error, privacy), partial, dispatch)
}
