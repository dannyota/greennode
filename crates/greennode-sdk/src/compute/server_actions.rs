use std::time::Duration;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};

use crate::wire::product::{evidence::Evidence, redirect::Retry};
use crate::{Error, Failure, RequestContext, WriteDispatch};

use super::mutation::{
    Mutation, Settlement, decode_data, invalid_mutation_response, not_sent, rejected, safe_server,
    settlement_error,
};
use super::quotes::{path_id, required, resize_info};
use super::server_writes::check_price;
use super::wire::{api, invalid_input};
use super::{Client, ResizeServerInput, Server};

const START: Mutation = action(
    "compute.StartServer",
    "v2/{}/servers/{}/start",
    Retry::Once,
    &[StatusCode::ACCEPTED],
    false,
);
const STOP: Mutation = action(
    "compute.StopServer",
    "v2/{}/servers/{}/stop",
    Retry::Once,
    &[StatusCode::ACCEPTED],
    false,
);
const REBOOT: Mutation = action(
    "compute.RebootServer",
    "v2/{}/servers/{}/reboot",
    Retry::Once,
    &[StatusCode::ACCEPTED],
    false,
);
const RENAME: Mutation = action(
    "compute.RenameServer",
    "v2/{}/servers/{}/rename",
    Retry::Repeatable,
    &[StatusCode::OK],
    false,
);
const RESIZE: Mutation = action(
    "compute.ResizeServer",
    "v2/{}/servers/{}/resize",
    Retry::Once,
    &[StatusCode::ACCEPTED],
    false,
);

const fn action(
    id: &'static str,
    template: &'static str,
    retry: Retry,
    success: &'static [StatusCode],
    overflow_overrides_status: bool,
) -> Mutation {
    Mutation {
        id,
        template,
        method: Method::PUT,
        overflow_overrides_status,
        retry,
        success,
    }
}

/// A server status prevented the requested action.
#[derive(Clone, Copy)]
pub struct UnexpectedStatus;

crate::error::debug_record!(UnexpectedStatus unit);

/// Identifies the server to start.
#[derive(Clone, Default)]
pub struct StartServerInput {
    /// Server to start.
    pub server_id: String,
    /// Skip the status confirmation read.
    pub no_wait: bool,
}

crate::error::debug_record!(StartServerInput { server_id, no_wait });

/// Identifies the server to stop.
#[derive(Clone, Default)]
pub struct StopServerInput {
    /// Server to stop.
    pub server_id: String,
    /// Skip the status confirmation read.
    pub no_wait: bool,
}

crate::error::debug_record!(StopServerInput { server_id, no_wait });

/// Identifies the server to reboot.
#[derive(Clone, Default)]
pub struct RebootServerInput {
    /// Server to reboot.
    pub server_id: String,
    /// Skip the status confirmation read.
    pub no_wait: bool,
}

crate::error::debug_record!(RebootServerInput { server_id, no_wait });

/// Identifies a server and its new name.
#[derive(Clone, Default)]
pub struct RenameServerInput {
    /// Server to rename.
    pub server_id: String,
    /// New server name.
    pub name: String,
}

crate::error::debug_record!(RenameServerInput { server_id, name });

/// Result of starting a server.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartServerOutput {
    /// Last verified server state.
    pub server: Server,
    /// Whether the write was acknowledged.
    pub changed: bool,
}

crate::wire::product::output::public_fields!(StartServerOutput { server, changed });

/// Result of stopping a server.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StopServerOutput {
    /// Last verified server state.
    pub server: Server,
    /// Whether the write was acknowledged.
    pub changed: bool,
}

crate::wire::product::output::public_fields!(StopServerOutput { server, changed });

/// Result of rebooting a server.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RebootServerOutput {
    /// Last verified server state.
    pub server: Server,
}

crate::wire::product::output::public_fields!(RebootServerOutput { server });

