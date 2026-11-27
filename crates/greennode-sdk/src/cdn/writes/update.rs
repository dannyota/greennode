use std::collections::{BTreeMap, HashSet};

use http::Method;
use serde::{Deserialize, Serialize};
use serde_json::{
    Value, json,
    value::{RawValue, to_raw_value},
};

use super::{
    Call, Client, WebAccelerator, WriteKind, encode, invalid, possibly_applied, record_user_uuids,
    settle::{Poll, Target, finish},
};
use crate::cdn::wire::convert;
use crate::wire::product::options::PathIds;
use crate::{Failure, RequestContext};

const UPDATE: Call = Call::write("cdn.UpdateWebAccelerator", "v1/cdn/update", &Method::PUT);
type Object = BTreeMap<String, Box<RawValue>>;

/// Changes the value of a default rule action by name or adds a new action.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct RuleActionInput {
    /// Name of the rule action.
    pub name: String,
    /// Server string value, including JSON text for actions such as `hsts`.
    #[serde(default)]
    pub value: String,
}

crate::error::debug_record!(RuleActionInput { name, value });

/// Adds an origin or replaces the fields of an existing origin.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct UpstreamInput {
    /// Identifies an existing origin. An empty value adds an origin.
    #[serde(rename = "ID", default)]
    pub id: String,
    /// Origin priority.
    #[serde(default)]
    pub priority: i64,
    /// Origin IP address.
    #[serde(rename = "IPAddress")]
    pub ip_address: String,
    /// Origin type, which defaults to `httpOrigin`.
    #[serde(default)]
    pub upstream_type: String,
    /// Optional origin value. HTTP origins use null.
    pub origin_value: Option<String>,
    /// Whether the origin uses SSL.
    #[serde(rename = "UseSSL", default)]
    pub use_ssl: bool,
}

crate::error::debug_record!(UpstreamInput {
    id,
    priority,
    ip_address,
    upstream_type,
    origin_value,
    use_ssl
});

/// Changes to merge into a fresh Web Accelerator read.
#[derive(Clone)]
pub struct UpdateWebAcceleratorInput {
    /// CDN identifier.
    pub cdn_id: String,
    /// Rule actions to add or modify.
    pub set_rule_actions: Vec<RuleActionInput>,
    /// Rule action names to remove.
    pub remove_rule_actions: Vec<String>,
    /// Origins to add or modify. Omitted origins stay in the CDN.
    pub upstreams: Vec<UpstreamInput>,
    /// Replacement load balancing type.
    pub lb_type: Option<String>,
    /// Replacement failover errors. An empty list clears the errors.
    pub fail_over_error_codes: Option<Vec<String>>,
    /// Replacement certificate identifier.
    pub certificate_id: Option<String>,
    /// Replacement origin Host header.
    pub origin_host_header: Option<String>,
    /// Replacement CNAMEs. An empty list clears the names.
    pub c_names: Option<Vec<String>>,
    /// Returns after one follow-up detail read instead of waiting for settlement.
    pub no_wait: bool,
}

crate::error::debug_record!(UpdateWebAcceleratorInput {
    cdn_id,
    set_rule_actions,
    remove_rule_actions,
    upstreams,
    lb_type,
    fail_over_error_codes,
    certificate_id,
    origin_host_header,
    c_names,
    no_wait
});

impl UpdateWebAcceleratorInput {
    /// Creates an input with no changes and settlement enabled.
    pub fn new(cdn_id: impl Into<String>) -> Self {
        Self {
            cdn_id: cdn_id.into(),
            set_rule_actions: Vec::new(),
            remove_rule_actions: Vec::new(),
            upstreams: Vec::new(),
            lb_type: None,
            fail_over_error_codes: None,
            certificate_id: None,
            origin_host_header: None,
            c_names: None,
            no_wait: false,
        }
    }

