use std::time::Duration;

use http::{Method, StatusCode};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Value, json};

use super::wire::{DecodeError, Field, WireField, go_json, response_error};
use super::write::{
    Settlement, Write, guard_operation, not_sent, rejected_response, write_operation,
};
use super::{Client, LoadBalancer, invalid_input, path_ids};
use crate::wire::product::{evidence::Evidence, operation::Operation, options::Options};
use crate::wire::strict_json::{self, Shape, ShapeField};
use crate::{Error, Failure, RequestContext};

const COLLECTION: &str = "v1/global-load-balancers";
const RESOURCE: &str = "v1/global-load-balancers/{}";
const CREATE: &str = "globalloadbalancer.CreateLoadBalancer";
const DELETE: &str = "globalloadbalancer.DeleteLoadBalancer";
const LAYER_FOUR: &str = "Layer 4";
const CREATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const DELETE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Selects the name and package for a credit-funded Layer 4 load balancer.
#[derive(Clone, Debug)]
pub struct CreateLoadBalancerInput {
    /// Name for the new load balancer.
    pub name: String,
    /// Package identifier.
    pub package_id: String,
    /// Optional description. Omitted from the request when empty.
    pub description: String,
}

impl CreateLoadBalancerInput {
    /// Creates input for a named load balancer with the selected package.
    pub fn new(name: impl Into<String>, package_id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            package_id: package_id.into(),
            description: String::new(),
        }
    }
}

/// The confirmed load balancer or its last safely retained identity and state.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateLoadBalancerOutput {
    /// Load balancer details established by confirmation reads.
    pub load_balancer: LoadBalancer,
    redact_user_id: bool,
}

impl Serialize for CreateLoadBalancerOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut output = serializer.serialize_struct("CreateLoadBalancerOutput", 1)?;
        output.serialize_field("LoadBalancer", &ConfirmedLoadBalancer(self))?;
        output.end()
    }
}

struct ConfirmedLoadBalancer<'a>(&'a CreateLoadBalancerOutput);

impl Serialize for ConfirmedLoadBalancer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let lb = &self.0.load_balancer;
        if !self.0.redact_user_id {
            return lb.serialize(serializer);
        }
        let mut output = serializer.serialize_struct("LoadBalancer", 12)?;
        output.serialize_field("ID", &lb.id)?;
        output.serialize_field("Name", &lb.name)?;
        output.serialize_field("Description", &lb.description)?;
        output.serialize_field("Status", &lb.status)?;
        output.serialize_field("Package", &lb.package)?;
        output.serialize_field("Type", &lb.r#type)?;
        output.serialize_field("UserID", "<redacted>")?;
        output.serialize_field("CreatedAt", &lb.created_at)?;
        output.serialize_field("UpdatedAt", &lb.updated_at)?;
        output.serialize_field("DeletedAt", &lb.deleted_at)?;
        output.serialize_field("VIPs", &lb.vips)?;
        output.serialize_field("Domains", &lb.domains)?;
        output.end()
    }
}

/// Selects the load balancer to delete.
#[derive(Clone, Debug)]
pub struct DeleteLoadBalancerInput {
    /// Target load balancer identifier.
    pub load_balancer_id: String,
}

impl DeleteLoadBalancerInput {
    /// Selects the target load balancer by its identifier.
    pub fn new(load_balancer_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
        }
    }
}

/// Identifies the target and whether its absence was confirmed.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteLoadBalancerOutput {
    /// Target load balancer identifier.
    pub load_balancer_id: String,
    /// Whether the service confirmed absence after deletion.
    pub deleted: bool,
    redact_deleted: bool,
}

