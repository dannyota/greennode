use http::Method;
use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::wire::{
    product::options::PathIds,
    strict_json::{Shape, ShapeField},
};
use crate::{Error, Failure, RequestContext, WriteDispatch};

use super::child_writes::{BOUND, ChildTarget, ChildWriter, WriteResult, encode_body, not_sent};
use super::policy_output::PolicyProjection;
use super::wire::{DecodeError, Read, invalid_input, validate_ids};
use super::{Client, L7Rule, Policy, guard};
use guard::shape;

const CREATE: &str = "loadbalancer.CreatePolicy";
const UPDATE: &str = "loadbalancer.UpdatePolicy";
const LIST_POLICIES: Read = Read {
    id: "loadbalancer.ListPolicies",
    route: "v2/{}/loadBalancers/{}/listeners/{}/l7policies",
};
const GET_POLICY: Read = Read {
    id: "loadbalancer.GetPolicy",
    route: "v2/{}/loadBalancers/{}/listeners/{}/l7policies/{}",
};
const RULE: Shape<'_> = shape!("uuid", "compareType", "ruleValue", "ruleType", "provisioningStatus", "operatingStatus";);
const POLICY: Shape<'_> = shape!("uuid", "name", "description", "redirectPoolId", "redirectPoolName",
    "action", "redirectUrl", "redirectHttpCode", "keepQueryString", "position", "displayStatus",
    "createdAt", "updatedAt", "progressStatus"; "l7Rules" => Shape::Array(&RULE));
const TO_POOL: &str = "REDIRECT_TO_POOL";
const TO_URL: &str = "REDIRECT_TO_URL";

/// Supplies one match rule for a listener policy.
#[derive(Clone, Default, PartialEq, Serialize)]
pub struct PolicyRuleInput {
    /// Provider rule type.
    pub r#type: String,
    /// How the rule compares a request to its value.
    pub compare_type: String,
    /// The value to compare against the request.
    pub value: String,
}

crate::error::debug_record!(PolicyRuleInput {
    r#type,
    compare_type,
    value
});

/// Creates a layer 7 policy on a regional listener.
#[derive(Clone, Default, Serialize)]
pub struct CreatePolicyInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Parent listener identifier.
    #[serde(skip)]
    pub listener_id: String,
    /// Name for the new policy.
    pub name: String,
    /// Action applied to matching requests.
    pub action: String,
    /// Target pool identifier.
    pub redirect_pool_id: String,
    /// Target URL.
    pub redirect_url: String,
    /// HTTP redirect status.
    pub redirect_http_code: i64,
    /// Whether to keep the original query string.
    pub keep_query_string: bool,
    /// Match rules to send to the provider.
    pub rules: Vec<PolicyRuleInput>,
    /// Returns after the provider accepts the write.
    #[serde(skip)]
    pub no_wait: bool,
}

crate::error::debug_record!(CreatePolicyInput {
    load_balancer_id,
    listener_id,
    name,
    action,
    redirect_pool_id,
    redirect_url,
    redirect_http_code,
    keep_query_string,
    rules,
    no_wait
});

/// Holds the policy returned by the create or its confirmation.
#[derive(Clone, PartialEq)]
pub struct CreatePolicyOutput {
    /// The observed or submitted policy.
    pub policy: Policy,
    projection: Option<PolicyProjection>,
}

crate::wire::product::output::public_fields!(CreatePolicyOutput { policy; projection });

impl Serialize for CreatePolicyOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut result = serializer.serialize_struct("CreatePolicyOutput", 1)?;
        if let Some(projection) = &self.projection {
            result.serialize_field("Policy", projection)?;
        } else {
            result.serialize_field("Policy", &self.policy)?;
        }
        result.end()
    }
}

/// Updates a layer 7 policy on a regional listener.
#[derive(Clone, Default, Serialize)]
pub struct UpdatePolicyInput {
    /// Parent load balancer identifier.
    #[serde(skip)]
    pub load_balancer_id: String,
    /// Parent listener identifier.
    #[serde(skip)]
    pub listener_id: String,
    /// Target policy identifier.
    #[serde(skip)]
    pub policy_id: String,
    /// Action applied to matching requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Target pool identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_pool_id: Option<String>,
    /// Target URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_url: Option<String>,
    /// HTTP redirect status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_http_code: Option<i64>,
    /// Whether to keep the original query string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_query_string: Option<bool>,
    /// Match rules to send to the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<PolicyRuleInput>>,
    /// Returns after the provider accepts the write.
    #[serde(skip)]
    pub no_wait: bool,
}

