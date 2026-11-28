use std::collections::{BTreeMap, HashSet};

use crate::config::Session;
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    options::{AttemptCheck, Options, PathIds},
};
use crate::wire::strict_json;
use crate::{ApiError, Error, ErrorClass, Failure, List, RequestContext};
use serde_json::value::RawValue;

use super::super::{Client, failure};
use super::models::*;
use super::wire::{RawRead, Read, Reads, Style, field, objects};

const LIST_ACLS: Read = Read::new(
    "network.ListNetworkACLs",
    "v2/{}/network-acl/list",
    Style::Inventory,
);
const GET_ACL: Read = Read::new(
    "network.GetNetworkACL",
    "v2/{}/network-acl/{}",
    Style::Protected,
);
const ACL_STATE: Read = Read::new("network.GetNetworkACL", "", Style::Inventory);
const LIST_GROUPS: Read = Read::new(
    "network.ListSecurityGroups",
    "v2/{}/secgroups",
    Style::Inventory,
);
const GET_GROUP: Read = Read::new(
    "network.GetSecurityGroup",
    "v2/{}/secgroups/{}",
    Style::Protected,
);
const LIST_RULES: Read = Read::new(
    "network.ListSecurityGroupRules",
    "v2/{}/secgroups/{}/secGroupRules",
    Style::Protected,
);
const ALL_RULES: Read = Read::new("network.ListAllSecurityGroupRules", "", Style::Inventory);

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Reads one page of the network ACL inventory.
    pub async fn list_network_acls(
        &self,
        ctx: &RequestContext,
        input: ListNetworkAclsInput,
    ) -> Result<ListNetworkAclsOutput, Failure<ListNetworkAclsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let page = reads
                .page(LIST_ACLS, &input.name, input.page, input.size)
                .await
                .map_err(failure)?;
            reads.sanitize_page(LIST_ACLS, page).map_err(failure)
        })
    }

    /// Reads a network ACL, checking complete inventory if the detail request returns a server error.
    pub async fn get_network_acl(
        &self,
        ctx: &RequestContext,
        input: GetNetworkAclInput,
    ) -> Result<GetNetworkAclOutput, Failure<GetNetworkAclOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = &input.network_acl_id;
            GET_ACL.path("NetworkACLID", id).map_err(failure)?;
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let result = reads.decode(GET_ACL, &[id], |raw| parse_acl(raw, id)).await;
            let acl = match result {
                Ok(Some(acl)) => reads.sanitize(GET_ACL, acl).map_err(failure)?,
                Ok(None) => return Err(failure(reads.invalid(ACL_STATE))),
                Err(error) => {
                    if error
                        .api_error()
                        .and_then(ApiError::observed_status)
                        .is_some_and(|status| (500..600).contains(&status))
                    {
                        let result = reads
                            .complete::<NetworkAcl>(LIST_ACLS, LIST_ACLS, PathIds::Strict, |acl| {
                                &acl.uuid
                            })
                            .await;
                        if result.is_ok_and(|acls| {
                            acls.iter().all(|acl| acl.uuid != *id && acl.id != *id)
                        }) {
                            let message = format!(
                                "vngcloud: resource not found: network.GetNetworkACL: network ACL {id}"
                            );
                            let not_found = Error::new(ErrorClass::NotFound).with_cause(
                                Error::from_api(ApiError::without_fallback(
                                    "",
                                    None,
                                    SafeText::new("NotFound", &[]),
                                    SafeText::new(&message, &[]),
                                    false,
                                )),
                            );
                            return Err(failure(reads.screen(not_found)));
                        }
                    }
                    return Err(failure(reads.screen(error)));
                }
            };
            Ok(GetNetworkAclOutput { acl })
        })
    }

    /// Reads a page of security groups, with an optional name filter.
    pub async fn list_security_groups(
        &self,
        ctx: &RequestContext,
        input: ListSecurityGroupsInput,
    ) -> Result<ListSecurityGroupsOutput, Failure<ListSecurityGroupsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let page = reads
                .page(LIST_GROUPS, &input.name, input.page, input.size)
                .await
                .map_err(failure)?;
            reads.sanitize_page(LIST_GROUPS, page).map_err(failure)
        })
    }

    /// Reads a security group by ID.
    pub async fn get_security_group(
        &self,
        ctx: &RequestContext,
        input: GetSecurityGroupInput,
    ) -> Result<GetSecurityGroupOutput, Failure<GetSecurityGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            GET_GROUP
                .path("SecurityGroupID", &input.security_group_id)
                .map_err(failure)?;
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let row = reads
                .decode(GET_GROUP, &[&input.security_group_id], |raw| {
                    field(&objects(raw)?, "data")
                })
                .await
                .map_err(failure)?;
            let security_group = reads.sanitize(GET_GROUP, row).map_err(failure)?;
            Ok(GetSecurityGroupOutput { security_group })
        })
    }

    /// Lists the rules in the selected security group.
    pub async fn list_security_group_rules(
        &self,
        ctx: &RequestContext,
        input: ListSecurityGroupRulesInput,
    ) -> Result<ListSecurityGroupRulesOutput, Failure<ListSecurityGroupRulesOutput>> {
        crate::wire::product::exit::finish!(async {
            LIST_RULES
                .path("SecurityGroupID", &input.security_group_id)
                .map_err(failure)?;
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let rows = read_rules(&mut reads, &input.security_group_id)
                .await
                .map_err(failure)?;
            let items = reads
                .sanitize(LIST_RULES, rows.unwrap_or_default())
                .map_err(failure)?;
            Ok(List { items })
        })
    }

    /// Lists rules across the project's complete security group inventory.
    pub async fn list_all_security_group_rules(
        &self,
        ctx: &RequestContext,
        _: ListAllSecurityGroupRulesInput,
    ) -> Result<ListAllSecurityGroupRulesOutput, Failure<ListAllSecurityGroupRulesOutput>> {
        crate::wire::product::exit::finish!(async {
            let ctx = ctx.child_with_deadline(
                self.session.clock().monotonic_now() + super::wire::walk_deadline(),
            );
            let mut reads = Reads::new(self, &ctx).map_err(failure)?;
            let groups = reads
                .complete::<SecurityGroup>(LIST_GROUPS, ALL_RULES, PathIds::Strict, |group| {
                    &group.id
                })
                .await
                .map_err(failure)?;
            let mut items = Vec::new();
            let mut retained = 0;
            for group in groups {
                if !reads.safe_identity(&group.id, PathIds::Strict) {
                    return Err(failure(reads.invalid(ALL_RULES)));
                }
                let rows = read_rules(&mut reads, &group.id).await.map_err(failure)?;
                let rows = rows
                    .filter(|rows| unique_type_ids(rows.iter().map(|rule| rule.id.as_str())))
                    .ok_or_else(|| failure(reads.invalid(ALL_RULES)))?;
                reads
                    .append_bounded(ALL_RULES, &mut retained, &mut items, rows)
                    .map_err(failure)?;
            }
            reads.protected_output(ALL_RULES, &items).map_err(failure)?;
            let items = reads.sanitize(ALL_RULES, items).map_err(failure)?;
            Ok(List { items })
        })
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
async fn read_rules(
    reads: &mut Reads<'_>,
    id: &str,
) -> Result<Option<Vec<SecurityGroupRule>>, Error> {
    reads
        .decode(LIST_RULES, &[id], |raw| {
            let mut rows: Option<Vec<SecurityGroupRule>> = field(&objects(raw)?, "data")?;
            if let Some(rows) = &mut rows {
                for rule in rows {
                    rule.security_group_id = id.to_owned();
                }
            }
            Ok(rows)
        })
        .await
}

pub(super) fn parse_acl(raw: &str, id: &str) -> Result<Option<NetworkAcl>, ()> {
    let Ok(envelope) = objects(raw) else {
        return Ok(None);
    };
    let Ok(acl): Result<NetworkAcl, ()> = field(&envelope, "data") else {
        return Ok(None);
    };
    let Some(data) = envelope.get("data") else {
        return Ok(None);
    };
    let fields = objects(data.get())?;
    let required = [
        "uuid",
        "status",
        "defaultAcl",
        "interfaceNetworkUuid",
        "subnetAssociationList",
        "aclPolicyRules",
    ];
    if required
        .iter()
        .any(|&key| fields.get(key).is_none_or(|value| value.get() == "null"))
        || acl.uuid != id
        || acl.status.is_empty()
        || acl.vpc_id.is_empty()
        || !unique_type_ids(acl.subnet_ids.iter().map(String::as_str))
    {
        return Ok(None);
    }
    Ok(Some(acl))
}

fn unique_type_ids<'a>(mut ids: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = HashSet::new();
    ids.all(|id| PathIds::Strict.allows(id) && seen.insert(id))
}

