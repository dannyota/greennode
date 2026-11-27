use std::collections::HashSet;

use http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::model::wire::{Field, go_json};
use crate::wire::{
    product::{evidence::Evidence, redirect::Retry},
    strict_json,
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch, error::SafeText};

use super::mutation::{Mutation, Settlement, not_sent, safe_server, settlement_error, status};
use super::quotes::{add_user_data, encoded_user_data, encrypted, path_id, required, server_info};
use super::wire::{api, invalid_input};
use super::{Client, CreateServerInput, Server};

const CREATE: Mutation = Mutation {
    id: "compute.CreateServer",
    template: "v2/{}/servers",
    method: Method::POST,
    retry: Retry::Once,
    overflow_overrides_status: true,
    success: &[StatusCode::ACCEPTED],
};
const DELETE: Mutation = Mutation {
    id: "compute.DeleteServer",
    template: "v2/{}/servers/{}",
    method: Method::DELETE,
    retry: Retry::Repeatable,
    overflow_overrides_status: false,
    success: &[StatusCode::ACCEPTED],
};
const DISK_ADVICE: &str = "a 400 can mean a disk type is not offered in ZoneID; list the zone's types with volume.ListVolumeTypes and ZoneID";
const USER_DATA_WITHHELD: &str = "server message withheld: it may quote user data";
const CREATE_UNKNOWN: &str = "create response had no id; the server may exist, list servers and match the name exactly before creating it again";
const CREATE_AMBIGUOUS: &str = "create may have already reached the server; list servers and match the name exactly before creating it again";

/// Result of ordering a server.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateServerOutput {
    /// The last confirmed server data, or the ID and requested name.
    pub server: Server,
    /// Quoted estimated monthly price in VND.
    pub monthly_price: f64,
}

crate::wire::product::output::public_fields!(CreateServerOutput {
    server,
    monthly_price
});

/// Options for deleting a server.
#[derive(Clone, Debug, Default)]
pub struct DeleteServerInput {
    /// Server to delete.
    pub server_id: String,
    /// Delete attached data volumes as well as the boot volume.
    pub delete_volumes: bool,
    /// Return after the acknowledgement without waiting for deletion.
    pub no_wait: bool,
}

/// Volumes requested for deletion or retained when a server is deleted.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DeleteServerOutput {
    /// Volumes requested for deletion if `delete_volumes` was selected.
    #[serde(rename = "DeletedVolumeIDs")]
    pub deleted_volume_ids: Vec<String>,
    /// Data volumes whose deletion has not been confirmed.
    #[serde(rename = "KeptVolumeIDs")]
    pub kept_volume_ids: Vec<String>,
}

