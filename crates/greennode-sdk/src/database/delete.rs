use super::wire::{Data, valid_id};
use super::write::{
    Settlement, api_error, local_error, not_sent, preflight, rejection, unsettled, write_operation,
};
use super::{Client, ListDatabaseInstancesInput, read_operation};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::{Options, PathIds},
    sender::Response,
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::task::Poll;

const DELETE: &str = "database.DeleteDatabaseInstance";
const MAX_LIST_PAGES: i32 = 50;
const INTERRUPTED: Settlement = Settlement::NotSettled(
    "vngcloud: write outcome not confirmed: delete confirmation interrupted",
);

/// Selects an exact database instance ID for deletion.
#[derive(Clone, Debug)]
pub struct DeleteDatabaseInstanceInput {
    /// Exact instance ID.
    pub id: String,
}

impl DeleteDatabaseInstanceInput {
    /// Selects the instance to check and delete.
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

/// The selected instance and whether deletion was confirmed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeleteDatabaseInstanceOutput {
    /// Instance ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// True only after the instance reports deletion or is absent.
    #[serde(rename = "Deleted")]
    pub deleted: bool,
}

/// Delete preflight found an ineligible instance and sent no mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeleteRefused;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Sends one delete for an eligible backup-free MySQL instance and confirms its absence.
    pub async fn delete_database_instance(
        &self,
        ctx: &RequestContext,
        input: DeleteDatabaseInstanceInput,
    ) -> Result<DeleteDatabaseInstanceOutput, Failure<DeleteDatabaseInstanceOutput>> {
        let id = input.id;
        if !valid_id(&id) {
            let message = if id.is_empty() {
                "vngcloud: invalid input: database.DeleteDatabaseInstance requires ID"
            } else {
                "vngcloud: invalid input: database.DeleteDatabaseInstance: ID must contain only letters, digits, hyphens, and underscores"
            };
            return Err(not_sent(local_error(
                ErrorClass::InvalidInput,
                "InvalidUsage",
                message,
            )));
        }
        self.write_provenance()?;
        let mut evidence = Evidence::new();
        self.delete_preflight(ctx, &id, &mut evidence).await?;
        let body = json!({
            "databaseInstances": [{"instancesId": id, "config": {"createFinalBackup": false, "deleteAllBackup": false}}],
            "action": "delete", "resType": "dbaas",
        });
        let operation = write_operation(
            DELETE,
            "vdb-relational/v1/database-instances/{}/delete",
            &SUCCESS_2XX,
            true,
        );
        let options = Options {
            path_ids: PathIds::WithUnderscore,
            ..Options::DEFAULT
        };
        let reply = match self
            .send_write::<DeleteReply>(
                ctx,
                &operation,
                &options,
                &[&id],
                body.to_string().into(),
                &mut evidence,
            )
            .await
        {
            Ok(reply) => reply,
            Err(failure) => {
                let reason = if failure.error().detail::<UnsafeReply>().is_some() {
                    "vngcloud: write outcome not confirmed: delete response withheld"
                } else {
                    "vngcloud: write outcome not confirmed: delete request unconfirmed"
                };
                return Err(rejection(failure, DELETE).unwrap_or_else(|| {
                    unsettled(
                        DELETE,
                        Settlement::NotSettled(reason),
                        Some(id),
                        &evidence,
                        unconfirmed,
                    )
                }));
            }
        };
        if reply.rejected() {
            return Err(Failure::new(
                api_error(
                    DELETE,
                    Some(200),
                    "DeleteRejected",
                    "delete not accepted",
                    false,
                ),
                None,
                WriteDispatch::MayHaveBeenSent,
            ));
        }
        let result = self
            .poll_instance(
                ctx,
                &id,
                &mut evidence,
                INTERRUPTED,
                |instance| match instance {
                    None => Poll::Ready(Ok(())),
                    Some(instance) => match instance.status.to_ascii_uppercase().as_str() {
                        "DELETED" => Poll::Ready(Ok(())),
                        "ACTIVE" | "DELETING" => Poll::Pending,
                        _ => Poll::Ready(Err(Settlement::NotSettled(
                            "vngcloud: write outcome not confirmed: delete status unrecognized",
                        ))),
                    },
                },
            )
            .await;
        match result {
            Ok(()) => {
                let id = if evidence.credentials().reflects(id.as_bytes()) {
                    "<redacted>".to_owned()
                } else {
                    id
                };
                Ok(DeleteDatabaseInstanceOutput { id, deleted: true })
            }
            Err(settlement) => Err(unsettled(
                DELETE,
                settlement,
                Some(id),
                &evidence,
                unconfirmed,
            )),
        }
    }

    async fn delete_preflight(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<(), Failure<DeleteDatabaseInstanceOutput>> {
        let instance = self
            .get_instance::<()>(ctx, id, evidence)
            .await
            .map_err(|failure| preflight(failure, "GetDatabaseInstance"))?;
        if !instance.datastore_type.eq_ignore_ascii_case("mysql") {
            return Err(refuse("the instance is not MySQL"));
        }
        if !instance.status.eq_ignore_ascii_case("ACTIVE") {
            return Err(refuse("the instance is not in the stable ACTIVE state"));
        }
        if instance.owner == Some(false) {
            return Err(refuse("the instance is shared, not owned by this account"));
        }
        if instance.deleted == Some(true) {
            return Err(refuse("the instance is already marked deleted"));
        }
        if !instance.replicas.is_empty() || !instance.replica_source_id.is_empty() {
            return Err(refuse("the instance has replication"));
        }
        #[cfg(any(test, feature = "test-support"))]
        if self
            .session
            .is_write_fixture(crate::testing::Product::Database)
            && self
                .session
                .project_id_context(ctx)
                .await
                .map_err(|error| not_sent(*error))?
                .is_none()
        {
            return Err(not_sent(Error::new(ErrorClass::InvalidConfig)));
        }
        let project = self
            .session
            .require_project_id(ctx)
            .await
            .map_err(|error| not_sent(*error))?;
        if !instance.project_id.is_empty() && instance.project_id != project {
            return Err(refuse("the instance is not in the selected project"));
        }
        self.prove_in_project(ctx, id, &project, evidence).await?;
        let zones = self
            .zones(ctx, evidence)
            .await
            .map_err(|failure| preflight(failure, "ListDatabaseZones"))?;
        if instance.zone_id.is_empty()
            || !zones
                .items
                .iter()
                .any(|zone| zone.uuid == instance.zone_id || zone.name == instance.zone_id)
        {
            return Err(refuse(
                "the instance zone is not a database zone of this region",
            ));
        }
        let backups: Vec<Backup> = self
            .read_path(
                ctx,
                &read_operation(DELETE, "vdb-relational/v1/backups/insId/{}"),
                &[id],
                &[],
                evidence,
            )
            .await
            .map_err(|failure: Failure<()>| preflight(failure, DELETE))?;
        if !backups.is_empty() {
            return Err(refuse("the instance has backups"));
        }
        Ok(())
    }

    async fn prove_in_project(
        &self,
        ctx: &RequestContext,
        id: &str,
        project: &str,
        evidence: &mut Evidence,
    ) -> Result<(), Failure<DeleteDatabaseInstanceOutput>> {
        let mut totals = (0, 0);
        let mut rows = 0u64;
        let mut matches = 0u64;
        for page_number in 1..=MAX_LIST_PAGES {
            let got = self
                .instances(
                    ctx,
                    ListDatabaseInstancesInput {
                        page_number,
                        page_size: 0,
                    },
                    evidence,
                )
                .await
                .map_err(|failure| preflight(failure, "ListDatabaseInstances"))?;
            if got.project_id != project {
                return Err(refuse("the instance list is not for the selected project"));
            }
            let page = got.page;
            let reported = (page.total_pages, page.total_elements);
            if page.size < 1
                || (!got.items.is_empty() && (page.total_pages < 1 || page.total_elements < 1))
            {
                return Err(refuse("the instance list paging metadata is incomplete"));
            }
            if page.number != page_number
                || page.total_pages < 0
                || page.total_elements < 0
                || (page_number > 1 && reported != totals)
            {
                return Err(refuse("the instance list pages are inconsistent"));
            }
            totals = reported;
            rows += got.items.len() as u64;
            for item in got.items.iter().filter(|item| item.id == id) {
                if !item.project_id.is_empty() && item.project_id != project {
                    return Err(refuse("the listed instance belongs to another project"));
                }
                matches += 1;
            }
            if page_number >= page.total_pages {
                if u64::try_from(page.total_elements).ok() != Some(rows) {
                    return Err(refuse("the instance list is incomplete"));
                }
                if matches != 1 {
                    return Err(refuse(
                        "the instance is not listed exactly once in the selected project",
                    ));
                }
                return Ok(());
            }
        }
        Err(refuse(
            "the instance list has too many pages to verify the project",
        ))
    }
}