const EXPOSURE: &str = "floatingip.Exposure";

pub(crate) struct SecurityGroupExposure {
    pub(crate) group: Box<RawValue>,
    pub(crate) rules: Vec<Box<RawValue>>,
}

pub(crate) async fn read_security_group_exposure(
    session: &Session,
    ctx: &RequestContext,
    id: &str,
    evidence: &mut Evidence,
    attempt_check: Option<AttemptCheck<'_>>,
    max_rules: usize,
) -> Result<SecurityGroupExposure, Box<Error>> {
    let mut read = RawRead {
        session,
        ctx,
        evidence,
    };
    let options = Options {
        attempt_check,
        ..Options::DEFAULT
    };
    let group = read
        .request(GET_GROUP.with_operation(EXPOSURE), &[id], &[], &options)
        .await
        .map_err(Box::new)?;
    let group_doc =
        checked_data(&group, read.evidence).map_err(|()| exposure_invalid(read.evidence))?;
    let fields = objects(group_doc.get()).map_err(|()| exposure_invalid(read.evidence))?;
    let group_id = exact_raw(&fields, "id")
        .and_then(|raw| serde_json::from_str::<String>(raw.get()).map_err(|_| ()))
        .map_err(|()| exposure_invalid(read.evidence))?;
    if group_id != id || !PathIds::Strict.allows(&group_id) || read.evidence.id_reflects(&group_id)
    {
        return Err(exposure_invalid(read.evidence));
    }
    let rules = read
        .request(LIST_RULES.with_operation(EXPOSURE), &[id], &[], &options)
        .await
        .map_err(Box::new)?;
    let fields = objects(&rules).map_err(|()| exposure_invalid(read.evidence))?;
    let rules_doc = exact_raw(&fields, "data").map_err(|()| exposure_invalid(read.evidence))?;
    use serde::de::DeserializeSeed;
    let mut decoder = serde_json::Deserializer::from_str(rules_doc.get());
    let decoded_rules = BoundedRules(max_rules)
        .deserialize(&mut decoder)
        .map_err(|_| exposure_invalid(read.evidence))?;
    if exposure_reflects(&rules, read.evidence)
        || decoded_rules.iter().any(|row| !row.get().starts_with('{'))
    {
        return Err(exposure_invalid(read.evidence));
    }
    Ok(SecurityGroupExposure {
        group: group_doc.to_owned(),
        rules: decoded_rules,
    })
}