/// Server returned by a successful rename.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RenameServerOutput {
    /// The renamed server.
    pub server: Server,
}

crate::wire::product::output::public_fields!(RenameServerOutput { server });

/// Result of changing the server's flavor.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResizeServerOutput {
    /// Last verified server state.
    pub server: Server,
    /// Signed quote in VND. Negative values indicate a credit.
    pub quoted_adjustment: f64,
}

crate::wire::product::output::public_fields!(ResizeServerOutput {
    server,
    quoted_adjustment
});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Starts a stopped server, or returns an active server without sending a write.
    pub async fn start_server(
        &self,
        ctx: &RequestContext,
        input: StartServerInput,
    ) -> Result<StartServerOutput, Failure<StartServerOutput>> {
        crate::wire::product::exit::finish!(async {
            self.toggle(
                ctx,
                &START,
                &input.server_id,
                input.no_wait,
                ("STOPPED", "ACTIVE"),
                |server, changed| StartServerOutput { server, changed },
            )
            .await
        })
    }

    /// Stops an active server, or returns a stopped server without sending a write.
    pub async fn stop_server(
        &self,
        ctx: &RequestContext,
        input: StopServerInput,
    ) -> Result<StopServerOutput, Failure<StopServerOutput>> {
        crate::wire::product::exit::finish!(async {
            self.toggle(
                ctx,
                &STOP,
                &input.server_id,
                input.no_wait,
                ("ACTIVE", "STOPPED"),
                |server, changed| StopServerOutput { server, changed },
            )
            .await
        })
    }

    /// Reboots an active server and optionally waits for it to return to active.
    pub async fn reboot_server(
        &self,
        ctx: &RequestContext,
        input: RebootServerInput,
    ) -> Result<RebootServerOutput, Failure<RebootServerOutput>> {
        crate::wire::product::exit::finish!(async {
            const ADVICE: &str = "read get-server and inspect the console before repeating reboot";
            let mut evidence = Evidence::for_write();
            let id = &input.server_id;
            let mut server = self
                .action_current(ctx, &REBOOT, id, None, &mut evidence)
                .await?;
            require_status(&REBOOT, id, &server, &["ACTIVE"], &evidence)
                .map_err(|error| not_sent(*error))?;
            let project = |server| RebootServerOutput { server };
            if let Err(error) = self
                .send_mutation(ctx, &REBOOT, &[id], None, false, &mut evidence)
                .await
            {
                return Err(ambiguous(
                    error,
                    &REBOOT,
                    ADVICE,
                    safe_server(server, &evidence).map(project),
                    &evidence,
                ));
            }
            let result = if input.no_wait {
                Ok(())
            } else {
                let earliest = self.session.clock().monotonic_now() + Duration::from_secs(10);
                self.settle_server(
                    ctx,
                    id,
                    Settlement::Rebooted(earliest),
                    &mut server,
                    &mut evidence,
                )
                .await
            };
            finish(
                &REBOOT,
                result,
                safe_server(server, &evidence).map(project),
                ADVICE,
                &evidence,
            )
        })
    }

    /// Renames a server and reads the server returned in the acknowledgement.
    pub async fn rename_server(
        &self,
        ctx: &RequestContext,
        input: RenameServerInput,
    ) -> Result<RenameServerOutput, Failure<RenameServerOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = &input.server_id;
            required(
                RENAME.id,
                &[
                    ("ServerID", !id.is_empty()),
                    ("Name", !input.name.is_empty()),
                ],
            )
            .map_err(|error| not_sent(*error))?;
            path_id(RENAME.id, "ServerID", id).map_err(|error| not_sent(*error))?;
            let body = json!({"newName": input.name});
            let mut evidence = Evidence::for_write();
            self.guard_write(ctx, &json!([id, body]), &mut evidence)
                .map_err(|error| not_sent(*error))?;
            let raw = self
                .send_mutation(ctx, &RENAME, &[id], Some(&body), false, &mut evidence)
                .await?;
            let server = decode_data::<Server>(&raw, &RENAME, &evidence)
                .map_err(|error| Failure::new(*error, None, WriteDispatch::MayHaveBeenSent))?;
            let output = (!server.name.is_empty()
                && (server.uuid.is_empty() || server.uuid == *id)
                && !evidence.text_reflects(&server.uuid))
            .then(|| safe_server(server, &evidence))
            .flatten()
            .map(|server| RenameServerOutput { server });
            output.ok_or_else(|| {
                Failure::new(
                    invalid_mutation_response(&RENAME, &evidence),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                )
            })
        })
    }

    /// Checks and quotes a flavor change, then resizes the server at most once.
    pub async fn resize_server(
        &self,
        ctx: &RequestContext,
        input: ResizeServerInput,
    ) -> Result<ResizeServerOutput, Failure<ResizeServerOutput>> {
        crate::wire::product::exit::finish!(async {
            const ADVICE: &str = "call GetServer to check its status";
            let id = &input.server_id;
            let info = resize_info(RESIZE.id, &input).map_err(|error| not_sent(*error))?;
            if !input.max_price.is_finite() || input.max_price < 0.0 {
                return Err(not_sent(invalid_input(
                    "compute.ResizeServer: MaxPrice must be finite and non-negative",
                )));
            }
            let mut evidence = Evidence::for_write();
            let body = json!(info);
            let mut server = self
                .action_current(
                    ctx,
                    &RESIZE,
                    id,
                    Some(&json!([body, input.max_price])),
                    &mut evidence,
                )
                .await?;
            if evidence.id_reflects(&server.flavor.flavor_id) {
                return Err(not_sent(invalid_preflight(&evidence)));
            }
            if server.flavor.flavor_id == input.flavor_id {
                return Err(not_sent(evidence.screen_error(invalid_input(&format!(
                    "{}: server {id} already has flavor {}",
                    RESIZE.id, input.flavor_id
                )))));
            }
            require_status(&RESIZE, id, &server, &["ACTIVE", "STOPPED"], &evidence)
                .map_err(|error| not_sent(*error))?;
            let quote = self
                .server_quote(ctx, "server", "resize", info, false, &mut evidence)
                .await
                .map_err(|error| not_sent(*error))?;
            let quoted_adjustment = quote.optimum_price;
            check_price(RESIZE.id, quoted_adjustment, input.max_price, &mut evidence)
                .map_err(|error| not_sent(*error))?;
            let output = |server, evidence: &Evidence| {
                safe_server(server, evidence)
                    .filter(|_| {
                        !evidence.output_reflects(&json!(quoted_adjustment))
                            && !evidence.reflects_fixed_two(quoted_adjustment)
                    })
                    .map(|server| ResizeServerOutput {
                        server,
                        quoted_adjustment,
                    })
            };
            if let Err(error) = self
                .send_mutation(ctx, &RESIZE, &[id], Some(&body), false, &mut evidence)
                .await
            {
                return Err(ambiguous(
                    error,
                    &RESIZE,
                    ADVICE,
                    output(server, &evidence),
                    &evidence,
                ));
            }
            let result = if input.no_wait {
                Ok(())
            } else {
                self.settle_server(
                    ctx,
                    id,
                    Settlement::Resized(&input.flavor_id),
                    &mut server,
                    &mut evidence,
                )
                .await
            };
            finish(
                &RESIZE,
                result,
                output(server, &evidence),
                ADVICE,
                &evidence,
            )
        })
    }

    async fn action_current<O>(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        id: &str,
        body: Option<&Value>,
        evidence: &mut Evidence,
    ) -> Result<Server, Failure<O>> {
        path_id(mutation.id, "ServerID", id).map_err(|error| not_sent(*error))?;
        self.guard_write(ctx, &json!([id, body]), evidence)
            .map_err(|error| not_sent(*error))?;
        let server = self
            .server_guard::<()>(ctx, id, evidence)
            .await
            .map_err(|error| not_sent(error.into_parts().0))?;
        server
            .into_checked(evidence)
            .map_err(|error| not_sent(*error))
    }

    async fn toggle<O>(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        id: &str,
        no_wait: bool,
        states: (&str, &'static str),
        projection: impl Fn(Server, bool) -> O,
    ) -> Result<O, Failure<O>> {
        const ADVICE: &str = "run this operation again to check";
        let (source, target) = states;
        let mut evidence = Evidence::for_write();
        let mut server = self
            .action_current(ctx, mutation, id, None, &mut evidence)
            .await?;
        require_status(mutation, id, &server, &[source, target], &evidence)
            .map_err(|error| not_sent(*error))?;
        let output = |server, changed, evidence: &Evidence| {
            safe_server(server, evidence)
                .filter(|_| !evidence.output_reflects(&json!(changed)))
                .map(|server| projection(server, changed))
        };
        if server.status.eq_ignore_ascii_case(target) {
            return output(server, false, &evidence)
                .ok_or_else(|| not_sent(invalid_preflight(&evidence)));
        }
        if let Err(error) = self
            .send_mutation(ctx, mutation, &[id], None, false, &mut evidence)
            .await
        {
            return Err(ambiguous(
                error,
                mutation,
                ADVICE,
                output(server, false, &evidence),
                &evidence,
            ));
        }
        let result = if no_wait {
            Ok(())
        } else {
            self.settle_server(
                ctx,
                id,
                Settlement::Status(target),
                &mut server,
                &mut evidence,
            )
            .await
        };
        finish(
            mutation,
            result,
            output(server, true, &evidence),
            ADVICE,
            &evidence,
        )
    }
}