crate::error::debug_record!(UpdatePolicyInput {
    load_balancer_id,
    listener_id,
    policy_id,
    action,
    redirect_pool_id,
    redirect_url,
    redirect_http_code,
    keep_query_string,
    rules,
    no_wait
});

/// Holds the policy returned by the update or its confirmation.
#[derive(Clone, PartialEq)]
pub struct UpdatePolicyOutput {
    /// The observed or submitted policy.
    pub policy: Policy,
    projection: Option<PolicyProjection>,
}

crate::wire::product::output::public_fields!(UpdatePolicyOutput { policy; projection });

impl Serialize for UpdatePolicyOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut result = serializer.serialize_struct("UpdatePolicyOutput", 1)?;
        if let Some(projection) = &self.projection {
            result.serialize_field("Policy", projection)?;
        } else {
            result.serialize_field("Policy", &self.policy)?;
        }
        result.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyBody<'a> {
    #[serde(skip_serializing_if = "str::is_empty")]
    name: &'a str,
    action: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    redirect_pool_id: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    redirect_url: &'a str,
    #[serde(skip_serializing_if = "zero")]
    redirect_http_code: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_query_string: Option<bool>,
    rules: Vec<RuleBody<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleBody<'a> {
    rule_type: &'a str,
    compare_type: &'a str,
    rule_value: &'a str,
}

struct Redirect<'a> {
    pool: &'a str,
    url: &'a str,
    status: i64,
    keep_query: bool,
}

impl<'a> Redirect<'a> {
    fn check(&self, operation: &str, action: &str) -> Result<(), Box<Error>> {
        let reason = match action {
            TO_POOL if self.pool.is_empty() => {
                "RedirectPoolID is required when Action is REDIRECT_TO_POOL"
            }
            TO_POOL if !self.url.is_empty() || self.status != 0 || self.keep_query => {
                "RedirectURL, RedirectHTTPCode, and KeepQueryString must be unset when Action is REDIRECT_TO_POOL"
            }
            TO_URL if self.url.is_empty() => {
                "RedirectURL is required when Action is REDIRECT_TO_URL"
            }
            TO_URL if !self.pool.is_empty() => {
                "RedirectPoolID must be empty when Action is REDIRECT_TO_URL"
            }
            _ => return Ok(()),
        };
        Err(invalid_input(&format!("{operation}: {reason}")))
    }

