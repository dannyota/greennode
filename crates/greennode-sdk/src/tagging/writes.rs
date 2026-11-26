use super::*;
use crate::wire::product::{evidence::Evidence, options::Options};
use serde_json::{Value, json};

#[derive(Default, Deserialize)]
struct RawSnapshot(Vec<Tag>);

impl Output for RawSnapshot {
    const SCHEMA: Shape<'static> = TAG_SHAPE;

    fn decode_response(op: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        crate::wire::product::response::classify(
            op.id,
            response.status,
            bytes::Bytes::copy_from_slice(&response.body),
            op.success,
            (true, op.sensitive, op.retry),
            &response.credentials,
        )?;
        strict_json::validate_shape(&response.body, &Self::SCHEMA, op.cap)
            .map_err(|_| Box::new(Error::new(ErrorClass::Api)))?;
        decode_snapshot(&response.body)
            .map(Self)
            .map_err(|_| Box::new(Error::new(ErrorClass::Api)))
    }
}

#[derive(Default, Deserialize)]
struct Acknowledgement;
impl Output for Acknowledgement {
    const SCHEMA: Shape<'static> = Shape::Any;
}

fn redact_output(evidence: &Evidence, value: &mut Value) {
    if let Some(tags) = value.get_mut("Tags").and_then(Value::as_array_mut) {
        for tag in tags {
            if let Some(system) = tag.get_mut("SystemTag")
                && system.as_bool().is_some_and(|flag| {
                    evidence.credentials().reflects(flag.to_string().as_bytes())
                })
            {
                *system = Value::String("<redacted>".to_owned());
            }
        }
    }
    if let Some(changed) = value.get_mut("Changed")
        && changed
            .as_bool()
            .is_some_and(|flag| evidence.credentials().reflects(flag.to_string().as_bytes()))
    {
        *changed = Value::String("<redacted>".to_owned());
    }
    evidence.redact(value);
}

fn safe_partial(evidence: &Evidence, mut value: Value) -> Value {
    redact_output(evidence, &mut value);
    value
}

fn system_tag_error() -> Error {
    Error::from_api(crate::ApiError::without_fallback(
        "",
        None,
        crate::error::SafeText::new("SystemTag", &[]),
        crate::error::SafeText::new("tagging: key is a system tag", &[]),
        false,
    ))
}