    fn validate(&self) -> Result<(), Failure<()>> {
        if self.set_rule_actions.is_empty()
            && self.remove_rule_actions.is_empty()
            && self.upstreams.is_empty()
            && self.lb_type.is_none()
            && self.fail_over_error_codes.is_none()
            && self.certificate_id.is_none()
            && self.origin_host_header.is_none()
            && self.c_names.is_none()
        {
            return Err(invalid(
                "cdn.UpdateWebAccelerator requires at least one change besides CDNID and NoWait",
            ));
        }
        let mut names = HashSet::new();
        for name in self
            .set_rule_actions
            .iter()
            .map(|action| &action.name)
            .chain(&self.remove_rule_actions)
        {
            if name.is_empty() || !names.insert(name) {
                return Err(invalid(
                    "cdn.UpdateWebAccelerator requires unique nonempty rule action names",
                ));
            }
        }
        if self.upstreams.iter().any(|origin| {
            origin.ip_address.is_empty()
                || (!origin.id.is_empty() && !PathIds::Strict.allows(&origin.id))
        }) {
            return Err(invalid(
                "cdn.UpdateWebAccelerator requires an IPAddress and valid ID for each existing upstream",
            ));
        }
        Ok(())
    }

    fn values(&self) -> Value {
        json!([
            self.set_rule_actions,
            self.remove_rule_actions,
            self.upstreams,
            self.lb_type,
            self.fail_over_error_codes,
            self.certificate_id,
            self.origin_host_header,
            self.c_names,
        ])
    }
}

/// The last good Web Accelerator detail read during an update.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateWebAcceleratorOutput {
    /// Web Accelerator detail.
    pub web_accelerator: WebAccelerator,
}

crate::wire::product::output::public_fields!(UpdateWebAcceleratorOutput { web_accelerator });

impl Client {
    /// Merges changes into an active Web Accelerator, sends once, and reads the resulting detail.
    /// If the accepted update cannot settle, its failure includes the last safe detail.
    pub async fn update_web_accelerator(
        &self,
        ctx: &RequestContext,
        input: UpdateWebAcceleratorInput,
    ) -> Result<UpdateWebAcceleratorOutput, Failure<UpdateWebAcceleratorOutput>> {
        crate::wire::product::exit::finish!(async {
            input.validate().map_err(convert)?;
            let mut evidence = self
                .write_evidence(ctx, Some(&input.values()), Some(&input.cdn_id))
                .map_err(convert)?;
            let (initial, _) = self
                .preflight(
                    ctx,
                    &UPDATE,
                    &input.cdn_id,
                    WriteKind::Update,
                    &mut evidence,
                )
                .await
                .map_err(convert)?;
            let merged = merge_update(initial.raw.get(), &input).map_err(convert)?;
            let malformed = |_| invalid("the merged CDN detail is not valid JSON");
            let original: Value = serde_json::from_str(initial.raw.get()).map_err(malformed)?;
            let projected = serde_json::to_value(&merged).map_err(malformed)?;
            let output = |web_accelerator| UpdateWebAcceleratorOutput { web_accelerator };
            if original == projected {
                return finish(initial, None, &mut evidence, output);
            }
            let body = encode(&merged)?;
            if evidence.output_reflects(&projected)
                || evidence.peer_body_reflects(&body).unwrap_or(true)
            {
                return Err(invalid(
                    "the merged CDN detail contains a configured credential",
                ));
            }
            record_user_uuids(initial.raw.get(), &mut evidence);
            if let Err(failure) = self
                .call_with_evidence(ctx, &UPDATE, &[], body, &mut evidence)
                .await
            {
                let failure = self
                    .explain_401(ctx, &UPDATE, &input.cdn_id, failure, &mut evidence)
                    .await;
                return Err(convert(possibly_applied(failure, "update")));
            }
            let mut poll = Poll::new(self, ctx, &UPDATE, &input.cdn_id, &mut evidence);
            let (last, error) = if input.no_wait {
                poll.read_once(initial).await
            } else {
                poll.settle(Target::ACTIVE, initial, false).await
            };
            finish(last, error, &mut evidence, output)
        })
    }
}