crate::wire::product::output::public_fields!(DeleteServerOutput {
    deleted_volume_ids,
    kept_volume_ids
});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Checks inventory and pricing, sends one create, and confirms the result unless declined.
    pub async fn create_server(
        &self,
        ctx: &RequestContext,
        input: CreateServerInput,
    ) -> Result<CreateServerOutput, Failure<CreateServerOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            required(
                CREATE.id,
                &[
                    ("Name", !input.name.is_empty()),
                    ("ZoneID", !input.zone_id.is_empty()),
                    ("FlavorID", !input.flavor_id.is_empty()),
                    ("ImageID", !input.image_id.is_empty()),
                    ("VPCID", !input.vpc_id.is_empty()),
                    ("SubnetID", !input.subnet_id.is_empty()),
                    ("SecurityGroupIDs", !input.security_group_ids.is_empty()),
                    ("RootDiskSize", input.root_disk_size != 0),
                    ("RootDiskTypeID", !input.root_disk_type_id.is_empty()),
                ],
            )
            .map_err(|error| not_sent(*error))?;
            if !input.max_price.is_finite() || input.max_price < 0.0 {
                return Err(not_sent(invalid_input(
                    "compute.CreateServer: MaxPrice must be finite and non-negative",
                )));
            }
            let info = server_info(CREATE.id, &input).map_err(|error| not_sent(*error))?;
            let body = create_body(&input).map_err(|error| not_sent(*error))?;
            self.guard_write(
                ctx,
                &json!([
                    body,
                    input.max_price,
                    input.user_data.as_ref().map(|data| data.expose_secret())
                ]),
                &mut evidence,
            )
            .map_err(|error| not_sent(*error))?;
            add_user_data(&input, &mut evidence);
            self.refuse_server_name(ctx, &input.name, &mut evidence)
                .await?;
            let quote = self
                .create_quote(ctx, &input, info, &mut evidence)
                .await
                .map_err(|error| not_sent(*error))?;
            check_price(CREATE.id, quote.optimum_price, input.max_price, &evidence)
                .map_err(|error| not_sent(*error))?;
            let sensitive = input
                .user_data
                .as_ref()
                .is_some_and(|data| !data.expose_secret().is_empty());
            let body = self
                .send_mutation(ctx, &CREATE, &[], Some(&body), sensitive, &mut evidence)
                .await
                .map_err(|failed| create_error(failed, sensitive, &evidence))?;
            let id = create_id(&body, &evidence)
                .map_err(|error| Failure::new(*error, None, WriteDispatch::MayHaveBeenSent))?;
            let mut server = Server {
                uuid: id.clone(),
                name: input.name,
                ..Server::default()
            };
            let settlement = if input.no_wait {
                Ok(())
            } else {
                self.settle_server(ctx, &id, Settlement::Created, &mut server, &mut evidence)
                    .await
            };
            let output = safe_server(server, &evidence)
                .filter(|_| {
                    !evidence.output_reflects(&json!(quote.optimum_price))
                        && !evidence.reflects_fixed_two(quote.optimum_price)
                })
                .map(|server| CreateServerOutput {
                    server,
                    monthly_price: quote.optimum_price,
                });
            match (settlement, output) {
                (Ok(()), Some(output)) => Ok(output),
                (Ok(()), None) => Err(Failure::new(
                    invalid_response(&evidence),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                )),
                (Err(error), output) => Err(Failure::new(
                    settlement_error(
                        CREATE.id,
                        *error,
                        "inspect get-server or list-servers before creating again",
                        &evidence,
                    ),
                    output,
                    WriteDispatch::MayHaveBeenSent,
                )),
            }
        })
    }

    /// Deletes a server and reports attached volumes, with optional reconciliation after confirmation.
    pub async fn delete_server(
        &self,
        ctx: &RequestContext,
        input: DeleteServerInput,
    ) -> Result<DeleteServerOutput, Failure<DeleteServerOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = &input.server_id;
            path_id(DELETE.id, "ServerID", id).map_err(|error| not_sent(*error))?;
            let mut evidence = Evidence::new();
            let body = json!({"deleteAllVolume": input.delete_volumes});
            self.guard_write(ctx, &json!([id, body]), &mut evidence)
                .map_err(|error| not_sent(*error))?;
            let mut server = self
                .server_guard::<()>(ctx, id, &mut evidence)
                .await
                .map_err(|error| not_sent(error.into_parts().0))?
                .into_checked(&evidence)
                .map_err(|error| not_sent(*error))?;
            if serde_json::to_value(&server).map_or(true, |value| evidence.output_reflects(&value))
            {
                return Err(not_sent(delete_inventory_error(&evidence)));
            }
            let volumes = self
                .volume
                .list_volumes_by_server_with_evidence(ctx, id, &mut evidence)
                .await
                .map_err(|error| not_sent(*error))?;
            let boot = &server.boot_volume_id;
            let mut ids = Vec::with_capacity(volumes.items.len());
            let mut unique = HashSet::new();
            if evidence.text_reflects(boot) {
                return Err(not_sent(delete_inventory_error(&evidence)));
            }
            for volume in volumes.items {
                let id = volume.uuid;
                if id.is_empty()
                    || !id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                    || evidence.text_reflects(&id)
                    || !unique.insert(id.clone())
                {
                    return Err(not_sent(delete_inventory_error(&evidence)));
                }
                if input.delete_volumes || id != *boot {
                    ids.push(id);
                }
            }
            self.send_mutation(ctx, &DELETE, &[id], Some(&body), false, &mut evidence)
                .await?;
            if !input.no_wait {
                if let Err(error) = self
                    .settle_server(ctx, id, Settlement::Deleted, &mut server, &mut evidence)
                    .await
                {
                    let partial = delete_output(ids, input.delete_volumes, &evidence);
                    return Err(Failure::new(
                        settlement_error(
                            DELETE.id,
                            *error,
                            "call GetServer to check its status",
                            &evidence,
                        ),
                        partial,
                        WriteDispatch::MayHaveBeenSent,
                    ));
                }
                if !input.delete_volumes {
                    let mut kept = Vec::with_capacity(ids.len());
                    for volume_id in ids {
                        let result = self
                            .volume
                            .get_volume_with_evidence(ctx, &volume_id, &mut evidence)
                            .await;
                        if result.is_ok()
                            || result.is_err_and(|error| !error.has_class(ErrorClass::NotFound))
                        {
                            kept.push(volume_id);
                        }
                    }
                    ids = kept;
                }
            }
            delete_output(ids, input.delete_volumes, &evidence).ok_or_else(|| {
                Failure::new(
                    delete_inventory_error(&evidence),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                )
            })
        })
    }

    async fn refuse_server_name(
        &self,
        ctx: &RequestContext,
        name: &str,
        evidence: &mut Evidence,
    ) -> Result<(), Failure<CreateServerOutput>> {
        let ctx = self.server_deadline(ctx);
        let invalid = || {
            not_sent(invalid_input(
                "compute.CreateServer: could not confirm no server has the requested name: incomplete inventory",
            ))
        };
        let mut metadata = None;
        let mut ids = HashSet::<[u8; 32]>::new();
        for page in 1..=50 {
            let results = self
                .server_page::<()>(&ctx, page, 10_000, evidence)
                .await
                .map_err(|error| {
                    let error = error.into_parts().0;
                    if status(&error) == Some(200) {
                        invalid()
                    } else {
                        not_sent(error)
                    }
                })?;
            let totals = (results.total_page, results.total_item, results.page_size);
            if results.page != page
                || results.total_page > 50
                || (results.items.is_empty()
                    && (results.total_item != 0 || page != 1 || results.total_page > 1))
                || metadata.replace(totals).is_some_and(|old| old != totals)
            {
                return Err(invalid());
            }
            for server in results.items {
                if server.name.is_empty()
                    || server.uuid.is_empty()
                    || evidence.text_reflects(&server.name)
                    || evidence.text_reflects(&server.uuid)
                    || !ids.insert(Sha256::digest(server.uuid.as_bytes()).into())
                {
                    return Err(invalid());
                }
                if server.name == name {
                    return Err(not_sent(invalid_input(
                        "compute.CreateServer: a server with the requested name already exists",
                    )));
                }
            }
            let count = i64::try_from(ids.len()).map_err(|_| invalid())?;
            if count > results.total_item {
                return Err(invalid());
            }
            if page >= results.total_page {
                return if count == results.total_item {
                    Ok(())
                } else {
                    Err(invalid())
                };
            }
        }
        Err(invalid())
    }
}