impl Serialize for DeleteLoadBalancerOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut output = serializer.serialize_struct("DeleteLoadBalancerOutput", 2)?;
        output.serialize_field("LoadBalancerID", &self.load_balancer_id)?;
        if self.redact_deleted {
            output.serialize_field("Deleted", "<redacted>")?;
        } else {
            output.serialize_field("Deleted", &self.deleted)?;
        }
        output.end()
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Submits one credit-funded create and waits for the exact load balancer to become active.
    pub async fn create_load_balancer(
        &self,
        ctx: &RequestContext,
        input: CreateLoadBalancerInput,
    ) -> Result<CreateLoadBalancerOutput, Failure<CreateLoadBalancerOutput>> {
        for (field, value) in [("Name", &input.name), ("PackageID", &input.package_id)] {
            if value.is_empty() {
                return Err(not_sent(invalid_input(format!(
                    "{CREATE} requires {field}"
                ))));
            }
        }
        let operation = write_operation(CREATE, COLLECTION, Method::POST);
        let mut write = self.start_write(CREATE)?;
        let mut body = json!({"name": input.name, "package": input.package_id, "paymentFlow": "automated", "type": LAYER_FOUR});
        if !input.description.is_empty() {
            body["description"] = Value::String(input.description);
        }
        let reply = write
            .request(ctx, &operation, &[], Some(&body))
            .await
            .map_err(|failure| write.rejection(failure, None, "create request unconfirmed"))?;
        let id = if operation.success.contains(&reply.status) {
            let accepted: Accepted =
                decode(&operation, &reply.body, &ACCEPTED_SHAPE).map_err(|_| {
                    write.finish(Settlement::Unknown("create request unconfirmed"), None)
                })?;
            accepted.load_balancer.0.id
        } else if reply.status.is_client_error() {
            return Err(Failure::new(
                rejected_response(&operation, &reply, Some(&body), None),
                None,
                crate::WriteDispatch::MayHaveBeenSent,
            ));
        } else {
            return Err(write.finish(Settlement::Unknown("create request unconfirmed"), None));
        };
        if !Options::DEFAULT.path_ids.allows(&id) || write.evidence.text_reflects(&id) {
            return Err(write.finish(
                Settlement::Unknown("create response lacks a valid ID"),
                None,
            ));
        }
        let wait = write.wait_context(ctx, CREATE_TIMEOUT);
        let mut observed = LoadBalancer {
            id: id.clone(),
            ..Default::default()
        };
        let reason = loop {
            if write.interrupted(&wait) {
                break Settlement::Unknown("create confirmation interrupted");
            }
            let Ok(detail) = write.load_balancer_detail(&wait, &id).await else {
                break Settlement::Unknown("create confirmation interrupted");
            };
            if write.interrupted(&wait) {
                break Settlement::Unknown("create confirmation interrupted");
            }
            if detail.id.0 != id
                || detail.name.0 != input.name
                || detail.package.0 != input.package_id
                || detail.kind.0 != LAYER_FOUR
            {
                break Settlement::Unknown("create detail does not match");
            }
            match detail.status.0.to_ascii_uppercase().as_str() {
                "ACTIVE" => {
                    let output = create_output(detail.into(), &write.evidence);
                    if write.safe_output(&output) {
                        return Ok(output);
                    }
                    break Settlement::Unknown("output withheld");
                }
                "ERROR" => break Settlement::Failed,
                "CREATING" | "PENDING" | "PROVISIONING" | "UPDATING" => observed = detail.into(),
                _ => break Settlement::Unknown("create status unrecognized"),
            }
            write.pause(&wait).await;
        };
        Err(write.finish(reason, Some(create_output(observed, &write.evidence))))
    }

    /// Deletes the exact Layer 4 load balancer after preflight and confirms its absence.
    pub async fn delete_load_balancer(
        &self,
        ctx: &RequestContext,
        input: DeleteLoadBalancerInput,
    ) -> Result<DeleteLoadBalancerOutput, Failure<DeleteLoadBalancerOutput>> {
        let operation = write_operation(DELETE, RESOURCE, Method::DELETE);
        let id = &input.load_balancer_id;
        let path = path_ids(&operation, &[("LoadBalancerID", id)])
            .map_err(|message| not_sent(invalid_input(message)))?;
        let mut write = self.start_write(DELETE)?;
        let detail = write
            .load_balancer_detail(ctx, id)
            .await
            .map_err(|error| not_sent(*error))?;
        if detail.id.0 != *id
            || detail.kind.0 != LAYER_FOUR
            || !matches!(
                detail.status.0.to_ascii_uppercase().as_str(),
                "ACTIVE" | "ERROR"
            )
        {
            return Err(not_sent(invalid_input(
                "preflight resource is not eligible for deletion".into(),
            )));
        }
        let output = |write: &Write<'_>, deleted| delete_output(id, deleted, &write.evidence);
        let reply = match write.request(ctx, &operation, &path, None).await {
            Ok(reply) => reply,
            Err(failure) => {
                return Err(write.rejection(
                    failure,
                    Some(output(&write, false)),
                    "delete request unconfirmed",
                ));
            }
        };
        if reply.status.is_client_error() {
            return Err(Failure::new(
                rejected_response(&operation, &reply, None, None),
                None,
                crate::WriteDispatch::MayHaveBeenSent,
            ));
        }
        if !operation.success.contains(&reply.status) {
            return Err(write.finish(
                Settlement::Unknown("delete request unconfirmed"),
                Some(output(&write, false)),
            ));
        }
        let wait = write.wait_context(ctx, DELETE_TIMEOUT);
        let reason = loop {
            if write.interrupted(&wait) {
                break Settlement::Unknown("delete confirmation interrupted");
            }
            let current = write.load_balancer_detail(&wait, id).await;
            if write.interrupted(&wait) {
                break Settlement::Unknown("delete confirmation interrupted");
            }
            match current {
                Err(error) if error.detail::<ConfirmedAbsence>().is_some() => {
                    let output = output(&write, true);
                    if write.safe_output(&output) {
                        return Ok(output);
                    }
                    break Settlement::Unknown("output withheld");
                }
                Ok(detail) if detail.id.0 != *id => {
                    break Settlement::Unknown("delete detail ID mismatch");
                }
                Ok(_) => write.pause(&wait).await,
                Err(_) => break Settlement::Unknown("delete confirmation interrupted"),
            }
        };
        Err(write.finish(reason, Some(output(&write, false))))
    }
}