fn merge_update(raw: &str, input: &UpdateWebAcceleratorInput) -> Result<Object, Failure<()>> {
    let mut object: Object =
        serde_json::from_str(raw).map_err(|_| invalid("the CDN detail is not an object"))?;
    if !input.set_rule_actions.is_empty() || !input.remove_rule_actions.is_empty() {
        let actions = merge_actions(object.get("defaultRuleAction").map(Box::as_ref), input)?;
        set(&mut object, "defaultRuleAction", &actions)?;
    }
    if !input.upstreams.is_empty() {
        let origins = merge_upstreams(object.get("upstreams").map(Box::as_ref), &input.upstreams)?;
        set(&mut object, "upstreams", &origins)?;
    }
    for (key, value) in [
        ("lbType", &input.lb_type),
        ("sslId", &input.certificate_id),
        ("originHostHeader", &input.origin_host_header),
    ] {
        if let Some(value) = value {
            set(&mut object, key, value)?;
        }
    }
    for (key, value) in [
        ("failOverErrorCode", &input.fail_over_error_codes),
        ("cName", &input.c_names),
    ] {
        if let Some(value) = value {
            set(&mut object, key, value)?;
        }
    }
    Ok(object)
}

fn collection(value: Option<&RawValue>) -> Result<Vec<Object>, Failure<()>> {
    match value.map(RawValue::get) {
        None | Some("null" | "\"\"") => Ok(Vec::new()),
        Some(text) => serde_json::from_str(text)
            .map_err(|_| invalid("the CDN detail does not have a valid list of rules or origins")),
    }
}

fn set(object: &mut Object, key: &str, value: &impl Serialize) -> Result<(), Failure<()>> {
    let raw =
        to_raw_value(value).map_err(|_| invalid("the merged CDN detail could not be encoded"))?;
    object.insert(key.into(), raw);
    Ok(())
}

fn string(object: &Object, key: &str) -> Option<String> {
    serde_json::from_str(object.get(key)?.get()).ok()
}

fn merge_actions(
    current: Option<&RawValue>,
    input: &UpdateWebAcceleratorInput,
) -> Result<Vec<Object>, Failure<()>> {
    let mut actions = collection(current)?;
    for action in &input.set_rule_actions {
        let mut found = false;
        for existing in &mut actions {
            if string(existing, "actionName").as_deref() == Some(&action.name) {
                set(existing, "value", &action.value)?;
                found = true;
            }
        }
        if !found {
            let mut new = Object::new();
            set(&mut new, "actionName", &action.name)?;
            set(&mut new, "value", &action.value)?;
            set(&mut new, "order", &0)?;
            actions.push(new);
        }
    }
    actions.retain(|entry| {
        string(entry, "actionName").is_none_or(|name| !input.remove_rule_actions.contains(&name))
    });
    Ok(actions)
}

fn merge_upstreams(
    current: Option<&RawValue>,
    upstreams: &[UpstreamInput],
) -> Result<Vec<Value>, Failure<()>> {
    let existing = collection(current)?;
    upstreams.iter().map(|upstream| {
        let origin_type = if upstream.upstream_type.is_empty() { "httpOrigin" } else { &upstream.upstream_type };
        let mut entry = json!({
            "priority": upstream.priority, "ipaddress": upstream.ip_address, "upstreamType": origin_type,
            "originValue": upstream.origin_value, "useSsl": upstream.use_ssl,
        });
        if !upstream.id.is_empty() {
            let id = existing.iter().filter_map(|origin| origin.get("cdnUpstreamId"))
                .filter_map(|raw| serde_json::from_str::<Value>(raw.get()).ok())
                .find(|raw| match raw {
                    Value::String(id) => *id == upstream.id,
                    Value::Number(id) => id.as_str() == upstream.id,
                    _ => false,
                })
                .ok_or_else(|| invalid("cdn.UpdateWebAccelerator: the CDN has no upstream with that ID"))?;
            if let Some(object) = entry.as_object_mut() {
                object.insert("cdnUpstreamId".into(), id);
            }
        }
        Ok(entry)
    }).collect()
}
