use std::collections::HashMap;

use http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::network::{NetworkAcl, NetworkAclRule};
use crate::{ErrorClass, Failure, RequestContext};

use super::super::super::Client;
use super::associations::AclChange;
use super::models::*;
use super::wire::{NetworkWriteError, Request, Write, input, not_sent, prefix, required};

const ADD: Request = Request::new(
    "network.AddNetworkACLRule",
    "v2/{}/network-acl/{}/rules",
    &Method::PUT,
    &[StatusCode::OK],
)
.once();
const REMOVE: Request = Request::new(
    "network.RemoveNetworkACLRule",
    "v2/{}/network-acl/{}/rules",
    &Method::PUT,
    &[StatusCode::OK],
)
.once();

#[derive(PartialEq, Eq, Hash)]
struct RuleKey<'a> {
    direction: &'a str,
    priority: i64,
    protocol: &'a str,
    port: &'a str,
    cidr: &'a str,
    action: &'a str,
    system: bool,
}

fn rule_counts(rules: &[NetworkAclRule], compare_system: bool) -> HashMap<RuleKey<'_>, usize> {
    let mut counts = HashMap::new();
    for rule in rules {
        let key = RuleKey {
            direction: &rule.direction,
            priority: rule.priority,
            protocol: &rule.protocol,
            port: &rule.port,
            cidr: &rule.cidr,
            action: &rule.action,
            system: compare_system && rule.system,
        };
        *counts.entry(key).or_default() += 1;
    }
    counts
}

fn canonical_cidr(value: &str) -> String {
    prefix(value).map_or_else(|| value.to_owned(), |(ip, bits, _)| format!("{ip}/{bits}"))
}

