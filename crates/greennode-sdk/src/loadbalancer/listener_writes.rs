use std::net::Ipv4Addr;

use http::Method;
use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::wire::{
    product::options::PathIds,
    strict_json::{Shape, ShapeField},
};
use crate::{Error, Failure, RequestContext, WriteDispatch};

use super::child_writes::{
    BOUND, ChildTarget, ChildWriter, INTERVAL, WriteResult, encode_body, lb_busy, not_sent,
};
use super::listener_output::ListenerProjection;
use super::wire::{CompleteNotFound, DecodeError, Read, invalid_input, screen_input, validate_ids};
use super::{Client, Listener, ListenerInsertHeader, guard};
use guard::shape;

const CREATE: &str = "loadbalancer.CreateListener";
const UPDATE: &str = "loadbalancer.UpdateListener";
const LISTENERS: Read = Read {
    id: "loadbalancer.ListListeners",
    route: "v2/{}/loadBalancers/{}/listeners",
};
const GET_LISTENER: Read = Read {
    id: "loadbalancer.GetListener",
    route: "v2/{}/loadBalancers/{}/listeners/{}",
};
const HEADER: Shape<'_> = shape!("headerName", "headerValue";);
const LISTENER: Shape<'_> = shape!("uuid", "name", "description", "protocol", "protocolPort",
    "connectionLimit", "defaultPoolId", "defaultPoolName", "timeoutClient", "timeoutMember",
    "timeoutConnection", "allowedCidrs", "certificateAuthorities", "displayStatus", "createdAt",
    "updatedAt", "defaultCertificateAuthority", "clientCertificateAuthentication", "progressStatus";
    "insertHeaders" => Shape::Array(&HEADER));

/// Creates a regional load balancer listener.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CreateListenerInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Listener name.
    pub name: String,
    /// Listener protocol.
    pub protocol: String,
    /// Listener port.
    pub port: i64,
    /// Default backend pool identifier.
    pub default_pool_id: String,
    /// Client connection timeout in seconds.
    pub timeout_client: i64,
    /// Backend member timeout in seconds.
    pub timeout_member: i64,
    /// Backend connection timeout in seconds.
    pub timeout_connection: i64,
    /// IPv4 CIDR prefixes allowed to reach the listener.
    pub allowed_cidrs: Vec<String>,
    /// Additional HTTPS certificate identifiers.
    pub certificate_ids: Vec<String>,
    /// Default HTTPS certificate identifier.
    pub default_certificate_id: String,
    /// Certificate identifier for client authentication.
    pub client_certificate_id: String,
    /// Headers to insert before forwarding a request.
    pub insert_headers: Vec<ListenerInsertHeader>,
    /// Returns after the provider accepts the write.
    #[serde(skip)]
    pub no_wait: bool,
}

/// Holds the listener returned by the create or its confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateListenerOutput {
    /// The observed or submitted listener.
    pub listener: Listener,
    projection: Option<ListenerProjection>,
}

impl Serialize for CreateListenerOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut result = serializer.serialize_struct("CreateListenerOutput", 1)?;
        if let Some(projection) = &self.projection {
            result.serialize_field("Listener", projection)?;
        } else {
            result.serialize_field("Listener", &self.listener)?;
        }
        result.end()
    }
}

/// Updates a regional load balancer listener.
#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateListenerInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Target listener identifier.
    #[serde(skip)]
    pub listener_id: String,
    /// Default backend pool identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_pool_id: Option<String>,
    /// Client connection timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_client: Option<i64>,
    /// Backend member timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_member: Option<i64>,
    /// Backend connection timeout in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_connection: Option<i64>,
    /// IPv4 CIDR prefixes allowed to reach the listener.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_cidrs: Option<Vec<String>>,
    /// Additional HTTPS certificate identifiers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_ids: Option<Vec<String>>,
    /// Default HTTPS certificate identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_certificate_id: Option<String>,
    /// Certificate identifier for client authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_certificate_id: Option<String>,
    /// Headers to insert before forwarding a request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_headers: Option<Vec<ListenerInsertHeader>>,
    /// Returns after the provider accepts the write.
    #[serde(skip)]
    pub no_wait: bool,
}

