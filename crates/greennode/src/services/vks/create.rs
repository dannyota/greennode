use clap::ArgMatches;
use greennode_sdk::vks::{Client, CreateClusterInput};
use greennode_sdk::{Config, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

const NAME: Field = flag("name", "string", true);
const VERSION: Field = flag("version", "string", true);
const VPC_ID: Field = flag("vpc-id", "string", true);
const SUBNET_ID: Field = flag("subnet-id", "string", true);
const POD_CIDR: Field = flag("pod-cidr", "string", true);
const NODE_GROUP_NAME: Field = flag("node-group-name", "string", true);
const NUM_NODES: Field = flag("num-nodes", "int", true);
const FLAVOR_ID: Field = flag("flavor-id", "string", true);
const OS: Field = flag("os", "string", true);
const DISK_SIZE: Field = flag("disk-size", "int", true);
const DISK_TYPE_ID: Field = flag("disk-type-id", "string", true);
const SECURITY_GROUP_ID: Field = flag("security-group-id", "[]string", true);
const SSH_KEY_ID: Field = flag("ssh-key-id", "string", true);
const NO_WAIT: Field = flag("no-wait", "bool", false);

macro_rules! required {
    ($flag:ident, $name:literal, $field:ident, $ty:ty, $from_json:expr, $present:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = ($from_json)(&value).ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = matches.get_one::<$ty>($flag.name).cloned().ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Nonzero($present),
        }
    };
}

macro_rules! string {
    ($flag:ident, $name:literal, $field:ident) => {
        required!(
            $flag,
            $name,
            $field,
            String,
            |value: &serde_json::Value| value.as_str().map(str::to_owned),
            |input| !input.$field.is_empty()
        )
    };
}

macro_rules! integer {
    ($flag:ident, $name:literal, $field:ident) => {
        required!(
            $flag,
            $name,
            $field,
            i64,
            serde_json::Value::as_i64,
            |input| input.$field != 0
        )
    };
}

const INPUT: InputDescriptor<CreateClusterInput> = InputDescriptor {
    defaults: || CreateClusterInput {
        name: String::new(),
        version: String::new(),
        vpc_id: String::new(),
        subnet_id: String::new(),
        pod_cidr: String::new(),
        node_group_name: String::new(),
        num_nodes: 0,
        flavor_id: String::new(),
        os: String::new(),
        disk_size: 0,
        disk_type_id: String::new(),
        security_group_ids: Vec::new(),
        ssh_key_id: String::new(),
        no_wait: false,
    },
    fields: &[
        string!(NAME, "Name", name),
        string!(VERSION, "Version", version),
        string!(VPC_ID, "VPCID", vpc_id),
        string!(SUBNET_ID, "SubnetID", subnet_id),
        string!(POD_CIDR, "PodCIDR", pod_cidr),
        string!(NODE_GROUP_NAME, "NodeGroupName", node_group_name),
        integer!(NUM_NODES, "NumNodes", num_nodes),
        string!(FLAVOR_ID, "FlavorID", flavor_id),
        string!(OS, "OS", os),
        integer!(DISK_SIZE, "DiskSize", disk_size),
        string!(DISK_TYPE_ID, "DiskTypeID", disk_type_id),
        InputField {
            json_name: "SecurityGroupIDs",
            flag: Some(SECURITY_GROUP_ID),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.security_group_ids = value
                    .as_array()
                    .ok_or(())?
                    .iter()
                    .map(|item| match item {
                        serde_json::Value::Null => Ok(String::new()),
                        item => item.as_str().map(str::to_owned).ok_or(()),
                    })
                    .collect::<Result<_, _>>()?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.security_group_ids = matches
                    .get_many::<String>(SECURITY_GROUP_ID.name)
                    .ok_or(())?
                    .cloned()
                    .collect();
                Ok(())
            }),
            requirement: Requirement::Presence,
        },
        string!(SSH_KEY_ID, "SSHKeyID", ssh_key_id),
        InputField {
            json_name: "NoWait",
            flag: Some(NO_WAIT),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.no_wait = value.as_bool().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.no_wait = *matches.get_one::<bool>(NO_WAIT.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(super::check_input_source),
    guard: Some(require_consent),
};

pub(super) const CREATE_CLUSTER: Operation = Operation {
    name: "create-cluster",
    sdk_operation: "vks.CreateCluster",
    short_help: "Create a private Kubernetes cluster",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[
        NAME,
        VERSION,
        VPC_ID,
        SUBNET_ID,
        POD_CIDR,
        NODE_GROUP_NAME,
        NUM_NODES,
        FLAVOR_ID,
        OS,
        DISK_SIZE,
        DISK_TYPE_ID,
        SECURITY_GROUP_ID,
        SSH_KEY_ID,
        NO_WAIT,
    ],
    json_fields: &[],
    extra_flags: &[],
    notes: &[
        "Requires --yes. There is no quote API. Worker servers and disks, the private cluster's \
        managed load balancer, and four service endpoints incur charges. The order is prepaid for \
        one month from credits; a one-node private cluster debited about 2,137,000 VND at create \
        in one recorded run, and deleting it refunded the unused value. That figure is an example, \
        not an estimate; cost depends on the node group, flavor, and disks. Auto-renew is off. \
        ACTIVE means provisioned, not workload ready. Cluster names must have 5 to 20 characters; \
        node group names must have 5 to 15. Names use lowercase letters, digits, and hyphens, \
        starting and ending with a letter or digit. The disk type, flavor, and subnet must be in \
        the same zone. Use `network get-subnet` for ZoneID and \
        `volume get-default-volume-type --zone-id <that zone>` for the disk type.",
    ],
    handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
        Box::pin(dispatch::management(
            invocation,
            operation,
            matches,
            &INPUT,
            create_cluster,
        ))
    })),
};

