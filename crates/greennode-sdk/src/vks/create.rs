mod reply;

use super::{
    Client,
    write::{not_sent, valid_id, wait_cluster},
};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation},
    options::Options,
    redirect::{Classification, Retry},
    retry,
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use reply::{CreateReply, Outcome};
use serde::Serialize;
use serde_json::json;

const CREATE: &str = "vks.CreateCluster";
const QUERY: &[(&str, &str)] = &[("autoRenewal", "false"), ("poc", "false")];
const ACCEPTED: &[StatusCode] = &[StatusCode::OK, StatusCode::ACCEPTED];

/// Describes a private cluster and its initial node group.
#[derive(Clone, Debug)]
pub struct CreateClusterInput {
    /// Cluster name.
    pub name: String,
    /// Kubernetes version.
    pub version: String,
    /// VPC identifier.
    pub vpc_id: String,
    /// Subnet identifier.
    pub subnet_id: String,
    /// Private IPv4 pod network.
    pub pod_cidr: String,
    /// Initial node group name.
    pub node_group_name: String,
    /// Number of nodes.
    pub num_nodes: i64,
    /// Node flavor identifier.
    pub flavor_id: String,
    /// Node operating system.
    pub os: String,
    /// Node disk size in GiB.
    pub disk_size: i64,
    /// Disk type identifier.
    pub disk_type_id: String,
    /// Node security group identifiers.
    pub security_group_ids: Vec<String>,
    /// SSH key identifier.
    pub ssh_key_id: String,
    /// Return after acceptance.
    pub no_wait: bool,
}

/// Reports the accepted cluster identifier and last observed status.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CreateClusterOutput {
    /// Cluster identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Last observed status.
    #[serde(rename = "Status")]
    pub status: String,
}