fn create_body(input: &CreateServerInput) -> Result<Value, Box<Error>> {
    let invalid = |message: &str| Box::new(invalid_input(&format!("{}: {message}", CREATE.id)));
    if input.allow_open_login && !input.attach_floating_ip {
        return Err(invalid("AllowOpenLogin requires AttachFloatingIP"));
    }
    if input.attach_floating_ip {
        return Err(invalid(
            "floating IP creation requires security group exposure checks",
        ));
    }
    let data = input
        .user_data
        .as_ref()
        .filter(|data| !data.expose_secret().is_empty());
    match (input.ssh_key_id.is_empty(), data.is_some()) {
        (true, false) => {
            return Err(invalid(
                "login is required: an SSH key, or user data that installs keys",
            ));
        }
        (false, true) => {
            return Err(invalid(
                "GreenNode refuses user data together with an SSH key; put the key in the cloud-config",
            ));
        }
        _ => {}
    }
    let mut value = json!({
        "name": input.name, "zoneId": input.zone_id, "flavorId": input.flavor_id,
        "imageId": input.image_id, "networkId": input.vpc_id, "subnetId": input.subnet_id,
        "securityGroup": input.security_group_ids, "rootDiskSize": input.root_disk_size,
        "rootDiskTypeId": input.root_disk_type_id, "encryptionVolume": encrypted(input),
        "isEnableAutoRenew": input.auto_renew, "attachFloating": input.attach_floating_ip,
    });
    for (key, text) in [
        ("sshKeyId", &input.ssh_key_id),
        ("dataDiskName", &input.data_disk_name),
        ("dataDiskTypeId", &input.data_disk_type_id),
        (
            "rootDiskEncryptionType",
            &input.root_disk_encryption_type_id,
        ),
        (
            "dataDiskEncryptionType",
            &input.data_disk_encryption_type_id,
        ),
        ("serverGroupId", &input.server_group_id),
    ] {
        if !text.is_empty() {
            value[key] = json!(text);
        }
    }
    if input.data_disk_size != 0 {
        value["dataDiskSize"] = json!(input.data_disk_size);
    }
    if let Some(data) = data {
        value["userData"] = json!(encoded_user_data(data).expose_secret());
        value["userDataBase64Encoded"] = json!(true);
    }
    Ok(value)
}