fn refuse(reason: &str) -> Failure<DeleteDatabaseInstanceOutput> {
    not_sent(
        api_error(
            "",
            None,
            "RequestFailed",
            &format!("database: delete refused: {reason}"),
            false,
        )
        .with_detail(DeleteRefused),
    )
}

fn unconfirmed(id: String) -> DeleteDatabaseInstanceOutput {
    DeleteDatabaseInstanceOutput { id, deleted: false }
}

#[derive(Deserialize)]
struct Backup {
    #[serde(default, rename = "id")]
    _id: Option<String>,
}

impl Data for Vec<Backup> {
    const SHAPE: Shape<'static> = Shape::Array(&Shape::Object(&[ShapeField {
        name: "id",
        shape: Shape::Any,
    }]));
    fn valid(&self) -> bool {
        true
    }
}

#[derive(Default, Deserialize)]
struct DeleteReply(Value);

impl DeleteReply {
    fn rejected(&self) -> bool {
        self.0
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|rows| {
                rows.iter()
                    .any(|row| row.get("success").and_then(Value::as_bool) == Some(false))
            })
    }
}

struct UnsafeReply;

impl Output for DeleteReply {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        if response.status.is_success() && response.body.is_empty() {
            return Ok(Self::default());
        }
        operation::decode_response(operation, response).map_err(|error| {
            if response.status.is_success()
                && serde_json::from_slice::<&serde_json::value::RawValue>(&response.body).is_ok()
            {
                Box::new(error.with_detail(UnsafeReply))
            } else {
                error
            }
        })
    }
}

const SUCCESS_2XX: [StatusCode; 100] = {
    let mut statuses = [StatusCode::OK; 100];
    let mut code = 200;
    while code < 300 {
        if let Ok(status) = StatusCode::from_u16(code) {
            statuses[(code - 200) as usize] = status;
        }
        code += 1;
    }
    statuses
};