fn operation(id: &'static str, write: bool) -> Operation {
    Operation {
        id,
        template: "v2/{}/tag/resource/{}",
        method: if write { Method::PUT } else { Method::GET },
        class: if write {
            Classification::Write
        } else {
            Classification::Read
        },
        retry: if write {
            Retry::Once
        } else {
            Retry::Repeatable
        },
        success: &[StatusCode::OK],
        cap: 4 << 20,
        allow_recovery: !write,
        allow_empty: write,
        no_output: write,
        keep_status: write,
        overflow_overrides_status: !write,
        sensitive: false,
        reject_reflected_credentials: false,
        no_redirect: write,
        retry_config: retry::Config::default(),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Sets a user tag while preserving the resource's other user tags.
    pub async fn tag_resource(
        &self,
        ctx: &RequestContext,
        input: TagResourceInput,
    ) -> Result<Value, Failure<Value>> {
        self.replace_tag(
            ctx,
            &input.resource_id,
            &input.resource_type,
            &input.key,
            Some(&input.value),
        )
        .await
    }

    /// Removes a user tag while preserving the resource's other user tags.
    pub async fn untag_resource(
        &self,
        ctx: &RequestContext,
        input: UntagResourceInput,
    ) -> Result<Value, Failure<Value>> {
        self.replace_tag(
            ctx,
            &input.resource_id,
            &input.resource_type,
            &input.key,
            None,
        )
        .await
    }

    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Failure by value"
    )]
    async fn replace_tag(
        &self,
        ctx: &RequestContext,
        id: &str,
        resource_type: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<Value, Failure<Value>> {
        let fail = |error| Failure::new(error, None, WriteDispatch::NotSent);
        if id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || resource_type.is_empty()
            || key.is_empty()
        {
            return Err(fail(Error::new(ErrorClass::InvalidInput)));
        }
        if key.starts_with("vng.") {
            return Err(fail(system_tag_error()));
        }
        let project = self
            .session
            .require_project_id(ctx)
            .await
            .map_err(|e| fail(*e))?;
        let base = self
            .session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| fail(Error::new(ErrorClass::InvalidConfig)))?;
        let op_id = if value.is_some() {
            "tagging.TagResource"
        } else {
            "tagging.UntagResource"
        };
        let read = operation(op_id, false);
        let write = operation(op_id, true);
        let mut evidence = Evidence::new();
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|e| fail(*e))?;
        let (_, snapshot) = operation::execute_with_evidence::<RawSnapshot>(
            runtime,
            base,
            &read,
            &Options::DEFAULT,
            &[&project, id],
            &[],
            self.session.bearer_source(),
            bytes::Bytes::new(),
            &mut evidence,
        )
        .await
        .map_err(|f| {
            let (e, _, _) = f.into_parts();
            fail(e)
        })?;
        let current = snapshot.0;
        if current.iter().any(|tag| tag.system_tag && tag.key == key) {
            return Err(fail(system_tag_error()));
        }
        let mut next: Vec<Tag> = current.iter().filter(|t| !t.system_tag).cloned().collect();
        let index = next.iter().position(|tag| tag.key == key);
        let previous = index.map(|i| next[i].value.clone());
        let changed = match (index, value) {
            (Some(i), Some(value)) if next[i].value != value => {
                next[i].value = value.to_owned();
                true
            }
            (None, Some(value)) => {
                next.push(Tag {
                    key: key.to_owned(),
                    value: value.to_owned(),
                    ..Tag::default()
                });
                true
            }
            (Some(i), None) => {
                next.remove(i);
                true
            }
            _ => false,
        };
        let output = |tags: &[Tag]| json!({"Tags": tags, "Previous": previous, "Changed": changed});
        if !changed {
            let mut result = output(&current);
            redact_output(&evidence, &mut result);
            return Ok(result);
        }
        let requests: Vec<Value> = next
            .iter()
            .map(|tag| json!({"key":tag.key,"value":tag.value}))
            .collect();
        let body = json!({"resourceId":id,"resourceType":resource_type,"tagRequestList":requests});
        let body =
            serde_json::to_vec(&body).map_err(|_| fail(Error::new(ErrorClass::InvalidInput)))?;
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|e| fail(*e))?;
        let result = operation::execute_with_evidence::<Acknowledgement>(
            runtime,
            base,
            &write,
            &Options::DEFAULT,
            &[&project, id],
            &[],
            self.session.bearer_source(),
            bytes::Bytes::from(body),
            &mut evidence,
        )
        .await;
        if let Err(failure) = result {
            let dispatch = failure.write_dispatch();
            let status = failure.error().detail::<StatusCode>().copied().or_else(|| {
                failure
                    .error()
                    .api_error()
                    .and_then(|api| api.status())
                    .and_then(|status| StatusCode::from_u16(status).ok())
            });
            let rejected = status.is_some_and(|status| {
                status.is_client_error() && status.as_u16() != 408 && status.as_u16() != 499
            });
            if dispatch == WriteDispatch::NotSent || rejected {
                let (error, _, _) = failure.into_parts();
                return Err(Failure::new(error, None, dispatch));
            }
            let failure = Failure::<Value>::wrap(
                Error::new(ErrorClass::NotSettled),
                None,
                WriteDispatch::MayHaveBeenSent,
                failure,
            );
            return Err(evidence.with_partial(
                failure,
                safe_partial(&evidence, output(&current)),
                WriteDispatch::MayHaveBeenSent,
            ));
        }
        let runtime = self.session.runtime(ctx, &mut jitter).map_err(|e| {
            evidence.with_partial(
                Failure::<Value>::wrap(
                    Error::new(ErrorClass::NotSettled),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                    fail(*e),
                ),
                safe_partial(&evidence, output(&current)),
                WriteDispatch::MayHaveBeenSent,
            )
        })?;
        let confirmed = operation::execute_with_evidence::<RawSnapshot>(
            runtime,
            base,
            &read,
            &Options::DEFAULT,
            &[&project, id],
            &[],
            self.session.bearer_source(),
            bytes::Bytes::new(),
            &mut evidence,
        )
        .await;
        let (_, snapshot) = confirmed.map_err(|failure| {
            let failure = Failure::<Value>::wrap(
                Error::new(ErrorClass::NotSettled),
                None,
                WriteDispatch::MayHaveBeenSent,
                failure,
            );
            evidence.with_partial(
                failure,
                safe_partial(&evidence, output(&current)),
                WriteDispatch::MayHaveBeenSent,
            )
        })?;
        let observed: Vec<_> = snapshot
            .0
            .iter()
            .filter(|t| !t.system_tag)
            .map(|t| (&t.key, &t.value))
            .collect();
        if observed.len() != next.len()
            || next.iter().any(|t| !observed.contains(&(&t.key, &t.value)))
        {
            return Err(evidence.with_partial(
                Failure::<Value>::new(
                    Error::new(ErrorClass::NotSettled),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ),
                safe_partial(&evidence, output(&snapshot.0)),
                WriteDispatch::MayHaveBeenSent,
            ));
        }
        let mut result = output(&snapshot.0);
        redact_output(&evidence, &mut result);
        Ok(result)
    }
}