fn create_id(body: &[u8], evidence: &Evidence) -> Result<String, Box<Error>> {
    let invalid = || Box::new(invalid_response(evidence));
    #[derive(Deserialize)]
    struct Acknowledgement {
        data: Option<Created>,
    }

    #[derive(Deserialize)]
    struct Created {
        uuid: Option<Field<String>>,
    }

    use strict_json::{Shape, ShapeField};
    const SHAPE: Shape<'_> = Shape::Object(&[ShapeField {
        name: "data",
        shape: Shape::Object(&[ShapeField {
            name: "uuid",
            shape: Shape::Any,
        }]),
    }]);
    let id = if body.is_empty() {
        String::new()
    } else {
        let text = go_json(body);
        strict_json::validate_exact_shape(text.as_bytes(), &SHAPE, 8 << 20)
            .map_err(|_| invalid())?;
        if evidence.go_peer_body_reflects(body).unwrap_or(true) {
            return Err(invalid());
        }
        serde_json::from_str::<Option<Acknowledgement>>(&text)
            .map_err(|_| invalid())?
            .and_then(|ack| ack.data)
            .and_then(|data| data.uuid)
            .map_or_else(String::new, |uuid| uuid.0)
    };
    if id.is_empty() {
        return Err(Box::new(api(
            CREATE.id,
            Some(202),
            "",
            CREATE_UNKNOWN,
            evidence,
        )));
    }
    if evidence.text_reflects(&id) || path_id(CREATE.id, "ServerID", &id).is_err() {
        return Err(invalid());
    }
    Ok(id)
}

fn invalid_response(evidence: &Evidence) -> Error {
    api(
        CREATE.id,
        Some(202),
        "InvalidResponse",
        CREATE_AMBIGUOUS,
        evidence,
    )
}

fn delete_inventory_error(evidence: &Evidence) -> Error {
    api(
        DELETE.id,
        None,
        "InvalidResponse",
        "invalid attached volume inventory; body withheld",
        evidence,
    )
}