/// Holds the listener returned by the update or its confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateListenerOutput {
    /// The observed or submitted listener.
    pub listener: Listener,
    projection: Option<ListenerProjection>,
}

impl Serialize for UpdateListenerOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut result = serializer.serialize_struct("UpdateListenerOutput", 1)?;
        if let Some(projection) = &self.projection {
            result.serialize_field("Listener", projection)?;
        } else {
            result.serialize_field("Listener", &self.listener)?;
        }
        result.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListenerBody<'a> {
    timeout_client: i64,
    timeout_member: i64,
    timeout_connection: i64,
    #[serde(skip_serializing_if = "str::is_empty")]
    default_pool_id: &'a str,
    allowed_cidrs: String,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    certificate_authorities: &'a [String],
    #[serde(skip_serializing_if = "str::is_empty")]
    default_certificate_authority: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    client_certificate: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    insert_headers: Vec<HeaderBody<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HeaderBody<'a> {
    header_name: &'a str,
    header_value: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody<'a> {
    listener_name: &'a str,
    listener_protocol: &'a str,
    listener_protocol_port: i64,
    #[serde(flatten)]
    settings: ListenerBody<'a>,
}

impl ListenerBody<'_> {
    fn submitted(self, mut listener: Listener) -> Listener {
        listener.default_pool_id = self.default_pool_id.into();
        listener.timeout_client = self.timeout_client;
        listener.timeout_member = self.timeout_member;
        listener.timeout_connection = self.timeout_connection;
        listener.allowed_cidrs = self.allowed_cidrs;
        listener.certificate_authorities = self.certificate_authorities.to_vec();
        listener.default_certificate_authority = nonempty(self.default_certificate_authority);
        listener.client_certificate_authentication = nonempty(self.client_certificate);
        listener.insert_headers = self
            .insert_headers
            .into_iter()
            .map(|header| ListenerInsertHeader {
                header_name: header.header_name.into(),
                header_value: header.header_value.into(),
            })
            .collect();
        listener
    }

    fn check_certificates(&self, operation: &str, protocol: &str) -> Result<(), Box<Error>> {
        if protocol == "HTTPS" {
            if self.default_certificate_authority.is_empty() {
                return Err(invalid_input(&format!(
                    "{operation}: DefaultCertificateID is required when Protocol is HTTPS"
                )));
            }
        } else if !self.certificate_authorities.is_empty()
            || !self.default_certificate_authority.is_empty()
            || !self.client_certificate.is_empty()
        {
            return Err(invalid_input(&format!(
                "{operation}: CertificateIDs, DefaultCertificateID, and ClientCertificateID are only valid when Protocol is HTTPS, got {protocol:?}"
            )));
        }
        check_certificate_ids(
            operation,
            self.certificate_authorities,
            self.default_certificate_authority,
            self.client_certificate,
        )
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a listener and optionally waits for the listener and load balancer to settle.
    pub async fn create_listener(
        &self,
        ctx: &RequestContext,
        input: CreateListenerInput,
    ) -> WriteResult<CreateListenerOutput> {
        let ids = [("LoadBalancerID", input.load_balancer_id.as_str())];
        let mut write =
            ChildWriter::new(self, ctx, CREATE, &input.load_balancer_id, &ids).map_err(not_sent)?;
        encode_body(&input, &write.evidence).map_err(not_sent)?;
        for (field, missing) in [
            ("Name", input.name.is_empty()),
            ("Protocol", input.protocol.is_empty()),
            ("Port", input.port == 0),
        ] {
            if missing {
                return Err(not_sent(invalid_input(&format!(
                    "{CREATE} requires {field}"
                ))));
            }
        }
        validate_optional_id(CREATE, "DefaultPoolID", &input.default_pool_id).map_err(not_sent)?;
        check_cidrs(CREATE, &input.allowed_cidrs).map_err(not_sent)?;
        let body = CreateBody {
            listener_name: &input.name,
            listener_protocol: &input.protocol,
            listener_protocol_port: input.port,
            settings: ListenerBody {
                timeout_client: default_timeout(input.timeout_client, 50),
                timeout_member: default_timeout(input.timeout_member, 50),
                timeout_connection: default_timeout(input.timeout_connection, 5),
                default_pool_id: &input.default_pool_id,
                allowed_cidrs: input.allowed_cidrs.join(","),
                certificate_authorities: &input.certificate_ids,
                default_certificate_authority: &input.default_certificate_id,
                client_certificate: &input.client_certificate_id,
                insert_headers: header_body(&input.insert_headers),
            },
        };
        body.settings
            .check_certificates(CREATE, &input.protocol)
            .map_err(not_sent)?;
        let bytes = encode_body(&body, &write.evidence).map_err(not_sent)?;
        let _lock = self
            .lock_load_balancer(ctx, &input.load_balancer_id)
            .await
            .map_err(|error| not_sent(Box::new(error)))?;
        let ready = write.ready(ctx, None).await.map_err(not_sent)?;
        screen_input(&write.evidence, [input.protocol.as_str()]).map_err(not_sent)?;
        if ready.lb_type == "Layer 7" && !matches!(input.protocol.as_str(), "HTTP" | "HTTPS") {
            return Err(not_sent(invalid_input(&format!(
                "{CREATE}: a Layer 7 load balancer accepts only HTTP and HTTPS listeners, got {:?}",
                input.protocol
            ))));
        }
        let target = ChildTarget {
            read: &LISTENERS,
            ..listener_target("", &ids)
        };
        let (status, response) = write
            .send(ctx, Method::POST, &target, bytes, None)
            .await
            .map_err(|failure| write.create_failure(failure, "list-listeners"))?;
        let uuid = write.acknowledged_id(status, &response, "list-listeners", input.no_wait)?;
        let fallback = body.settings.submitted(Listener {
            uuid: uuid.clone(),
            name: body.listener_name.to_owned(),
            protocol: body.listener_protocol.to_owned(),
            protocol_port: input.port,
            ..Listener::default()
        });
        let ids = [
            ("LoadBalancerID", input.load_balancer_id.as_str()),
            ("ListenerID", uuid.as_str()),
        ];
        let target = listener_target(&uuid, &ids);
        finish(
            ctx,
            &mut write,
            &target,
            input.no_wait,
            true,
            fallback,
            |listener, projection| CreateListenerOutput {
                listener,
                projection,
            },
        )
        .await
    }

    /// Updates a listener using its current settings for fields left unset.
    pub async fn update_listener(
        &self,
        ctx: &RequestContext,
        input: UpdateListenerInput,
    ) -> WriteResult<UpdateListenerOutput> {
        let ids = [
            ("LoadBalancerID", input.load_balancer_id.as_str()),
            ("ListenerID", input.listener_id.as_str()),
        ];
        let mut write =
            ChildWriter::new(self, ctx, UPDATE, &input.load_balancer_id, &ids).map_err(not_sent)?;
        if encode_body(&input, &write.evidence)
            .map_err(not_sent)?
            .as_ref()
            == b"{}"
        {
            return Err(not_sent(invalid_input(&format!(
                "{UPDATE} requires at least one field to change"
            ))));
        }
        validate_optional_id(
            UPDATE,
            "DefaultPoolID",
            input.default_pool_id.as_deref().unwrap_or_default(),
        )
        .map_err(not_sent)?;
        if let Some(cidrs) = &input.allowed_cidrs {
            check_cidrs(UPDATE, cidrs).map_err(not_sent)?;
        }
        check_certificate_ids(
            UPDATE,
            input.certificate_ids.as_deref().unwrap_or_default(),
            input.default_certificate_id.as_deref().unwrap_or_default(),
            input.client_certificate_id.as_deref().unwrap_or_default(),
        )
        .map_err(not_sent)?;
        let _lock = self
            .lock_load_balancer(ctx, &input.load_balancer_id)
            .await
            .map_err(|error| not_sent(Box::new(error)))?;
        let target = listener_target(&input.listener_id, &ids);
        let ready = write.ready(ctx, Some(&target)).await.map_err(not_sent)?;
        let listener: Listener = GET_LISTENER
            .parse(ready.child.as_deref().unwrap_or("null"), &write.evidence)
            .map_err(not_sent)?;
        let body = merged(&input, &listener);
        if body.allowed_cidrs.is_empty() {
            return Err(not_sent(invalid_input(&format!(
                "{UPDATE}: the listener's own AllowedCIDRs read back empty; set AllowedCIDRs explicitly"
            ))));
        }
        let bytes = encode_body(&body, &write.evidence).map_err(not_sent)?;
        screen_input(&write.evidence, [listener.protocol.as_str()]).map_err(not_sent)?;
        body.check_certificates(UPDATE, &listener.protocol)
            .map_err(not_sent)?;
        write
            .send(ctx, Method::PUT, &target, bytes, Some(&target))
            .await?;
        let fallback = if input.no_wait {
            body.submitted(listener.clone())
        } else {
            listener
        };
        finish(
            ctx,
            &mut write,
            &target,
            input.no_wait,
            false,
            fallback,
            |listener, projection| UpdateListenerOutput {
                listener,
                projection,
            },
        )
        .await
    }
}

