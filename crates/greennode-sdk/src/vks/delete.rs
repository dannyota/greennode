use super::{
    Client,
    write::{not_sent, valid_id, wait_cluster},
};
use crate::RequestContext;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    redirect::{Classification, Retry},
    retry,
};
use crate::wire::strict_json::Shape;
use crate::{Error, ErrorClass, Failure, WriteDispatch};
use http::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Selects an exact cluster for deletion.
#[derive(Clone, Debug)]
pub struct DeleteClusterInput {
    /// Cluster ID containing only ASCII letters, digits, and hyphens.
    pub cluster_id: String,
    /// Return after acceptance without waiting for confirmed absence.
    pub no_wait: bool,
}

impl DeleteClusterInput {
    /// Selects the cluster and waits for confirmed absence by default.
    pub fn new(cluster_id: impl Into<String>) -> Self {
        Self {
            cluster_id: cluster_id.into(),
            no_wait: false,
        }
    }
}

/// Reports whether absence was confirmed after deletion.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeleteClusterOutput {
    /// True only when a subsequent read confirms absence.
    #[serde(rename = "Deleted")]
    pub deleted: bool,
}

crate::wire::product::output::public_fields!(DeleteClusterOutput { deleted });

#[derive(Default, Deserialize)]
struct Acknowledgement {}

impl Output for Acknowledgement {
    const SCHEMA: Shape<'static> = Shape::Any;
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Deletes one exact cluster and, unless requested otherwise, waits up to 30 minutes for absence.
    /// An unconfirmed result retains partial output and write dispatch evidence.
    pub async fn delete_cluster(
        &self,
        ctx: &RequestContext,
        input: DeleteClusterInput,
    ) -> Result<DeleteClusterOutput, Failure<DeleteClusterOutput>> {
        crate::wire::product::exit::finish!(async {
            self.write_base()?;
            self.delete_cluster_inner(ctx, input).await
        })
    }

    pub(super) async fn delete_cluster_inner(
        &self,
        ctx: &RequestContext,
        input: DeleteClusterInput,
    ) -> Result<DeleteClusterOutput, Failure<DeleteClusterOutput>> {
        if !valid_id(&input.cluster_id) {
            return Err(not_sent(Error::new(ErrorClass::InvalidInput).with_detail("greennode: invalid input: vks.DeleteCluster: ClusterID must be non-empty and contain only letters, digits, and hyphens")));
        }
        let mut evidence = Evidence::new();
        self.cluster_state(ctx, &input.cluster_id, &mut evidence)
            .await
            .map_err(|error| not_sent(*error))?;
        let output = DeleteClusterOutput::default();
        let operation = delete_operation();
        let base = self
            .base()
            .map_err(|failure: Failure<DeleteClusterOutput>| {
                let (error, _, _) = failure.into_parts();
                not_sent(error)
            })?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        if let Err(failure) = operation::execute::<Acknowledgement>(
            runtime,
            base,
            &operation,
            &[&input.cluster_id],
            &[],
            self.session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        {
            let (error, _, dispatch) = failure.into_parts();
            return Err(unsettled(
                super::safe_error("vks.DeleteCluster", &error),
                output,
                dispatch,
            ));
        }
        if input.no_wait {
            return Ok(output);
        }
        wait_cluster(ctx, self.session.clock(), async |wait| {
            match self
                .cluster_state(wait, &input.cluster_id, &mut evidence)
                .await
            {
                Err(error) if error.has_class(ErrorClass::NotFound) => Ok(true),
                Err(error) => Err(error),
                Ok(_) => Ok(false),
            }
        })
        .await
        .map_err(|error| unsettled(*error, output, WriteDispatch::MayHaveBeenSent))?;
        Ok(DeleteClusterOutput { deleted: true })
    }
}

fn unsettled(
    error: Error,
    output: DeleteClusterOutput,
    dispatch: WriteDispatch,
) -> Failure<DeleteClusterOutput> {
    let mut message = String::from(
        "vks.DeleteCluster: inspect inventory and billing before another mutation: greennode: write outcome not confirmed",
    );
    if let Some(api) = error.api_error() {
        use std::fmt::Write as _;
        let _ = write!(message, "\n{}: {}", api.operation(), api.message());
        if let Some(status) = api.status() {
            let _ = write!(message, " (status {status})");
        }
    } else {
        let cause = match error.class() {
            ErrorClass::Cancelled => "operation cancelled",
            ErrorClass::DeadlineExceeded => "deadline exceeded",
            _ => "request failed",
        };
        message.push('\n');
        message.push_str(cause);
    }
    Failure::new(
        Error::new(ErrorClass::NotSettled)
            .with_detail(message)
            .with_cause(error),
        Some(output),
        dispatch,
    )
}

fn delete_operation() -> Operation {
    Operation {
        id: "vks.DeleteCluster",
        template: "v1/clusters/{}",
        method: Method::DELETE,
        class: Classification::Write,
        retry: Retry::Once,
        success: &[StatusCode::OK, StatusCode::ACCEPTED],
        cap: 8 << 20,
        allow_recovery: false,
        allow_empty: true,
        no_output: true,
        keep_status: true,
        overflow_overrides_status: false,
        sensitive: true,
        reflection: crate::wire::product::operation::Reflection::Redact,
        no_redirect: true,
        retry_config: retry::Config::default(),
    }
}