    fn into_body(self, name: &'a str, action: &'a str, rules: Vec<RuleBody<'a>>) -> PolicyBody<'a> {
        PolicyBody {
            name,
            action,
            redirect_pool_id: self.pool,
            redirect_url: self.url,
            redirect_http_code: self.status,
            keep_query_string: (action != TO_POOL).then_some(self.keep_query),
            rules,
        }
    }
}

impl PolicyBody<'_> {
    fn submitted(self, mut policy: Policy, replace_rules: bool) -> Policy {
        policy.action = self.action.into();
        policy.redirect_pool_id = self.redirect_pool_id.into();
        policy.redirect_url = self.redirect_url.into();
        policy.redirect_http_code = self.redirect_http_code;
        policy.keep_query_string = self.keep_query_string.unwrap_or(false);
        if replace_rules {
            policy.l7_rules = self
                .rules
                .into_iter()
                .map(|rule| L7Rule {
                    rule_type: rule.rule_type.into(),
                    compare_type: rule.compare_type.into(),
                    rule_value: rule.rule_value.into(),
                    ..L7Rule::default()
                })
                .collect();
        }
        policy
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a listener policy and optionally waits for its confirmation.
    pub async fn create_policy(
        &self,
        ctx: &RequestContext,
        input: CreatePolicyInput,
    ) -> WriteResult<CreatePolicyOutput> {
        crate::wire::product::exit::finish!(async {
            let ids = [
                ("LoadBalancerID", input.load_balancer_id.as_str()),
                ("ListenerID", input.listener_id.as_str()),
            ];
            let mut write = ChildWriter::new(self, ctx, CREATE, &input.load_balancer_id, &ids)
                .map_err(not_sent)?;
            encode_body(&input, &write.evidence).map_err(not_sent)?;
            for (field, value) in [
                ("Name", input.name.as_str()),
                ("Action", input.action.as_str()),
            ] {
                if value.is_empty() {
                    return Err(not_sent(invalid_input(&format!(
                        "{CREATE} requires {field}"
                    ))));
                }
            }
            check_pool_id(CREATE, &input.redirect_pool_id).map_err(not_sent)?;
            let redirect = Redirect {
                pool: &input.redirect_pool_id,
                url: &input.redirect_url,
                status: input.redirect_http_code,
                keep_query: input.keep_query_string,
            };
            redirect.check(CREATE, &input.action).map_err(not_sent)?;
            let rules = encode_rules(CREATE, &input.rules).map_err(not_sent)?;
            let body = redirect.into_body(&input.name, &input.action, rules);
            let bytes = encode_body(&body, &write.evidence).map_err(not_sent)?;
            let _lock = self
                .lock_load_balancer(ctx, &input.load_balancer_id)
                .await
                .map_err(|error| not_sent(Box::new(error)))?;
            write.ready(ctx, None).await.map_err(not_sent)?;
            let target = ChildTarget {
                read: &LIST_POLICIES,
                ..policy_target("", &ids)
            };
            let (status, response) = write
                .send(ctx, Method::POST, &target, bytes, None)
                .await
                .map_err(|failure| write.create_failure(failure, "list-policies"))?;
            let uuid = write.acknowledged_id(status, &response, "list-policies", input.no_wait)?;
            let fallback = body.submitted(
                Policy {
                    uuid: uuid.clone(),
                    name: input.name.clone(),
                    ..Policy::default()
                },
                true,
            );
            let ids = [
                ("LoadBalancerID", input.load_balancer_id.as_str()),
                ("ListenerID", input.listener_id.as_str()),
                ("PolicyID", uuid.as_str()),
            ];
            let target = policy_target(&uuid, &ids);
            finish(
                ctx,
                &mut write,
                &target,
                input.no_wait,
                true,
                fallback,
                |policy, projection| CreatePolicyOutput { policy, projection },
            )
            .await
        })
    }

    /// Updates a policy including its action, redirect settings, and match rules.
    pub async fn update_policy(
        &self,
        ctx: &RequestContext,
        input: UpdatePolicyInput,
    ) -> WriteResult<UpdatePolicyOutput> {
        crate::wire::product::exit::finish!(async {
            let ids = [
                ("LoadBalancerID", input.load_balancer_id.as_str()),
                ("ListenerID", input.listener_id.as_str()),
                ("PolicyID", input.policy_id.as_str()),
            ];
            let mut write = ChildWriter::new(self, ctx, UPDATE, &input.load_balancer_id, &ids)
                .map_err(not_sent)?;
            if encode_body(&input, &write.evidence)
                .map_err(not_sent)?
                .as_ref()
                == b"{}"
            {
                return Err(not_sent(invalid_input(&format!(
                    "{UPDATE} requires at least one field to change"
                ))));
            }
            check_pool_id(
                UPDATE,
                input.redirect_pool_id.as_deref().unwrap_or_default(),
            )
            .map_err(not_sent)?;
            let requested_rules = input
                .rules
                .as_deref()
                .map(|rules| encode_rules(UPDATE, rules))
                .transpose()
                .map_err(not_sent)?;
            let _lock = self
                .lock_load_balancer(ctx, &input.load_balancer_id)
                .await
                .map_err(|error| not_sent(Box::new(error)))?;
            let target = policy_target(&input.policy_id, &ids);
            let ready = write.ready(ctx, Some(&target)).await.map_err(not_sent)?;
            let policy: Policy = GET_POLICY
                .parse(ready.child.as_deref().unwrap_or("null"), &write.evidence)
                .map_err(not_sent)?;
            let action = input.action.as_deref().unwrap_or(&policy.action);
            let redirect = Redirect {
                pool: merge_redirect(
                    action != TO_URL,
                    input.redirect_pool_id.as_deref(),
                    policy.redirect_pool_id.as_str(),
                ),
                url: merge_redirect(
                    action != TO_POOL,
                    input.redirect_url.as_deref(),
                    policy.redirect_url.as_str(),
                ),
                status: merge_redirect(
                    action != TO_POOL,
                    input.redirect_http_code,
                    policy.redirect_http_code,
                ),
                keep_query: merge_redirect(
                    action != TO_POOL,
                    input.keep_query_string,
                    policy.keep_query_string,
                ),
            };
            redirect.check(UPDATE, action).map_err(not_sent)?;
            let rules = match requested_rules {
                Some(rules) => rules,
                None => rules_from_read(&policy, &write).map_err(not_sent)?,
            };
            let body = redirect.into_body("", action, rules);
            let bytes = encode_body(&body, &write.evidence).map_err(not_sent)?;
            write
                .send(ctx, Method::PUT, &target, bytes, Some(&target))
                .await?;
            let fallback = if input.no_wait {
                body.submitted(policy.clone(), input.rules.is_some())
            } else {
                policy
            };
            finish(
                ctx,
                &mut write,
                &target,
                input.no_wait,
                false,
                fallback,
                |policy, projection| UpdatePolicyOutput { policy, projection },
            )
            .await
        })
    }
}

fn encode_rules<'a>(
    operation: &str,
    rules: &'a [PolicyRuleInput],
) -> Result<Vec<RuleBody<'a>>, Box<Error>> {
    if rules.is_empty() {
        return Err(no_rules(operation));
    }
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let entry = RuleBody {
                rule_type: &rule.r#type,
                compare_type: &rule.compare_type,
                rule_value: &rule.value,
            };
            if incomplete(&entry) {
                return Err(invalid_input(&format!(
                    "{operation}: Rules[{index}] requires Type, CompareType, and Value"
                )));
            }
            Ok(entry)
        })
        .collect()
}