fn merged<'a>(input: &'a UpdateListenerInput, listener: &'a Listener) -> ListenerBody<'a> {
    ListenerBody {
        timeout_client: input.timeout_client.unwrap_or(listener.timeout_client),
        timeout_member: input.timeout_member.unwrap_or(listener.timeout_member),
        timeout_connection: input
            .timeout_connection
            .unwrap_or(listener.timeout_connection),
        default_pool_id: input
            .default_pool_id
            .as_deref()
            .unwrap_or(&listener.default_pool_id),
        allowed_cidrs: input
            .allowed_cidrs
            .as_ref()
            .map_or_else(|| listener.allowed_cidrs.clone(), |cidrs| cidrs.join(",")),
        certificate_authorities: input
            .certificate_ids
            .as_deref()
            .unwrap_or(&listener.certificate_authorities),
        default_certificate_authority: input
            .default_certificate_id
            .as_deref()
            .or(listener.default_certificate_authority.as_deref())
            .unwrap_or_default(),
        client_certificate: input
            .client_certificate_id
            .as_deref()
            .or(listener.client_certificate_authentication.as_deref())
            .unwrap_or_default(),
        insert_headers: header_body(
            input
                .insert_headers
                .as_deref()
                .unwrap_or(&listener.insert_headers),
        ),
    }
}

