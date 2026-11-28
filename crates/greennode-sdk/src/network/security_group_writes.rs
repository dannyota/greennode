use std::{net::IpAddr, time::Duration};

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::model::wire::model;
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext};

use super::mutations::{
    Mutation, advise, check_active_wait, check_input, create_id, error_description, input_error,
    not_sent, path_id, retain, screen, sent, settlement, tick_active_wait, typed_error,
};
use super::{Client, SecurityGroup, SecurityGroupRule};

const CREATE_GROUP: Mutation = Mutation::new(
    "network.CreateSecurityGroup",
    "v2/{}/secgroups",
    Method::POST,
    &[StatusCode::CREATED],
);
const UPDATE_GROUP: Mutation = Mutation::new(
    "network.UpdateSecurityGroup",
    "v2/{}/secgroups/{}",
    Method::PUT,
    &[StatusCode::OK],
)
.repeatable();
const DELETE_GROUP: Mutation = Mutation::new(
    "network.DeleteSecurityGroup",
    "v2/{}/secgroups/{}",
    Method::DELETE,
    &[StatusCode::NO_CONTENT],
)
.repeatable();
const CREATE_RULE: Mutation = Mutation::new(
    "network.CreateSecurityGroupRule",
    "v2/{}/secgroups/{}/secgroupRules",
    Method::POST,
    &[StatusCode::CREATED],
);
const DELETE_RULE: Mutation = Mutation::new(
    "network.DeleteSecurityGroupRule",
    "v2/{}/secgroups/{}/secgroupRules/{}",
    Method::DELETE,
    &[StatusCode::NO_CONTENT],
)
.repeatable();
const GROUP_RECOVERY: &str =
    "list security groups and match the name exactly before creating it again";
const RULE_RECOVERY: &str = "list the group's rules before creating it again";

/// Names and describes a security group to create.
#[derive(Clone, Debug)]
pub struct CreateSecurityGroupInput {
    /// Display name.
    pub name: String,
    /// Description to store.
    pub description: String,
    /// Return the create response without checking for ACTIVE.
    pub no_wait: bool,
}

impl CreateSecurityGroupInput {
    /// Names the new group.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            no_wait: false,
        }
    }
}

/// The created group.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateSecurityGroupOutput {
    /// The last observed group state.
    pub security_group: SecurityGroup,
}

/// Changes selected fields of an existing security group.
#[derive(Clone, Debug)]
pub struct UpdateSecurityGroupInput {
    /// Group identifier.
    pub security_group_id: String,
    /// Replacement name, if supplied.
    pub name: Option<String>,
    /// Replacement description, if supplied.
    pub description: Option<String>,
}

impl UpdateSecurityGroupInput {
    /// Selects the group to update.
    pub fn new(security_group_id: impl Into<String>) -> Self {
        Self {
            security_group_id: security_group_id.into(),
            name: None,
            description: None,
        }
    }
}

/// The group after an update.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateSecurityGroupOutput {
    /// Confirmed group, or submitted fields if confirmation failed.
    pub security_group: SecurityGroup,
}

super::route_inventory::required_id_input!(
    /// Selects an unused security group for deletion.
    DeleteSecurityGroupInput,
    security_group_id
);

/// Confirms that the service accepted the group deletion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeleteSecurityGroupOutput {}

/// Defines a new security group rule.
#[derive(Clone, Debug)]
pub struct CreateSecurityGroupRuleInput {
    /// Group identifier.
    pub security_group_id: String,
    /// Traffic direction, `ingress` or `egress`.
    pub direction: String,
    /// Protocol to match.
    pub protocol: String,
    /// CIDR prefix to match.
    pub remote_ip_prefix: String,
    /// Address family. An empty value infers it from the prefix.
    pub ether_type: String,
    /// First port in the range.
    pub port_range_min: i64,
    /// Last port. Zero copies the first port.
    pub port_range_max: i64,
    /// Rule description.
    pub description: String,
}