fn create_error(
    failed: Failure<CreateServerOutput>,
    sensitive: bool,
    evidence: &Evidence,
) -> Failure<CreateServerOutput> {
    let (error, _, dispatch) = failed.into_parts();
    let remote_status = status(&error);
    let error = if remote_status == Some(400)
        || (sensitive && remote_status.is_some_and(|status| (400..600).contains(&status)))
    {
        let observed = error.api_error();
        let code = observed.map_or("", ApiError::code);
        let message = if sensitive {
            USER_DATA_WITHHELD
        } else {
            observed.map_or("Bad Request", ApiError::message)
        };
        let message = if remote_status == Some(400) {
            let end = message.floor_char_boundary(512 - DISK_ADVICE.len() - 2);
            format!("{}; {DISK_ADVICE}", &message[..end])
        } else {
            message.to_owned()
        };
        evidence.screen_error(Error::from_api(ApiError::new(
            CREATE.id,
            remote_status,
            SafeText::new(code, &[]),
            SafeText::new(&message, &[]),
            observed.is_some_and(ApiError::retryable),
        )))
    } else {
        error
    };
    let error = if dispatch == WriteDispatch::MayHaveBeenSent
        && !remote_status.is_some_and(|status| (400..500).contains(&status))
        && !evidence.text_reflects(CREATE_AMBIGUOUS)
    {
        let reason = error.api_error().map_or("", ApiError::message);
        let reason = if evidence.text_reflects(reason) {
            ""
        } else {
            reason
        };
        let end = reason.floor_char_boundary(512 - CREATE_AMBIGUOUS.len() - 2);
        let advice = if reason.is_empty() {
            CREATE_AMBIGUOUS.to_owned()
        } else {
            format!("{}; {CREATE_AMBIGUOUS}", &reason[..end])
        };
        if evidence.text_reflects(&advice) {
            error
        } else {
            error.with_detail(crate::error::RecoveryAdvice::new(&advice))
        }
    } else {
        error
    };
    Failure::new(error, None, dispatch)
}

fn delete_output(
    ids: Vec<String>,
    delete: bool,
    evidence: &Evidence,
) -> Option<DeleteServerOutput> {
    if !evidence.can_retain_partial() {
        return None;
    }
    let retained = ids
        .into_iter()
        .map(|id| {
            if evidence.text_reflects(&id) {
                "<redacted>".into()
            } else {
                id
            }
        })
        .collect();
    Some(if delete {
        DeleteServerOutput {
            deleted_volume_ids: retained,
            kept_volume_ids: Vec::new(),
        }
    } else {
        DeleteServerOutput {
            deleted_volume_ids: Vec::new(),
            kept_volume_ids: retained,
        }
    })
}

pub(super) fn check_price(
    op: &str,
    quoted: f64,
    cap: f64,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    if evidence.output_reflects(&json!([quoted, cap]))
        || evidence.text_reflects(&format!("{quoted:.0} {cap:.0}"))
        || evidence.reflects_fixed_two(quoted)
    {
        return Err(Box::new(api(
            "pricing.GetQuote",
            Some(200),
            "InvalidResponse",
            "invalid quote response; body withheld",
            evidence,
        )));
    }
    let (class, code, message) = if quoted == 0.0 {
        (
            ErrorClass::Unpriced,
            "Unpriced",
            format!(
                "vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource: {op}: quote {quoted} VND"
            ),
        )
    } else if quoted > cap {
        (
            ErrorClass::PriceAboveMax,
            "PriceAboveMax",
            format!(
                "vngcloud: quoted price is above MaxPrice: {op}: quote {quoted:.0} VND exceeds MaxPrice {cap:.0} VND"
            ),
        )
    } else {
        return Ok(());
    };
    Err(Box::new(
        Error::new(class).with_cause(api("", None, code, &message, evidence)),
    ))
}
