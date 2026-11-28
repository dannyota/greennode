//! Read resource tags from the vServer gateway.

#[cfg(test)]
mod tests;
mod writes;

use crate::config::Session;
use crate::wire::product::{
    operation::{self, Operation, Output},
    redirect::{Classification, Retry},
    retry,
    sender::Response,
};
use crate::wire::strict_json::{self, Shape, ShapeField};
use crate::{Config, Error, ErrorClass, Failure, List, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};

/// Tagging-specific failure details.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ErrorDetail {
    /// The key belongs to a system tag.
    SystemTag,
    /// The replacement could not be confirmed.
    NotSettled,
}

crate::error::debug_record!(ErrorDetail; SystemTag, NotSettled);

pub use crate::error::SystemTagText;

/// One resource tag, including whether it is managed by the platform.
#[derive(Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct Tag {
    /// Tag key.
    pub key: String,
    /// Tag value.
    pub value: String,
    /// Whether the platform manages this tag.
    pub system_tag: bool,
    /// Creation timestamp, when supplied.
    pub created_at: String,
}

crate::wire::product::output::public_fields!(Tag {
    key,
    value,
    system_tag,
    created_at
});

/// Identifies the resource whose tags should be read.
#[derive(Clone)]
pub struct ListResourceTagsInput {
    /// Resource identifier.
    pub resource_id: String,
}

crate::error::debug_record!(ListResourceTagsInput { resource_id });
impl ListResourceTagsInput {
    /// Stores the resource identifier.
    pub fn new(resource_id: impl Into<String>) -> Self {
        Self {
            resource_id: resource_id.into(),
        }
    }
}

/// All tags on a resource, including system tags.
pub type ListResourceTagsOutput = List<Tag>;

/// Identifies a resource and user tag to set.
#[derive(Clone)]
pub struct TagResourceInput {
    /// Resource identifier.
    pub resource_id: String,
    /// Resource type sent to the gateway.
    pub resource_type: String,
    /// User tag key.
    pub key: String,
    /// User tag value.
    pub value: String,
}

crate::error::debug_record!(TagResourceInput {
    resource_id,
    resource_type,
    key,
    value
});

/// Identifies a resource and user tag to remove.
#[derive(Clone)]
pub struct UntagResourceInput {
    /// Resource identifier.
    pub resource_id: String,
    /// Resource type sent to the gateway.
    pub resource_type: String,
    /// User tag key.
    pub key: String,
}

crate::error::debug_record!(UntagResourceInput {
    resource_id,
    resource_type,
    key
});

/// The last observed tags and the value before a replacement.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct WriteResourceTagsOutput {
    /// All observed tags, including system tags.
    pub tags: Vec<Tag>,
    /// Previous value, if the key existed.
    pub previous: Option<String>,
    /// Whether a replacement was needed.
    pub changed: bool,
}

crate::wire::product::output::public_fields!(WriteResourceTagsOutput {
    tags,
    previous,
    changed
});

/// Shares the configuration's session for tag operations.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

#[derive(Default, Deserialize)]
struct Snapshot(#[serde(default)] Vec<Tag>);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTag {
    key: Option<String>,
    value: Option<String>,
    system_tag: Option<bool>,
    #[serde(default)]
    created_at: Option<String>,
}

fn decode_snapshot(body: &[u8]) -> Result<Vec<Tag>, ()> {
    let wire: Vec<WireTag> = serde_json::from_slice(body).map_err(|_| ())?;
    let mut seen = HashSet::new();
    wire.into_iter()
        .map(|tag| {
            let key = tag.key.filter(|key| !key.is_empty()).ok_or(())?;
            if !seen.insert(key.clone()) {
                return Err(());
            }
            Ok(Tag {
                key,
                value: tag.value.ok_or(())?,
                system_tag: tag.system_tag.ok_or(())?,
                created_at: tag.created_at.unwrap_or_default(),
            })
        })
        .collect()
}

const TAG_SHAPE: Shape<'static> = Shape::Array(&Shape::Object(&[
    ShapeField {
        name: "key",
        shape: Shape::Any,
    },
    ShapeField {
        name: "value",
        shape: Shape::Any,
    },
    ShapeField {
        name: "systemTag",
        shape: Shape::Any,
    },
    ShapeField {
        name: "createdAt",
        shape: Shape::Any,
    },
]));

impl Output for Snapshot {
    const SCHEMA: Shape<'static> = TAG_SHAPE;
    fn decode_response(op: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        response.classify(op.id, op.success, (true, op.sensitive, op.retry), false)?;
        let invalid = || Box::new(Error::new(ErrorClass::Api));
        if response.malformed_json() {
            return Err(Box::new(Error::from_api(
                crate::ApiError::without_fallback(
                    op.id,
                    None,
                    crate::error::SafeText::new("", &[]),
                    crate::error::SafeText::new("response failed to decode", &[]),
                    false,
                ),
            )));
        }
        response.decode_projection(|body| {
            strict_json::validate_shape(body, &Self::SCHEMA, op.cap).map_err(|_| invalid())?;
            let mut tags = decode_snapshot(body).map_err(|_| invalid())?;
            for tag in &mut tags {
                if response
                    .credentials
                    .text_reflects(if tag.system_tag { "true" } else { "false" })
                {
                    return Err(invalid());
                }
                for value in [&mut tag.key, &mut tag.value, &mut tag.created_at] {
                    if response.credentials.text_reflects(value) {
                        *value = "<redacted>".to_owned();
                    }
                }
            }
            Ok(Self(tags))
        })
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a client without I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
        }
    }
    /// Reads all user and system tags for a resource.
    pub async fn list_resource_tags(
        &self,
        ctx: &RequestContext,
        input: ListResourceTagsInput,
    ) -> Result<ListResourceTagsOutput, Failure<ListResourceTagsOutput>> {
        crate::wire::product::exit::finish!(async {
            let failure = |error| Failure::new(error, None, WriteDispatch::NotAWrite);
            if input.resource_id.is_empty()
                || !input
                    .resource_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(failure(Error::new(ErrorClass::InvalidInput)));
            }
            let project = self
                .session
                .require_project_id(ctx)
                .await
                .map_err(|error| failure(*error))?;
            let base = self
                .session
                .endpoints()
                .vserver
                .as_ref()
                .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
            let mut jitter = || self.session.retry_jitter();
            let runtime = self
                .session
                .runtime(ctx, &mut jitter)
                .map_err(|error| failure(*error))?;
            let op = Operation {
                id: "tagging.ListResourceTags",
                template: "v2/{}/tag/resource/{}",
                method: Method::GET,
                class: Classification::Read,
                retry: Retry::Repeatable,
                success: &[StatusCode::OK],
                cap: 4 << 20,
                allow_recovery: true,
                allow_empty: false,
                no_output: false,
                keep_status: false,
                raw_body: false,
                overflow_overrides_status: false,
                sensitive: false,
                reflection: crate::wire::product::operation::Reflection::Redact,
                no_redirect: false,
                report_redirect: false,
                retry_config: retry::Config::default(),
            };
            let (_, snapshot) = operation::execute::<Snapshot>(
                runtime,
                base,
                &op,
                &[&project, &input.resource_id],
                &[],
                self.session.bearer_source(),
                bytes::Bytes::new(),
            )
            .await
            .map_err(|error| {
                let (error, _, dispatch) = error.into_parts();
                Failure::new(error, None, dispatch)
            })?;
            Ok(List { items: snapshot.0 })
        })
    }
}