fn validate_optional_id(operation: &str, field: &str, id: &str) -> Result<(), Box<Error>> {
    if id.is_empty() {
        return Ok(());
    }
    validate_ids(operation, &[(field, id)], PathIds::Strict)
}

fn check_certificate_ids(
    operation: &str,
    ids: &[String],
    default: &str,
    client: &str,
) -> Result<(), Box<Error>> {
    for id in ids {
        validate_ids(operation, &[("CertificateIDs", id)], PathIds::Strict)?;
    }
    validate_optional_id(operation, "DefaultCertificateID", default)?;
    validate_optional_id(operation, "ClientCertificateID", client)
}

fn cidr_parts(cidr: &str) -> Option<(u32, u8)> {
    let (address, length) = cidr.split_once('/')?;
    if length.starts_with('+') || (length.starts_with('0') && length.len() > 1) {
        return None;
    }
    let length: u8 = length.parse().ok()?;
    let address: Ipv4Addr = address.parse().ok()?;
    (length <= 32).then_some((u32::from(address), length))
}

fn check_cidrs(operation: &str, cidrs: &[String]) -> Result<(), Box<Error>> {
    if cidrs.is_empty() {
        return Err(invalid_input(&format!(
            "{operation} requires at least one AllowedCIDRs entry"
        )));
    }
    for cidr in cidrs {
        let (address, length) = cidr_parts(cidr).ok_or_else(|| invalid_input(&format!("{operation}: AllowedCIDRs entry must be an IPv4 CIDR prefix such as 10.0.0.0/24, got {cidr:?}")))?;
        let network = address
            & u32::MAX
                .checked_shl(u32::from(32 - length))
                .unwrap_or_default();
        if network != address {
            let address = Ipv4Addr::from(network);
            return Err(invalid_input(&format!(
                "{operation}: AllowedCIDRs entry must have no host bits set; got {cidr:?}, want \"{address}/{length}\""
            )));
        }
    }
    Ok(())
}