fn require_consent(_: &CreateClusterInput, matches: &ArgMatches) -> Result<(), InputError> {
    if !matches.get_one::<bool>("yes").copied().unwrap_or(false) {
        return Err(InputError(
            "create-cluster incurs charges; pass --yes to confirm",
        ));
    }
    Ok(())
}

fn create_cluster<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a CreateClusterInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config).create_cluster(ctx, input.clone()).await,
            std::convert::identity,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Invocation, command_with_registry, dispatch::OperationFailure, render};
    use crate::services::Registry;
    use greennode_sdk::vks::CreateClusterOutput;
    use greennode_sdk::{
        ConfigOptions, ConfigSource, ErrorClass, Secret, SystemClock, WriteDispatch, testing,
    };
    use std::ffi::{OsStr, OsString};

    struct Source;

    impl ConfigSource for Source {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }
    }

    fn fixture<'a>(
        _: &'a RequestContext,
        _: &'a Config,
        input: &'a CreateClusterInput,
    ) -> FutureResult<'a, ExecutionResult> {
        Box::pin(async move {
            assert_eq!(input.name, "cluster");
            assert_eq!(input.security_group_ids, ["sg-2", "sg-3"]);
            let output = CreateClusterOutput {
                id: "cluster-1".into(),
                status: if input.no_wait { "CREATING" } else { "ACTIVE" }.into(),
            };
            let projected = render::project(&output);
            if input.no_wait {
                Err(OperationFailure {
                    error: testing::identity(ErrorClass::NotSettled),
                    partial: Some(projected.unwrap()),
                    dispatch: WriteDispatch::MayHaveBeenSent,
                    query_partial: false,
                }
                .into())
            } else {
                Ok(Some(projected))
            }
        })
    }

    #[test]
    fn an_explicit_empty_security_group_list_reaches_sdk_validation() {
        let registry = Registry::new(&[super::super::SERVICE]).unwrap();
        let matches = command_with_registry(&registry)
            .try_get_matches_from(["greennode", "vks", "create-cluster", "--yes"])
            .unwrap();
        let (_, _, matches) = registry.select(&matches).unwrap();
        let json = r#"{"Name":"cluster","Version":"v1","VPCID":"vpc-1","SubnetID":"subnet-1","PodCIDR":"10.20.0.0/16","NodeGroupName":"group","NumNodes":1,"FlavorID":"flavor-1","OS":"ubuntu","DiskSize":20,"DiskTypeID":"disk-1","SecurityGroupIDs":[],"SSHKeyID":"key-1"}"#;
        let parsed = crate::cli::input::merge(&INPUT, matches, Some(json), None).unwrap();
        assert!(crate::cli::input::check_required(&INPUT, &parsed).is_ok());
    }

    #[tokio::test]
    async fn dispatch_renders_go_create_goldens_and_ignores_queries_on_partial() {
        const JSON: &str = r#"{"Name":"cluster","Version":"v1","VPCID":"vpc-1","SubnetID":"subnet-1","PodCIDR":"10.20.0.0/16","NodeGroupName":"group","NumNodes":1,"FlavorID":"flavor-1","OS":"ubuntu","DiskSize":20,"DiskTypeID":"disk-1","SecurityGroupIDs":["old"],"SSHKeyID":"key-1"}"#;
        let registry = Registry::new(&[super::super::SERVICE]).unwrap();
        for (format, is_partial, golden) in [
            (
                "json",
                false,
                include_str!("../../../tests/fixtures/vks/create-cluster.json.golden"),
            ),
            (
                "table",
                false,
                include_str!("../../../tests/fixtures/vks/create-cluster.table.golden"),
            ),
            (
                "text",
                false,
                include_str!("../../../tests/fixtures/vks/create-cluster.text.golden"),
            ),
            (
                "json",
                true,
                include_str!("../../../tests/fixtures/vks/create-cluster-partial.json.golden"),
            ),
            (
                "table",
                true,
                include_str!("../../../tests/fixtures/vks/create-cluster-partial.table.golden"),
            ),
            (
                "text",
                true,
                include_str!("../../../tests/fixtures/vks/create-cluster-partial.text.golden"),
            ),
        ] {
            let mut args = vec![
                "greennode",
                "vks",
                "create-cluster",
                "--yes",
                "--output",
                format,
                "--cli-input-json",
                JSON,
                "--security-group-id",
                "sg-2",
                "--security-group-id",
                "sg-3",
            ];
            if is_partial {
                args.extend(["--no-wait", "--query", "ID"]);
            }
            let parsed = command_with_registry(&registry)
                .try_get_matches_from(args)
                .unwrap();
            let (selected, _, matches) = registry.select(&parsed).unwrap();
            let (context, _) = RequestContext::new();
            let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
            let mut invocation = Invocation {
                args: vec![],
                source: &Source,
                config_options: ConfigOptions::default()
                    .with_region("hcm-3".into())
                    .with_static_token(Secret::new("toy-static")),
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
                stdin_terminal: false,
                stdout_terminal: false,
                stderr_terminal: false,
                clock: &SystemClock,
                context: &context,
            };
            let exit =
                dispatch::management(&mut invocation, selected, matches, &INPUT, fixture).await;
            assert_eq!(exit, u8::from(is_partial));
            assert_eq!(stdout, golden.as_bytes());
            if is_partial {
                let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
                assert_eq!(envelope["error"]["code"], "NotSettled");
            } else {
                assert!(stderr.is_empty());
            }
        }
    }
}