fn request_rule(acl_id: &str, rule: &NetworkAclRule) -> Value {
    json!({"type": rule.direction, "seqNumber": rule.priority, "protocol": rule.protocol,
        "port": rule.port, "source": rule.cidr, "action": rule.action, "system": rule.system,
        "interfaceAclPolicyUuid": acl_id})
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
fn new_rule(
    input: &AddNetworkAclRuleInput,
) -> Result<NetworkAclRule, Failure<AddNetworkAclRuleOutput>> {
    let op = ADD.id;
    if !(1..=1999).contains(&input.priority) {
        return Err(super::wire::input(op, "requires Priority to be 1 to 1999"));
    }
    let cidr = match prefix(&input.cidr) {
        Some((ip, bits, true)) => format!("{ip}/{bits}"),
        Some(_) => {
            return Err(super::wire::input(
                op,
                "requires CIDR to have no host bits set",
            ));
        }
        None => return Err(super::wire::input(op, "requires CIDR to be a CIDR prefix")),
    };
    let protocol = match input.protocol.to_ascii_lowercase().as_str() {
        "any" => "ANY",
        "tcp" => "tcp",
        "udp" => "udp",
        "icmp" => "icmp",
        _ => {
            return Err(super::wire::input(
                op,
                "requires Protocol to be one of ANY, tcp, udp, icmp (case-insensitive)",
            ));
        }
    };
    let (min, max) = (input.port_range_min, input.port_range_max);
    if !(0..=65535).contains(&min) {
        return Err(super::wire::input(
            op,
            "requires PortRangeMin to be 0 to 65535",
        ));
    }
    if !(0..=65535).contains(&max) {
        return Err(super::wire::input(
            op,
            "requires PortRangeMax to be 0 to 65535",
        ));
    }
    let max = if max == 0 { min } else { max };
    if min > max {
        return Err(super::wire::input(
            op,
            "requires PortRangeMin at or below PortRangeMax",
        ));
    }
    match protocol {
        "ANY" if (min, max) != (0, 65535) => {
            return Err(super::wire::input(
                op,
                "requires PortRangeMin 0 and PortRangeMax 65535 for protocol ANY",
            ));
        }
        "icmp" if min != 0 || !matches!(max, 0 | 65535) => {
            return Err(super::wire::input(
                op,
                "requires PortRangeMin 0 and PortRangeMax 65535, or both 0, for protocol icmp",
            ));
        }
        "tcp" | "udp" if (min, max) == (0, 0) => {
            return Err(super::wire::input(
                op,
                &format!(
                    "{protocol} needs an explicit PortRangeMin (and, for a range, PortRangeMax); leaving both at 0 may mean every port"
                ),
            ));
        }
        _ => {}
    }
    let port = if min == max {
        min.to_string()
    } else {
        format!("{min}-{max}")
    };
    Ok(NetworkAclRule {
        direction: input.direction.clone(),
        priority: input.priority,
        protocol: protocol.into(),
        port,
        cidr,
        action: input.action.clone(),
        ..Default::default()
    })
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Adds a rule while preserving the ACL's complete existing ruleset.
    pub async fn add_network_acl_rule(
        &self,
        ctx: &RequestContext,
        settings: AddNetworkAclRuleInput,
    ) -> Result<AddNetworkAclRuleOutput, Failure<AddNetworkAclRuleOutput>> {
        let mut write = Write::new(self, ctx)?;
        let op = ADD.id;
        let id = &settings.network_acl_id;
        required(op, "NetworkACLID", !id.is_empty())?;
        required(op, "Direction", !settings.direction.is_empty())?;
        required(op, "Priority", settings.priority != 0)?;
        write.check(
            op,
            &[
                ("Direction", &settings.direction),
                ("Protocol", &settings.protocol),
                ("CIDR", &settings.cidr),
                ("Action", &settings.action),
            ],
            &[("NetworkACLID", id)],
        )?;
        let rule = new_rule(&settings)?;
        let acl = write.active_acl(op, id).await.map_err(not_sent)?;
        if let Some(existing) = acl.rules.iter().find(|row| {
            row.direction.eq_ignore_ascii_case(&rule.direction) && row.priority == rule.priority
        }) {
            if existing.protocol == rule.protocol
                && existing.port == rule.port
                && canonical_cidr(&existing.cidr) == rule.cidr
                && existing.action.eq_ignore_ascii_case(&rule.action)
            {
                return write.unchanged(
                    op,
                    AddNetworkAclRuleOutput {
                        acl,
                        changed: false,
                    },
                );
            }
            return Err(not_sent(write.local(ErrorClass::InvalidInput, &format!("vngcloud: invalid input: {op}: network ACL {id} already has a {} rule at priority {} with different fields; remove it first", rule.direction, rule.priority))));
        }
        let mut rules = acl.rules.clone();
        rules.push(rule);
        let output = |acl| AddNetworkAclRuleOutput { acl, changed: true };
        let result = write
            .replace_acl_rules(ADD, id, acl, rules, settings.no_wait)
            .await;
        write.map_result(op, result, output)
    }

    /// Removes a rule by direction and priority, preserving service-managed rules.
    pub async fn remove_network_acl_rule(
        &self,
        ctx: &RequestContext,
        settings: RemoveNetworkAclRuleInput,
    ) -> Result<RemoveNetworkAclRuleOutput, Failure<RemoveNetworkAclRuleOutput>> {
        let mut write = Write::new(self, ctx)?;
        let op = REMOVE.id;
        let id = &settings.network_acl_id;
        let (direction, priority) = (&settings.direction, settings.priority);
        write.check(op, &[("Direction", direction)], &[("NetworkACLID", id)])?;
        if priority < 0 {
            return Err(input(op, "requires Priority to be zero or greater"));
        }
        let acl = write.active_acl(op, id).await.map_err(not_sent)?;
        let Some((index, row)) = acl.rules.iter().enumerate().find(|(_, row)| {
            row.direction.eq_ignore_ascii_case(direction) && row.priority == priority
        }) else {
            return Err(not_sent(write.local(ErrorClass::NotFound, &format!("vngcloud: resource not found: {op}: network ACL {id} has no {direction} rule at priority {priority}"))));
        };
        if row.system || row.priority >= 2000 {
            return Err(not_sent(write.guard_error(NetworkWriteError::DefaultResource, &format!("{op}: network ACL {id}'s {direction} rule at priority {priority} is a default rule and cannot be removed"))));
        }
        let rules = acl
            .rules
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, rule)| rule.clone())
            .collect();
        let output = |acl| RemoveNetworkAclRuleOutput { acl, changed: true };
        let result = write
            .replace_acl_rules(REMOVE, id, acl, rules, settings.no_wait)
            .await;
        write.map_result(op, result, output)
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Write<'_> {
    async fn replace_acl_rules(
        &mut self,
        request: Request,
        id: &str,
        acl: NetworkAcl,
        rules: Vec<NetworkAclRule>,
        no_wait: bool,
    ) -> Result<NetworkAcl, Failure<NetworkAcl>> {
        let entries: Vec<_> = rules.iter().map(|rule| request_rule(id, rule)).collect();
        let same = |row: &NetworkAcl| rule_counts(&row.rules, false) == rule_counts(&rules, false);
        let unchanged = |before: &NetworkAcl, after: &NetworkAcl| {
            rule_counts(&before.rules, true) == rule_counts(&after.rules, true)
        };
        let fallback = |mut acl: NetworkAcl| {
            acl.rules = rules
                .iter()
                .cloned()
                .map(|mut rule| {
                    rule.uuid.clear();
                    rule.system = false;
                    rule
                })
                .collect();
            acl
        };
        self.replace_acl(
            request,
            id,
            acl,
            json!({"aclId": id, "detailAclRuleList": entries}),
            no_wait,
            AclChange {
                field: "rules",
                unchanged,
                matches: same,
                fallback,
            },
        )
        .await
    }
}