struct ConfirmedAbsence;

#[derive(Default, Deserialize)]
#[serde(default)]
struct Detail {
    id: Field<String>,
    name: Field<String>,
    package: Field<String>,
    status: Field<String>,
    #[serde(rename = "type")]
    kind: Field<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Accepted {
    #[serde(rename = "globalLoadBalancer")]
    load_balancer: Field<LoadBalancer>,
}

const ACCEPTED_SHAPE: Shape<'static> = Shape::Object(&[ShapeField {
    name: "globalLoadBalancer",
    shape: LoadBalancer::SHAPE,
}]);
const DETAIL_SHAPE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "id",
        shape: Shape::Any,
    },
    ShapeField {
        name: "name",
        shape: Shape::Any,
    },
    ShapeField {
        name: "package",
        shape: Shape::Any,
    },
    ShapeField {
        name: "status",
        shape: Shape::Any,
    },
    ShapeField {
        name: "type",
        shape: Shape::Any,
    },
]);

impl Write<'_> {
    async fn load_balancer_detail(
        &mut self,
        ctx: &RequestContext,
        id: &str,
    ) -> Result<Detail, Box<Error>> {
        let operation = guard_operation("globalloadbalancer.WriteDetail", RESOURCE);
        let reply = self
            .request(ctx, &operation, &[id], None)
            .await
            .map_err(|failure| {
                let error = failure.into_parts().0;
                if error.is_body_too_large()
                    && error.detail::<StatusCode>() == Some(&StatusCode::NOT_FOUND)
                {
                    Box::new(
                        super::write::status_rejection(
                            operation.id,
                            StatusCode::NOT_FOUND,
                            self.evidence.credentials(),
                        )
                        .with_detail(ConfirmedAbsence),
                    )
                } else {
                    Box::new(error)
                }
            })?;
        match reply.status {
            StatusCode::OK => decode(&operation, &reply.body, &DETAIL_SHAPE),
            status => {
                let error = rejected_response(&operation, &reply, None, Some("response withheld"));
                if status == StatusCode::NOT_FOUND {
                    Err(Box::new(error.with_detail(ConfirmedAbsence)))
                } else {
                    Err(Box::new(error))
                }
            }
        }
    }
}

pub(super) fn decode<T: serde::de::DeserializeOwned + Default>(
    operation: &Operation,
    bytes: &[u8],
    shape: &Shape<'_>,
) -> Result<T, Box<Error>> {
    let body = go_json(bytes);
    let raw = serde_json::from_str::<Option<&serde_json::value::RawValue>>(&body)
        .map_err(|_| Box::new(response_error(operation, DecodeError::MalformedJson)))?;
    strict_json::validate_shape(body.as_bytes(), shape, operation.cap)
        .map_err(|_| Box::new(response_error(operation, DecodeError::InvalidShape)))?;
    raw.map(|raw| {
        serde_json::from_str(raw.get())
            .map_err(|_| Box::new(response_error(operation, DecodeError::InvalidShape)))
    })
    .transpose()
    .map(Option::unwrap_or_default)
}

impl From<Detail> for LoadBalancer {
    fn from(value: Detail) -> Self {
        Self {
            id: value.id.0,
            name: value.name.0,
            package: value.package.0,
            status: value.status.0,
            r#type: value.kind.0,
            ..Self::default()
        }
    }
}

fn create_output(mut load_balancer: LoadBalancer, evidence: &Evidence) -> CreateLoadBalancerOutput {
    for text in [
        &mut load_balancer.id,
        &mut load_balancer.name,
        &mut load_balancer.description,
        &mut load_balancer.status,
        &mut load_balancer.package,
        &mut load_balancer.r#type,
        &mut load_balancer.created_at,
        &mut load_balancer.updated_at,
        &mut load_balancer.deleted_at,
    ] {
        text.redact(evidence);
    }
    CreateLoadBalancerOutput {
        load_balancer,
        redact_user_id: evidence.reflected_in(&json!(0)),
    }
}

fn delete_output(id: &str, deleted: bool, evidence: &Evidence) -> DeleteLoadBalancerOutput {
    let mut load_balancer_id = id.to_owned();
    load_balancer_id.redact(evidence);
    DeleteLoadBalancerOutput {
        load_balancer_id,
        deleted,
        redact_deleted: evidence.reflected_in(&json!(deleted)),
    }
}