fn valid_name(value: &str, max: usize) -> bool {
    (5..=max).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn private_cidr(value: &str) -> bool {
    let Some((address, bits)) = value.split_once('/') else {
        return false;
    };
    if (bits.len() > 1 && bits.starts_with('0')) || !bits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let (Ok(address), Ok(bits)) = (address.parse::<std::net::Ipv4Addr>(), bits.parse::<u32>())
    else {
        return false;
    };
    if bits > 32 {
        return false;
    }
    let address = u32::from(address);
    let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
    if address & mask != address {
        return false;
    }
    [(0x0a000000, 8), (0xac100000, 12), (0xc0a80000, 16)]
        .iter()
        .any(|(base, prefix)| bits >= *prefix && address >> (32 - prefix) == base >> (32 - prefix))
}

impl CreateClusterInput {
    /// Sets the required cluster and node group fields and waits for `ACTIVE` by default.
    #[expect(clippy::too_many_arguments, reason = "The API requires every field")]
    pub fn new(
        name: String,
        version: String,
        vpc_id: String,
        subnet_id: String,
        pod_cidr: String,
        node_group_name: String,
        num_nodes: i64,
        flavor_id: String,
        os: String,
        disk_size: i64,
        disk_type_id: String,
        security_group_ids: Vec<String>,
        ssh_key_id: String,
    ) -> Self {
        Self {
            name,
            version,
            vpc_id,
            subnet_id,
            pod_cidr,
            node_group_name,
            num_nodes,
            flavor_id,
            os,
            disk_size,
            disk_type_id,
            security_group_ids,
            ssh_key_id,
            no_wait: false,
        }
    }

    fn body(&self) -> serde_json::Value {
        json!({
            "name": self.name,
            "version": self.version,
            "releaseChannel": "STABLE",
            "enablePrivateCluster": true,
            "networkType": "CILIUM_OVERLAY",
            "vpcId": self.vpc_id,
            "cidr": self.pod_cidr,
            "azStrategy": "SINGLE",
            "listSubnetIds": [&self.subnet_id],
            "enabledLoadBalancerPlugin": false,
            "enabledBlockStoreCsiPlugin": false,
            "nodeGroup": {
                "name": self.node_group_name,
                "numNodes": self.num_nodes,
                "os": self.os,
                "flavorId": self.flavor_id,
                "diskSize": self.disk_size,
                "diskType": self.disk_type_id,
                "enablePrivateNodes": true,
                "securityGroups": self.security_group_ids,
                "sshKeyId": self.ssh_key_id,
                "subnetId": self.subnet_id,
                "upgradeConfig": {"strategy": "SURGE"},
            },
        })
    }

    fn resource_ids(&self) -> impl Iterator<Item = &str> {
        [
            &self.vpc_id,
            &self.subnet_id,
            &self.flavor_id,
            &self.disk_type_id,
            &self.ssh_key_id,
        ]
        .into_iter()
        .chain(&self.security_group_ids)
        .map(String::as_str)
    }

    fn invalid_field(&self) -> Option<(&'static str, &'static str)> {
        if !valid_name(&self.name, 20) {
            return Some((
                "Name",
                "must be 5 to 20 lowercase letters, digits, or hyphens, starting and ending with a letter or digit",
            ));
        }
        if !valid_name(&self.node_group_name, 15) {
            return Some((
                "NodeGroupName",
                "must be 5 to 15 lowercase letters, digits, or hyphens, starting and ending with a letter or digit",
            ));
        }
        if self.version.is_empty() {
            return Some(("Version", "must be non-empty"));
        }
        if self.os.is_empty() {
            return Some(("OS", "must be non-empty"));
        }
        if !(1..=10).contains(&self.num_nodes) {
            return Some(("NumNodes", "must be 1 to 10"));
        }
        if self.disk_size <= 0 {
            return Some(("DiskSize", "must be positive"));
        }
        if self.security_group_ids.is_empty() {
            return Some(("SecurityGroupIDs", "must contain at least one ID"));
        }
        if !private_cidr(&self.pod_cidr) {
            return Some(("PodCIDR", "must be a canonical private IPv4 CIDR"));
        }
        for (field, value) in [
            ("VPCID", &self.vpc_id),
            ("SubnetID", &self.subnet_id),
            ("FlavorID", &self.flavor_id),
            ("DiskTypeID", &self.disk_type_id),
            ("SSHKeyID", &self.ssh_key_id),
        ] {
            if !valid_id(value) {
                return Some((
                    field,
                    "must be non-empty and contain only letters, digits, and hyphens",
                ));
            }
        }
        if self.security_group_ids.iter().any(|id| !valid_id(id)) {
            return Some((
                "SecurityGroupIDs",
                "must be non-empty and contain only letters, digits, and hyphens",
            ));
        }
        None
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Orders one private cluster and, by default, waits up to 30 minutes for `ACTIVE`.
    /// An unconfirmed or failed settlement retains validated partial output and dispatch evidence.
    pub async fn create_cluster(
        &self,
        ctx: &RequestContext,
        input: CreateClusterInput,
    ) -> Result<CreateClusterOutput, Failure<CreateClusterOutput>> {
        let base = self.write_base()?;
        if let Some((field, rule)) = input.invalid_field() {
            return Err(not_sent(Error::new(ErrorClass::InvalidInput).with_detail(
                format!("greennode: invalid input: {CREATE}: {field} {rule}"),
            )));
        }
        let body = serde_json::to_vec(&input.body())
            .map_err(|_| not_sent(Error::new(ErrorClass::InvalidInput)))?;
        let mut evidence = Evidence::new();
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        let reply = operation::execute_with_evidence::<CreateReply>(
            runtime,
            base,
            &create_operation(),
            &Options::DEFAULT,
            &[],
            QUERY,
            self.session.bearer_source(),
            bytes::Bytes::from(body),
            &mut evidence,
        )
        .await;
        let mut output = match reply {
            Err(failure) => {
                let (error, _, dispatch) = failure.into_parts();
                if dispatch == WriteDispatch::NotSent {
                    return Err(not_sent(error));
                }
                return Err(partial_failure(
                    unsettled_send(error),
                    CreateClusterOutput::default(),
                    &evidence,
                ));
            }
            Ok((
                status,
                CreateReply {
                    outcome: Outcome::Refused(reason),
                },
            )) => {
                return Err(Failure::new(
                    reply::refusal(status, reason, input.resource_ids(), &evidence),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
            Ok((
                _,
                CreateReply {
                    outcome: Outcome::Unconfirmed(output),
                },
            )) => {
                return Err(partial_failure(unsettled(None), output, &evidence));
            }
            Ok((
                _,
                CreateReply {
                    outcome: Outcome::Unknown,
                },
            )) => {
                return Err(partial_failure(
                    unsettled(None),
                    CreateClusterOutput::default(),
                    &evidence,
                ));
            }
            Ok((
                _,
                CreateReply {
                    outcome: Outcome::Accepted(output),
                },
            )) => output,
        };
        if !input.no_wait {
            if evidence.credentials().reflects(output.id.as_bytes()) {
                return Err(partial_failure(unsettled(None), output, &evidence));
            }
            let result = wait_cluster(ctx, self.session.clock(), async |wait| {
                let status = self.cluster_state(wait, &output.id, &mut evidence).await?;
                observe_status(&mut output, status)
            })
            .await;
            if let Err(error) = result {
                let error = if error.class() == ErrorClass::Failed {
                    *error
                } else {
                    unsettled(Some(*error))
                };
                return Err(partial_failure(error, output, &evidence));
            }
        }
        output.redact(&evidence);
        Ok(output)
    }
}

impl CreateClusterOutput {
    fn redact(&mut self, evidence: &Evidence) {
        for field in [&mut self.id, &mut self.status] {
            if evidence.credentials().reflects(field.as_bytes()) {
                *field = "<redacted>".to_owned();
            }
        }
    }
}

fn observe_status(output: &mut CreateClusterOutput, status: String) -> Result<bool, Box<Error>> {
    output.status = status;
    if output.status == "ERROR" || output.status.contains("FAIL") {
        return Err(Box::new(Error::new(ErrorClass::Failed).with_detail(format!(
            "{CREATE}: inspect inventory and billing before another mutation: greennode: write failed"
        ))));
    }
    Ok(output.status == "ACTIVE")
}

fn partial_failure(
    error: Error,
    output: CreateClusterOutput,
    evidence: &Evidence,
) -> Failure<CreateClusterOutput> {
    let failure = evidence.with_partial(
        Failure::<()>::new(error, None, WriteDispatch::MayHaveBeenSent),
        json!([output.id, output.status]),
        WriteDispatch::MayHaveBeenSent,
    );
    let (error, partial, dispatch) = failure.into_parts();
    let partial = partial
        .and_then(|value| serde_json::from_value::<(String, String)>(value).ok())
        .map(|(id, status)| CreateClusterOutput { id, status });
    Failure::new(error, partial, dispatch)
}

fn unsettled_send(error: Error) -> Error {
    let status = error.api_error().and_then(ApiError::status);
    let cause = if status.is_some_and(|status| (300..400).contains(&status)) {
        None
    } else {
        Some(super::safe_error(CREATE, &error))
    };
    unsettled(cause)
}

fn unsettled(cause: Option<Error>) -> Error {
    let mut message = format!(
        "{CREATE}: inspect inventory and billing before another mutation: greennode: write outcome not confirmed"
    );
    let error = Error::new(ErrorClass::NotSettled);
    let Some(cause) = cause else {
        return error.with_detail(message);
    };
    use std::fmt::Write as _;
    if let Some(api) = cause.api_error() {
        let _ = write!(message, "\n{}: {}", api.operation(), api.message());
        if let Some(status) = api.status() {
            let _ = write!(message, " (status {status})");
        }
    } else {
        let reason = match cause.class() {
            ErrorClass::Cancelled => "context canceled",
            ErrorClass::DeadlineExceeded => "context deadline exceeded",
            _ => "request failed",
        };
        let _ = write!(message, "\n{reason}");
    }
    error.with_detail(message).with_cause(cause)
}

fn create_operation() -> Operation {
    Operation {
        id: CREATE,
        template: "v1/clusters",
        method: Method::POST,
        class: Classification::Write,
        retry: Retry::Once,
        success: ACCEPTED,
        cap: 8 << 20,
        allow_recovery: false,
        allow_empty: false,
        no_output: false,
        keep_status: true,
        overflow_overrides_status: true,
        sensitive: true,
        reflection: crate::wire::product::operation::Reflection::Redact,
        no_redirect: true,
        retry_config: retry::Config::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{Config, ConfigOptions, EndpointOverrides, IamUserAuth, Secret, TestClock};
    use std::time::Duration;

    fn input() -> CreateClusterInput {
        CreateClusterInput::new(
            "cluster".into(),
            "v1".into(),
            "vpc-1".into(),
            "subnet-1".into(),
            "10.20.0.0/16".into(),
            "group".into(),
            1,
            "flavor-1".into(),
            "ubuntu".into(),
            20,
            "disk-1".into(),
            vec!["security-1".into()],
            "key-1".into(),
        )
    }

    #[test]
    fn orders_one_private_cluster_without_auto_renewal() {
        let body = input().body();
        assert!(
            body == json!({
                "name": "cluster", "version": "v1", "releaseChannel": "STABLE",
                "enablePrivateCluster": true, "networkType": "CILIUM_OVERLAY",
                "vpcId": "vpc-1", "cidr": "10.20.0.0/16", "azStrategy": "SINGLE",
                "listSubnetIds": ["subnet-1"], "enabledLoadBalancerPlugin": false,
                "enabledBlockStoreCsiPlugin": false,
                "nodeGroup": {
                    "name": "group", "numNodes": 1, "os": "ubuntu", "flavorId": "flavor-1",
                    "diskSize": 20, "diskType": "disk-1", "enablePrivateNodes": true,
                    "securityGroups": ["security-1"], "sshKeyId": "key-1", "subnetId": "subnet-1",
                    "upgradeConfig": {"strategy": "SURGE"},
                },
            })
        );
        let operation = create_operation();
        assert_eq!(operation.method, Method::POST);
        assert_eq!(operation.template, "v1/clusters");
        assert_eq!(QUERY, &[("autoRenewal", "false"), ("poc", "false")]);
        assert!(matches!(operation.retry, Retry::Once));
        assert!(operation.no_redirect);
        assert!(!operation.allow_recovery);
    }

    #[tokio::test]
    async fn validates_provenance_and_input_before_the_cancelled_send() {
        let options = ConfigOptions::default().with_region("hcm-3".into());
        let iam = options.clone().with_iam_user(IamUserAuth::new(
            "root".into(),
            "user".into(),
            Secret::new("toy-password"),
        ));
        let (ctx, cancel) = RequestContext::new();
        cancel.cancel();
        for config in [
            options.with_static_token(Secret::new("toy-static")),
            iam.clone().with_endpoint_overrides(EndpointOverrides {
                vks: Some("https://vks.console.greennode.ai:443/vks-api/".into()),
                ..Default::default()
            }),
        ] {
            let client = Client::new(&Config::new(config).unwrap());
            let failure = client.create_cluster(&ctx, input()).await.unwrap_err();
            assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
            assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
            assert!(failure.partial().is_none());
        }
        let client = Client::new(&Config::new(iam).unwrap());
        let mut invalid = input();
        invalid.security_group_ids.push("bad/path".into());
        let failure = client.create_cluster(&ctx, invalid).await.unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert!(
            failure
                .error()
                .detail::<String>()
                .unwrap()
                .contains("SecurityGroupIDs")
        );
        let failure = client.create_cluster(&ctx, input()).await.unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::Cancelled);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert!(failure.partial().is_none());
    }

    #[test]
    fn refused_redirect_keeps_the_order_unconfirmed() {
        let error = unsettled_send(super::super::api_error(
            CREATE,
            Some(302),
            "response withheld",
            false,
        ));
        assert_eq!(error.class(), ErrorClass::NotSettled);
        assert!(error.api_error().is_none());
        assert!(!error.detail::<String>().unwrap().contains('\n'));
    }

    #[test]
    fn acceptance_and_partial_output_redact_evidence_before_projection() {
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new("toy-canary"));
        let mut output = CreateClusterOutput {
            id: "toy-canary".into(),
            status: "CREATING".into(),
        };
        let failure = partial_failure(unsettled(None), output.clone(), &evidence);
        let partial = failure.partial().unwrap();
        assert!(partial.id == "<redacted>");
        assert_eq!(partial.status, "CREATING");
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        output.redact(&evidence);
        assert!(output.id == "<redacted>");
        assert_eq!(output.status, "CREATING");
    }

    #[tokio::test]
    async fn settlement_stops_on_deadline_cancellation_active_and_failure() {
        let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let (ctx, cancel) = RequestContext::new();
        let mut output = CreateClusterOutput::default();
        let failure = wait_cluster(&ctx, &clock, async |_| {
            clock.advance(Duration::from_secs(30 * 60));
            observe_status(&mut output, "CREATING".into())
        })
        .await
        .unwrap_err();
        assert_eq!(failure.class(), ErrorClass::DeadlineExceeded);
        assert_eq!(output.status, "CREATING");
        let failure = wait_cluster(&ctx, &clock, async |_| {
            cancel.cancel();
            Ok(true)
        })
        .await
        .unwrap_err();
        assert_eq!(failure.class(), ErrorClass::Cancelled);
        let (ctx, _) = RequestContext::new();
        wait_cluster(&ctx, &clock, async |_| {
            observe_status(&mut output, "ACTIVE".into())
        })
        .await
        .unwrap();
        assert_eq!(output.status, "ACTIVE");
        let error = wait_cluster(&ctx, &clock, async |_| {
            observe_status(&mut output, "PROVISION_FAILED".into())
        })
        .await
        .unwrap_err();
        assert_eq!(error.class(), ErrorClass::Failed);
        assert_eq!(output.status, "PROVISION_FAILED");
    }

    #[test]
    fn validates_cluster_names_and_private_networks() {
        assert!(valid_name("cluster-1", 20));
        assert!(!valid_name("-cluster", 20));
        assert!(!valid_name("Cluster", 20));
        assert!(private_cidr("10.20.0.0/16"));
        assert!(private_cidr("172.16.0.0/12"));
        assert!(!private_cidr("172.16.0.1/12"));
        assert!(!private_cidr("8.8.0.0/16"));
        assert!(!private_cidr("10.0.0.0/08"));
        assert!(!private_cidr("::/0"));
    }
}
