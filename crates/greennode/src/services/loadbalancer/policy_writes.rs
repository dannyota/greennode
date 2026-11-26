use super::*;
use serde_json::Value;

const POLICY_NAME: Field = flag("name", "string", true);
const ACTION: Field = flag("action", "string", true);

macro_rules! option_field {
    ($field:ident, $flag:ident, $name:literal, $ty:ty, $decode:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$field = Some(($decode)(value).ok_or(())?);
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                input.$field = Some(matches.get_one::<$ty>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! json_list {
    ($field:ident, $name:literal, $decode:ident, $into:expr) => {
        InputField {
            json_name: $name,
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$field = ($into)($decode(value)?);
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }
    };
}

fn rules(value: Value) -> Result<Vec<sdk::PolicyRuleInput>, ()> {
    let Value::Array(items) = value else {
        return Err(());
    };
    items
        .into_iter()
        .map(|item| {
            if item.is_null() {
                return Ok(sdk::PolicyRuleInput::default());
            }
            let Value::Object(object) = item else {
                return Err(());
            };
            let mut rule_type = None;
            let mut compare = None;
            let mut value = None;
            for (key, item) in object {
                let field = if key.eq_ignore_ascii_case("Type") {
                    &mut rule_type
                } else if key.eq_ignore_ascii_case("CompareType") {
                    &mut compare
                } else if key.eq_ignore_ascii_case("Value") {
                    &mut value
                } else {
                    return Err(());
                };
                if field.is_some() {
                    return Err(());
                }
                *field = Some(if item.is_null() {
                    String::new()
                } else {
                    item.as_str().ok_or(())?.to_owned()
                });
            }
            Ok(sdk::PolicyRuleInput {
                r#type: rule_type.unwrap_or_default(),
                compare_type: compare.unwrap_or_default(),
                value: value.unwrap_or_default(),
            })
        })
        .collect()
}

const UPDATE_ACTION: Field = flag("action", "*string", false);
const REDIRECT_POOL_ID: Field = flag("redirect-pool-id", "string", false);
const UPDATE_REDIRECT_POOL_ID: Field = flag("redirect-pool-id", "*string", false);
const REDIRECT_URL: Field = flag("redirect-url", "string", false);
const UPDATE_REDIRECT_URL: Field = flag("redirect-url", "*string", false);
const REDIRECT_HTTP_CODE: Field = flag("redirect-http-code", "int", false);
const UPDATE_REDIRECT_HTTP_CODE: Field = flag("redirect-http-code", "*int", false);
const KEEP_QUERY_STRING: Field = flag("keep-query-string", "bool", false);
const UPDATE_KEEP_QUERY_STRING: Field = flag("keep-query-string", "*bool", false);

pub(super) const CREATE_POLICY: Operation = operation!(
    @kind Kind::Write,
    CreatePolicyInput, create_policy, "create-policy", "CreatePolicy", "Create a listener policy",
    "greennode loadbalancer create-policy --load-balancer-id <load-balancer-id> --listener-id <listener-id> --name <name> --action <action>",
    &["Supply at least one `Rules` entry through `--cli-input-json`. A pool redirect requires a pool ID. A URL redirect requires a URL. Use `--no-wait` to return submitted settings."], [
        LOAD_BALANCER_ID => required_id!(sdk::CreatePolicyInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        LISTENER_ID => required_id!(sdk::CreatePolicyInput, listener_id, LISTENER_ID, "ListenerID"),
        POLICY_NAME => required_id!(sdk::CreatePolicyInput, name, POLICY_NAME, "Name"),
        ACTION => required_id!(sdk::CreatePolicyInput, action, ACTION, "Action"),
        REDIRECT_POOL_ID => string_field!(sdk::CreatePolicyInput, redirect_pool_id, REDIRECT_POOL_ID, "RedirectPoolID", Requirement::Optional),
        REDIRECT_URL => string_field!(sdk::CreatePolicyInput, redirect_url, REDIRECT_URL, "RedirectURL", Requirement::Optional),
        REDIRECT_HTTP_CODE => int_field!(redirect_http_code, REDIRECT_HTTP_CODE, "RedirectHTTPCode"),
        KEEP_QUERY_STRING => optional_bool!(keep_query_string, KEEP_QUERY_STRING, "KeepQueryString"),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ], [
        flag("Rules", "[]loadbalancer.PolicyRuleInput", false) => json_list!(rules, "Rules", rules, std::convert::identity),
    ]
);

pub(super) const UPDATE_POLICY: Operation = operation!(
    @kind Kind::Write,
    UpdatePolicyInput, update_policy, "update-policy", "UpdatePolicy", "Update a listener policy",
    "greennode loadbalancer update-policy --load-balancer-id <load-balancer-id> --listener-id <listener-id> --policy-id <policy-id> --action <action>",
    &["Reads and merges the current policy. Changing the action discards inherited redirect fields that the new action cannot carry. Supplying `Rules` replaces the list. Use `--no-wait` to return submitted settings."], [
        LOAD_BALANCER_ID => required_id!(sdk::UpdatePolicyInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        LISTENER_ID => required_id!(sdk::UpdatePolicyInput, listener_id, LISTENER_ID, "ListenerID"),
        POLICY_ID => required_id!(sdk::UpdatePolicyInput, policy_id, POLICY_ID, "PolicyID"),
        UPDATE_ACTION => option_field!(action, UPDATE_ACTION, "Action", String, |value: Value| value.as_str().map(str::to_owned)),
        UPDATE_REDIRECT_POOL_ID => option_field!(redirect_pool_id, UPDATE_REDIRECT_POOL_ID, "RedirectPoolID", String, |value: Value| value.as_str().map(str::to_owned)),
        UPDATE_REDIRECT_URL => option_field!(redirect_url, UPDATE_REDIRECT_URL, "RedirectURL", String, |value: Value| value.as_str().map(str::to_owned)),
        UPDATE_REDIRECT_HTTP_CODE => option_field!(redirect_http_code, UPDATE_REDIRECT_HTTP_CODE, "RedirectHTTPCode", i64, |value: Value| value.as_i64()),
        UPDATE_KEEP_QUERY_STRING => option_field!(keep_query_string, UPDATE_KEEP_QUERY_STRING, "KeepQueryString", bool, |value: Value| value.as_bool()),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ], [
        flag("Rules", "*[]loadbalancer.PolicyRuleInput", false) => json_list!(rules, "Rules", rules, Some),
    ]
);
