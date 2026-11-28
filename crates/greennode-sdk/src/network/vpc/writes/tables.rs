use http::{Method, StatusCode};
use serde_json::json;

use crate::model::wire::model;
use crate::network::RouteTable;
use crate::{ErrorClass, Failure, RequestContext};

use super::super::super::Client;
use super::models::*;
use super::wire::{CREATE_BOUND, INTERVAL, NetworkWriteError, Request, Write, not_sent};

const CREATE: Request = Request::new(
    "network.CreateRouteTable",
    "v2/{}/route-table",
    &Method::POST,
    &[StatusCode::ACCEPTED],
);
const DELETE: Request = Request::new(
    "network.DeleteRouteTable",
    "v2/{}/route-table/{}",
    &Method::DELETE,
    &[StatusCode::ACCEPTED],
)
.repeatable();
const CREATE_ADVICE: &str =
    "the table may exist, list route tables and match the name exactly before creating it again";

model! {
    struct CreatedRouteTable {
        uuid: String => "uuid", "UUID";
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Creates a route table and optionally waits for it to become active.
    pub async fn create_route_table(
        &self,
        ctx: &RequestContext,
        input: CreateRouteTableInput,
    ) -> Result<CreateRouteTableOutput, Failure<CreateRouteTableOutput>> {
        let mut write = Write::new(self, ctx)?;
        let op = CREATE.id;
        write.check(op, &[("VPCID", &input.vpc_id), ("Name", &input.name)], &[])?;
        let reply = write
            .send(
                CREATE,
                &[],
                Some(json!({"name": input.name, "networkId": input.vpc_id})),
            )
            .await
            .map_err(|error| write.advise(error, CREATE_ADVICE))?;
        let created = write.created(
            CREATE,
            reply,
            false,
            |row: &CreatedRouteTable| &row.uuid,
            !input.no_wait,
            CREATE_ADVICE,
        )?;
        let mut route_table = RouteTable {
            uuid: created.uuid,
            name: input.name,
            network_id: input.vpc_id,
            ..Default::default()
        };
        let id = route_table.uuid.clone();
        if input.no_wait {
            return write.outcome(op, route_table, Ok(()), |route_table| {
                CreateRouteTableOutput { route_table }
            });
        }
        let result = write.settle(CREATE_BOUND, INTERVAL,
            &format!("{op}: route table {id} did not reach ACTIVE within 3m0s; the table exists and this create must not be repeated"),
            async |write, ctx| {
                route_table = match write.table(ctx, &id).await {
                    Ok(row) => row,
                    Err(error) if error.has_class(ErrorClass::NotFound) => return Ok(None),
                    Err(error) => return Err(write.settled_error(&format!("{op}: route table {id}"), error)),
                };
                match route_table.status.as_str() {
                    "ACTIVE" => Ok(Some(())),
                    "ERROR" => Err(write.status_error(op, &format!("route table {id}"))),
                    _ => Ok(None),
                }
            }).await;
        write.outcome(op, route_table, result, |route_table| {
            CreateRouteTableOutput { route_table }
        })
    }

    /// Deletes a route table after checking its VPC's subnet associations.
    pub async fn delete_route_table(
        &self,
        ctx: &RequestContext,
        input: DeleteRouteTableInput,
    ) -> Result<DeleteRouteTableOutput, Failure<DeleteRouteTableOutput>> {
        let mut write = Write::new(self, ctx)?;
        let id = &input.route_table_id;
        let op = DELETE.id;
        write.check(op, &[], &[("RouteTableID", id)])?;
        let table = write.table(ctx, id).await.map_err(not_sent)?;
        let vpc = write.vpc(ctx, &table.network_id).await.map_err(not_sent)?;
        let subnets = write
            .subnets(ctx, &table.network_id)
            .await
            .map_err(not_sent)?;
        if subnets.iter().any(|subnet| subnet.route_table_id == *id) {
            return Err(not_sent(write.in_use(&format!(
                "{op}: route table {id} is named by a subnet of its VPC"
            ))));
        }
        if vpc.route_table_id == *id
            && subnets
                .iter()
                .any(|subnet| subnet.route_table_id.is_empty())
        {
            return Err(not_sent(write.guard_error(NetworkWriteError::DefaultResource,
                &format!("{op}: route table {id} is the VPC's main route table and a subnet names no route table, so it relies on this one"))));
        }
        write.send(DELETE, &[id], None).await?;
        if input.no_wait {
            return Ok(DeleteRouteTableOutput {});
        }
        let result = write.settle(CREATE_BOUND, INTERVAL,
            &format!("{op}: route table {id} still exists after 3m0s; the delete may still complete and this call is safe to run again"),
            async |write, ctx| match write.table(ctx, id).await {
                Ok(row) if row.status == "ERROR" => Err(write.status_error(op, &format!("route table {id}"))),
                Ok(_) => Ok(None),
                Err(error) if error.has_class(ErrorClass::NotFound) => Ok(Some(())),
                Err(error) => Err(write.settled_error(&format!("{op}: route table {id}"), error)),
            }).await;
        write.outcome(op, (), result, |()| DeleteRouteTableOutput {})
    }
}