fn rules_from_read<'a>(
    policy: &'a Policy,
    write: &ChildWriter<'_>,
) -> Result<Vec<RuleBody<'a>>, Box<Error>> {
    if policy.l7_rules.is_empty() {
        return Err(no_rules(UPDATE));
    }
    policy
        .l7_rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let entry = RuleBody {
                rule_type: &rule.rule_type,
                compare_type: &rule.compare_type,
                rule_value: &rule.rule_value,
            };
            if incomplete(&entry) {
                let read = Read {
                    id: UPDATE,
                    route: GET_POLICY.route,
                };
                return Err(Box::new(read.api(
                    "",
                    &format!(
                        "policy rule {index} read back incomplete; refusing to resend it unchanged"
                    ),
                    &write.evidence,
                )));
            }
            Ok(entry)
        })
        .collect()
}

fn incomplete(rule: &RuleBody<'_>) -> bool {
    rule.rule_type.is_empty() || rule.compare_type.is_empty() || rule.rule_value.is_empty()
}

fn no_rules(operation: &str) -> Box<Error> {
    invalid_input(&format!("{operation}: a policy requires at least one rule"))
}

fn check_pool_id(operation: &str, id: &str) -> Result<(), Box<Error>> {
    if id.is_empty() {
        return Ok(());
    }
    validate_ids(operation, &[("RedirectPoolID", id)], PathIds::Strict)
}

fn merge_redirect<T: Default>(carried: bool, input: Option<T>, current: T) -> T {
    input.unwrap_or_else(|| if carried { current } else { T::default() })
}

fn zero(value: &i64) -> bool {
    *value == 0
}

fn policy_target<'a>(id: &'a str, ids: &'a [(&'static str, &'a str)]) -> ChildTarget<'a> {
    ChildTarget {
        kind: "policy",
        id,
        read: &GET_POLICY,
        ids,
        shape: &POLICY,
        guard_field: Some("action"),
        validate: |read, row, evidence| read.parse::<Policy>(row, evidence).map(|_| ()),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
async fn finish<O>(
    ctx: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
    no_wait: bool,
    creating: bool,
    mut policy: Policy,
    output: impl FnOnce(Policy, Option<PolicyProjection>) -> O,
) -> WriteResult<O> {
    let result = if no_wait {
        Ok(())
    } else {
        settle(ctx, write, target, creating, &mut policy).await
    };
    let retained = PolicyProjection::retain(&mut policy, &write.evidence)
        .map(|projection| output(policy, projection))
        .map_err(|()| GET_POLICY.decode_error(&write.evidence, DecodeError::ReflectedCredential));
    match (result, retained) {
        (Ok(()), Ok(output)) => Ok(output),
        (Ok(()), Err(error)) => Err(Failure::new(*error, None, WriteDispatch::MayHaveBeenSent)),
        (Err(error), retained) => {
            let partial = if write.evidence.can_retain_partial() {
                retained.ok()
            } else {
                None
            };
            Err(Failure::new(
                *error,
                partial,
                WriteDispatch::MayHaveBeenSent,
            ))
        }
    }
}

async fn settle(
    ctx: &RequestContext,
    write: &mut ChildWriter<'_>,
    target: &ChildTarget<'_>,
    creating: bool,
    last: &mut Policy,
) -> Result<(), Box<Error>> {
    let clock = write.client.session.clock();
    let wait = ctx.child_with_deadline(clock.monotonic_now() + BOUND);
    write
        .settle_child(ctx, &wait, target, creating, last, |row: &Policy| {
            (&row.uuid, &row.progress_status)
        })
        .await
}