struct BoundedRules(usize);

impl<'de> serde::de::DeserializeSeed<'de> for BoundedRules {
    type Value = Vec<Box<RawValue>>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct Visitor(usize);
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Vec<Box<RawValue>>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a bounded array of rules")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut rules = Vec::new();
                while let Some(rule) = sequence.next_element::<&RawValue>()? {
                    if rules.len() == self.0 {
                        return Err(serde::de::Error::custom("too many rules"));
                    }
                    rules.push(rule.to_owned());
                }
                Ok(rules)
            }
        }
        deserializer.deserialize_seq(Visitor(self.0))
    }
}

pub(crate) fn exposure_reflects(raw: &str, evidence: &Evidence) -> bool {
    !evidence.go_peer_body_screen(raw.as_bytes()).is_clean()
}

fn checked_data<'a>(text: &'a str, evidence: &Evidence) -> Result<&'a RawValue, ()> {
    if exposure_reflects(text, evidence) {
        return Err(());
    }
    let document = objects(text)?;
    exact_raw(&document, "data")
}

fn exact_raw<'a>(fields: &BTreeMap<String, &'a RawValue>, field: &str) -> Result<&'a RawValue, ()> {
    if fields
        .keys()
        .any(|key| key != field && strict_json::field_matches(key, field))
    {
        return Err(());
    }
    fields.get(field).copied().ok_or(())
}

pub(crate) fn exposure_invalid(evidence: &Evidence) -> Box<Error> {
    Box::new(
        Error::from_api(ApiError::without_fallback(
            EXPOSURE,
            None,
            SafeText::new("InvalidResponse", &[]),
            SafeText::new("invalid floating IP response; body withheld", &[]),
            false,
        ))
        .screen_api(&|text| evidence.text_reflects(text)),
    )
}

#[cfg(test)]
mod bounded_rules_tests {
    use super::*;
    use serde::de::DeserializeSeed;

    #[test]
    fn refuses_at_rule_cap_before_decoding_malformed_tail() {
        let input = format!("[{},{{", vec!["{}"; 4097].join(","));
        let mut decoder = serde_json::Deserializer::from_str(&input);
        let error = BoundedRules(4096).deserialize(&mut decoder).unwrap_err();
        assert!(error.to_string().contains("too many rules"));
    }
}