fn require_status(
    mutation: &Mutation,
    id: &str,
    server: &Server,
    allowed: &[&str],
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    if evidence.output_reflects(&json!([server.uuid, server.status])) {
        return Err(Box::new(invalid_preflight(evidence)));
    }
    if allowed
        .iter()
        .any(|status| server.status.eq_ignore_ascii_case(status))
    {
        return Ok(());
    }
    Err(Box::new(
        api(
            "",
            None,
            "UnexpectedStatus",
            &format!(
                "compute: unexpected status: {}: server {id} is {:?}",
                mutation.id, server.status
            ),
            evidence,
        )
        .with_detail(UnexpectedStatus),
    ))
}

fn invalid_preflight(evidence: &Evidence) -> Error {
    api(
        "compute.GetServer",
        Some(200),
        "InvalidResponse",
        "invalid server response; body withheld",
        evidence,
    )
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
pub(super) fn finish<O>(
    mutation: &Mutation,
    result: Result<(), Box<Error>>,
    output: Option<O>,
    advice: &str,
    evidence: &Evidence,
) -> Result<O, Failure<O>> {
    match (result, output) {
        (Ok(()), Some(output)) => Ok(output),
        (Ok(()), None) => Err(Failure::new(
            invalid_mutation_response(mutation, evidence),
            None,
            WriteDispatch::MayHaveBeenSent,
        )),
        (Err(error), output) => Err(Failure::new(
            settlement_error(mutation.id, *error, advice, evidence),
            output,
            WriteDispatch::MayHaveBeenSent,
        )),
    }
}

fn ambiguous<O>(
    error: Failure<O>,
    mutation: &Mutation,
    advice: &str,
    partial: Option<O>,
    evidence: &Evidence,
) -> Failure<O> {
    let (error, _, dispatch) = error.into_parts();
    if dispatch == WriteDispatch::NotSent || rejected(&error) {
        return Failure::new(error, None, dispatch);
    }
    Failure::new(
        settlement_error(mutation.id, error, advice, evidence),
        partial,
        dispatch,
    )
}