fn default_timeout(value: i64, fallback: i64) -> i64 {
    if value == 0 { fallback } else { value }
}

fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn header_body(rows: &[ListenerInsertHeader]) -> Vec<HeaderBody<'_>> {
    rows.iter()
        .map(|header| HeaderBody {
            header_name: &header.header_name,
            header_value: &header.header_value,
        })
        .collect()
}

fn listener_target<'a>(id: &'a str, ids: &'a [(&'static str, &'a str)]) -> ChildTarget<'a> {
    ChildTarget {
        kind: "listener",
        id,
        read: &GET_LISTENER,
        ids,
        shape: &LISTENER,
        guard_field: Some("protocol"),
        validate: |read, row, evidence| read.parse::<Listener>(row, evidence).map(|_| ()),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
async fn finish<O>(
    ctx: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
    no_wait: bool,
    creating: bool,
    mut listener: Listener,
    output: impl FnOnce(Listener, Option<ListenerProjection>) -> O,
) -> WriteResult<O> {
    let result = if no_wait {
        Ok(())
    } else {
        settle(ctx, write, target, creating, &mut listener).await
    };
    let retained = ListenerProjection::retain(&mut listener, &write.evidence)
        .map(|projection| output(listener, projection))
        .map_err(|()| GET_LISTENER.decode_error(&write.evidence, DecodeError::ReflectedCredential));
    match (result, retained) {
        (Ok(()), Ok(output)) => Ok(output),
        (Ok(()), Err(error)) => Err(Failure::new(*error, None, WriteDispatch::MayHaveBeenSent)),
        (Err(error), retained) => {
            let partial = if write.evidence.can_retain_partial() {
                retained.ok()
            } else {
                None
            };
            Err(Failure::new(
                *error,
                partial,
                WriteDispatch::MayHaveBeenSent,
            ))
        }
    }
}

async fn settle(
    ctx: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
    creating: bool,
    last: &mut Listener,
) -> Result<(), Box<Error>> {
    let clock = write.client.session.clock();
    let wait = ctx.child_with_deadline(clock.monotonic_now() + BOUND);
    loop {
        wait.child_with_deadline(clock.monotonic_now() + INTERVAL)
            .wait(clock)
            .await;
        write.confirm_time(ctx, &wait, target)?;
        let row = write.read_data(&wait, &GET_LISTENER, target.ids).await;
        write.confirm_time(ctx, &wait, target)?;
        let row = match row {
            Ok(row) => row,
            Err(error) if creating && error.detail::<CompleteNotFound>().is_some() => continue,
            Err(error) => return Err(write.unconfirmed(target, *error)),
        };
        let listener: Listener = guard::parse(&GET_LISTENER, &row, &LISTENER, &write.evidence)
            .map_err(|error| write.unconfirmed(target, *error))?;
        if listener.uuid != target.id || listener.progress_status.is_empty() {
            let error = GET_LISTENER.decode_error(&write.evidence, DecodeError::InvalidShape);
            return Err(write.unconfirmed(target, *error));
        }
        screen_input(
            &write.evidence,
            [listener.uuid.as_str(), &listener.progress_status],
        )
        .map_err(|error| write.unconfirmed(target, *error))?;
        *last = listener;
        match last.progress_status.as_str() {
            "ERROR" => return Err(write.failed(target.kind, target.id)),
            "CREATED" => {
                let parent = write.parent(&wait).await;
                write.confirm_time(ctx, &wait, target)?;
                let (status, _) = parent.map_err(|error| write.unconfirmed(target, *error))?;
                if status == "ERROR" {
                    return Err(write.failed("load balancer", write.lb_id));
                }
                if !lb_busy(&status) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}