impl CreateSecurityGroupRuleInput {
    /// Sets the group, direction, protocol and CIDR.
    pub fn new(
        security_group_id: impl Into<String>,
        direction: impl Into<String>,
        protocol: impl Into<String>,
        remote_ip_prefix: impl Into<String>,
    ) -> Self {
        Self {
            security_group_id: security_group_id.into(),
            direction: direction.into(),
            protocol: protocol.into(),
            remote_ip_prefix: remote_ip_prefix.into(),
            ether_type: String::new(),
            port_range_min: 0,
            port_range_max: 0,
            description: String::new(),
        }
    }
}

/// The created security group rule.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateSecurityGroupRuleOutput {
    /// Rule identifier and submitted properties.
    pub security_group_rule: SecurityGroupRule,
}

/// Selects a rule for deletion within its parent group.
#[derive(Clone, Debug)]
pub struct DeleteSecurityGroupRuleInput {
    /// Group identifier.
    pub security_group_id: String,
    /// Rule identifier.
    pub security_group_rule_id: String,
}

impl DeleteSecurityGroupRuleInput {
    /// Sets both identifiers required for the deletion.
    pub fn new(
        security_group_id: impl Into<String>,
        security_group_rule_id: impl Into<String>,
    ) -> Self {
        Self {
            security_group_id: security_group_id.into(),
            security_group_rule_id: security_group_rule_id.into(),
        }
    }
}

/// Confirms that the service accepted the rule deletion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeleteSecurityGroupRuleOutput {}

/// The selected group belongs to the project system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemSecurityGroup;

/// The selected group is still in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecurityGroupInUse;

model! {
    struct CreatedGroup {
        numeric_id: i64 => "id", "id";
        uuid: String => "uuid", "uuid";
        name: String => "secgroupName", "secgroupName";
    }
}

model! {
    struct CreatedRule {
        uuid: String => "uuid", "uuid";
        group_uuid: String => "secgroupUuid", "secgroupUuid";
        rule_id: i64 => "ruleId", "ruleId";
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Creates a group and, unless skipped, waits for ACTIVE.
    pub async fn create_security_group(
        &self,
        ctx: &RequestContext,
        input: CreateSecurityGroupInput,
    ) -> Result<CreateSecurityGroupOutput, Failure<CreateSecurityGroupOutput>> {
        let op = &CREATE_GROUP;
        let mut evidence = self.request_evidence(ctx).map_err(not_sent)?;
        let body = json!({"name": input.name, "description": input.description});
        check_input(op.id, &evidence, &[("Name", &input.name)], &[], Some(&body))
            .map_err(not_sent)?;
        let raw = self
            .mutate_vserver(ctx, op, &[], Some(&body), &mut evidence)
            .await
            .map_err(|failure| advise(failure, op.id, GROUP_RECOVERY, &evidence))?;
        let row: CreatedGroup = op
            .decode(&raw, &evidence)
            .map_err(|error| advise(sent(error), op.id, GROUP_RECOVERY, &evidence))?;
        create_id(op, &row.uuid, !input.no_wait, "create response had no id; the group may exist, list security groups and match the name exactly before creating it again", &evidence).map_err(sent)?;
        let mut group = SecurityGroup {
            id: row.uuid,
            name: row.name,
            description: input.description,
            ..Default::default()
        };
        let id = group.id.clone();
        let result = if input.no_wait {
            Ok(())
        } else {
            self.wait_security_group(ctx, &id, &mut group, &mut evidence)
                .await
        };
        let output = retain(group, &evidence)
            .map(|security_group| CreateSecurityGroupOutput { security_group });
        match result {
            Ok(()) => output.ok_or_else(|| sent(op.invalid(&evidence))),
            Err(error) => Err(settlement(
                op.id,
                "security group",
                &id,
                error,
                output,
                &evidence,
            )),
        }
    }

    async fn wait_security_group(
        &self,
        ctx: &RequestContext,
        id: &str,
        latest: &mut SecurityGroup,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let clock = self.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(60));
        loop {
            check_active_wait(&wait, ctx, clock)?;
            match self.guard_security_group(&wait, id, evidence).await {
                Ok(group) => {
                    *latest = group;
                    match latest.status.as_str() {
                        "ACTIVE" => return check_active_wait(&wait, ctx, clock),
                        "ERROR" => return Err(Error::new(ErrorClass::Failed)),
                        _ => {}
                    }
                }
                Err(error) if !error.has_class(ErrorClass::NotFound) => return Err(error),
                Err(_) => {}
            }
            tick_active_wait(&wait, ctx, clock).await?;
        }
    }

    /// Sends one update and confirms the group by reading its detail.
    pub async fn update_security_group(
        &self,
        ctx: &RequestContext,
        input: UpdateSecurityGroupInput,
    ) -> Result<UpdateSecurityGroupOutput, Failure<UpdateSecurityGroupOutput>> {
        let op = &UPDATE_GROUP;
        let id = input.security_group_id.as_str();
        path_id(op.id, "SecurityGroupID", id).map_err(not_sent)?;
        if input.name.is_none() && input.description.is_none() {
            return Err(not_sent(input_error(
                op.id,
                " requires at least one field to change",
            )));
        }
        let mut evidence = self.request_evidence(ctx).map_err(not_sent)?;
        check_input(
            op.id,
            &evidence,
            &[],
            &[id],
            Some(&json!([input.name, input.description])),
        )
        .map_err(not_sent)?;
        let current = self
            .guard_security_group(ctx, id, &mut evidence)
            .await
            .map_err(not_sent)?;
        check_system(op.id, &current, &evidence).map_err(not_sent)?;
        let name = input.name.unwrap_or(current.name);
        let description = input.description.unwrap_or(current.description);
        let body = json!({"name": name, "description": description});
        self.mutate_vserver(ctx, op, &[id], Some(&body), &mut evidence)
            .await?;
        let confirmed = self.guard_security_group(ctx, id, &mut evidence).await;
        match confirmed {
            Ok(group) => retain(group, &evidence)
                .map(|security_group| UpdateSecurityGroupOutput { security_group })
                .ok_or_else(|| sent(op.invalid(&evidence))),
            Err(error) => {
                let fallback = SecurityGroup {
                    id: id.to_owned(),
                    name,
                    description,
                    ..Default::default()
                };
                let partial = retain(fallback, &evidence)
                    .map(|security_group| UpdateSecurityGroupOutput { security_group });
                Err(settlement(
                    op.id,
                    "security group",
                    id,
                    error,
                    partial,
                    &evidence,
                ))
            }
        }
    }

    /// Deletes a non-system security group after checking server membership.
    pub async fn delete_security_group(
        &self,
        ctx: &RequestContext,
        input: DeleteSecurityGroupInput,
    ) -> Result<DeleteSecurityGroupOutput, Failure<DeleteSecurityGroupOutput>> {
        let op = &DELETE_GROUP;
        let id = input.security_group_id.as_str();
        path_id(op.id, "SecurityGroupID", id).map_err(not_sent)?;
        let mut evidence = self.request_evidence(ctx).map_err(not_sent)?;
        check_input(op.id, &evidence, &[], &[id], None).map_err(not_sent)?;
        let group = self
            .guard_security_group(ctx, id, &mut evidence)
            .await
            .map_err(not_sent)?;
        check_system(op.id, &group, &evidence).map_err(not_sent)?;
        let servers = self
            .guard_servers_by_security_group(ctx, id, &mut evidence)
            .await
            .map_err(not_sent)?;
        if !servers.items.is_empty() {
            let message = format!(
                "network: security group in use: {}: security group {id} has {} server(s) attached",
                op.id,
                servers.items.len()
            );
            return Err(not_sent(
                typed_error(ErrorClass::Api, "SecurityGroupInUse", &message, &evidence)
                    .with_detail(SecurityGroupInUse),
            ));
        }
        let result = self
            .mutate_vserver(ctx, op, &[id], None, &mut evidence)
            .await;
        result.map_err(|failure| {
            let (error, partial, dispatch) = failure.into_parts();
            let error = if error.api_error().is_some_and(|api| {
                api.message()
                    .to_ascii_lowercase()
                    .contains("securitygroupinuse")
            }) {
                let message = format!(
                    "network: security group in use: {}",
                    error_description(&error)
                );
                typed_error(ErrorClass::Api, "SecurityGroupInUse", &message, &evidence)
                    .with_detail(SecurityGroupInUse)
                    .with_cause(error)
            } else {
                error
            };
            Failure::new(screen(error, &evidence), partial, dispatch)
        })?;
        Ok(DeleteSecurityGroupOutput {})
    }

    /// Creates a CIDR-based rule for the selected group.
    pub async fn create_security_group_rule(
        &self,
        ctx: &RequestContext,
        input: CreateSecurityGroupRuleInput,
    ) -> Result<CreateSecurityGroupRuleOutput, Failure<CreateSecurityGroupRuleOutput>> {
        let op = &CREATE_RULE;
        let mut evidence = self.request_evidence(ctx).map_err(not_sent)?;
        let id = input.security_group_id.as_str();
        check_input(
            op.id,
            &evidence,
            &[
                ("SecurityGroupID", id),
                ("Direction", &input.direction),
                ("Protocol", &input.protocol),
                ("RemoteIPPrefix", &input.remote_ip_prefix),
            ],
            &[id],
            None,
        )
        .map_err(not_sent)?;
        path_id(op.id, "SecurityGroupID", id).map_err(not_sent)?;
        let (ether_type, port_max) = rule_shape(&input).map_err(not_sent)?;
        let body = json!({"securityGroupId": id, "direction": input.direction, "etherType": ether_type, "protocol": input.protocol,
            "portRangeMin": input.port_range_min, "portRangeMax": port_max, "remoteIpPrefix": input.remote_ip_prefix, "description": input.description});
        let raw = self
            .mutate_vserver(ctx, op, &[id], Some(&body), &mut evidence)
            .await
            .map_err(|failure| advise(failure, op.id, RULE_RECOVERY, &evidence))?;
        let row: CreatedRule = op
            .decode(&raw, &evidence)
            .map_err(|error| advise(sent(error), op.id, RULE_RECOVERY, &evidence))?;
        create_id(op, &row.uuid, false, "create response had no id; the rule may exist, list the group's rules before creating it again", &evidence).map_err(sent)?;
        if evidence.id_reflects(&row.group_uuid) {
            return Err(sent(op.invalid(&evidence)));
        }
        let rule = SecurityGroupRule {
            id: row.uuid,
            security_group_id: input.security_group_id,
            direction: input.direction,
            ether_type: ether_type.to_owned(),
            protocol: input.protocol,
            port_range_min: input.port_range_min,
            port_range_max: port_max,
            remote_ip_prefix: input.remote_ip_prefix,
            description: input.description,
            ..Default::default()
        };
        retain(rule, &evidence)
            .map(|security_group_rule| CreateSecurityGroupRuleOutput {
                security_group_rule,
            })
            .ok_or_else(|| sent(op.invalid(&evidence)))
    }

    /// Deletes a rule only after finding it in the selected group.
    pub async fn delete_security_group_rule(
        &self,
        ctx: &RequestContext,
        input: DeleteSecurityGroupRuleInput,
    ) -> Result<DeleteSecurityGroupRuleOutput, Failure<DeleteSecurityGroupRuleOutput>> {
        let op = &DELETE_RULE;
        let (group, rule) = (
            input.security_group_id.as_str(),
            input.security_group_rule_id.as_str(),
        );
        path_id(op.id, "SecurityGroupID", group).map_err(not_sent)?;
        path_id(op.id, "SecurityGroupRuleID", rule).map_err(not_sent)?;
        let mut evidence = self.request_evidence(ctx).map_err(not_sent)?;
        check_input(op.id, &evidence, &[], &[group, rule], None).map_err(not_sent)?;
        let rules = self
            .guard_security_group_rules(ctx, group, &mut evidence)
            .await
            .map_err(not_sent)?;
        if rules.iter().all(|row| row.id != rule) {
            let message = format!(
                "vngcloud: resource not found: {}: security group rule {rule} not found in security group {group}",
                op.id
            );
            return Err(not_sent(typed_error(
                ErrorClass::NotFound,
                "NotFound",
                &message,
                &evidence,
            )));
        }
        self.mutate_vserver(ctx, op, &[group, rule], None, &mut evidence)
            .await?;
        Ok(DeleteSecurityGroupRuleOutput {})
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn check_system(op: &str, group: &SecurityGroup, evidence: &Evidence) -> Result<(), Error> {
    if group.system || group.is_system {
        let message = format!(
            "network: system security group: {op}: security group {} is a system group",
            group.id
        );
        return Err(
            typed_error(ErrorClass::Api, "SystemSecurityGroup", &message, evidence)
                .with_detail(SystemSecurityGroup),
        );
    }
    Ok(())
}

pub(crate) fn prefix(value: &str) -> Option<(IpAddr, u8)> {
    let (address, bits) = value.split_once('/')?;
    let address = address.parse::<IpAddr>().ok()?;
    if bits.is_empty()
        || !bits.bytes().all(|byte| byte.is_ascii_digit())
        || (bits.len() > 1 && bits.starts_with('0'))
    {
        return None;
    }
    let bits = bits.parse::<u8>().ok()?;
    (bits <= if address.is_ipv4() { 32 } else { 128 }).then_some((address, bits))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn rule_shape(input: &CreateSecurityGroupRuleInput) -> Result<(&'static str, i64), Error> {
    let invalid = |message| input_error(CREATE_RULE.id, message);
    if !matches!(input.direction.as_str(), "ingress" | "egress") {
        return Err(invalid(
            ": Direction must be exactly \"ingress\" or \"egress\"",
        ));
    }
    let (address, _) = prefix(&input.remote_ip_prefix).ok_or_else(|| {
        invalid(": RemoteIPPrefix must be a CIDR prefix such as 203.0.113.0/24 or 2001:db8::/32")
    })?;
    let family = if address.is_ipv4() { "IPv4" } else { "IPv6" };
    if !input.ether_type.is_empty() && input.ether_type != family {
        return Err(invalid(
            ": EtherType does not match RemoteIPPrefix's family",
        ));
    }
    let min = input.port_range_min;
    let max = input.port_range_max;
    if !(0..=65535).contains(&min) {
        return Err(invalid(": PortRangeMin must be 0 to 65535"));
    }
    if !(0..=65535).contains(&max) {
        return Err(invalid(": PortRangeMax must be 0 to 65535"));
    }
    let port_max = if max == 0 { min } else { max };
    if min > port_max {
        return Err(invalid(": PortRangeMin is above PortRangeMax"));
    }
    if input.protocol.eq_ignore_ascii_case("icmp") && (min != 0 || max != 0) {
        return Err(invalid(
            ": Protocol icmp takes no port range; leave PortRangeMin and PortRangeMax at 0, an icmp rule covers all ICMP types and codes",
        ));
    }
    if min == 0
        && ["tcp", "udp"]
            .iter()
            .any(|proto| input.protocol.eq_ignore_ascii_case(proto))
    {
        return Err(invalid(
            ": Protocol requires PortRangeMin at least 1; write \"all ports\" as PortRangeMin 1, PortRangeMax 65535",
        ));
    }
    Ok((family, port_max))
}
