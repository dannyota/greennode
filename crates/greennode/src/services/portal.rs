mod project;

use greennode_sdk::portal::{Client, GetUserInfoInput};
use greennode_sdk::{Config, RequestContext};
use serde::Serialize;

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError};
use crate::cli::op::{Handler, Kind, Operation, Service};
use crate::cli::render::DynamicMap;

const USER_INFO_INPUT: InputDescriptor<GetUserInfoInput> = InputDescriptor {
    defaults: GetUserInfoInput::default,
    fields: &[],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|_, json| {
        if json == Some("file://") {
            return Err(InputError("--cli-input-json: cannot open file"));
        }
        Ok(())
    }),
    guard: None,
};

pub(super) const SERVICE: Service = Service {
    name: "portal",
    error_rules: &[],
    operations: &[
        project::QUOTA,
        project::TAG,
        Operation {
            name: "get-user-info",
            sdk_operation: "portal.GetUserInfo",
            short_help: "Read the current portal user's information",
            example: Some("greennode portal get-user-info --query UserInfo"),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[
                "Prints account data such as email, names, user ID, cash and billing status.",
                "Secret-looking map keys, such as password, token and credential, print as\n`<redacted>` at any depth.",
                "Responses are limited to 8 MiB. The SDK suppresses response captures and rejects\nreflected credentials, duplicate keys and conflicting envelopes.",
            ],
            handler: Some(Handler::new(
                &USER_INFO_INPUT,
                |operation, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        operation,
                        matches,
                        &USER_INFO_INPUT,
                        get_user_info,
                    ))
                },
            )),
        },
        project::QUOTAS,
        project::ZONES,
    ],
};

#[derive(Serialize)]
struct UserInfoProjection {
    #[serde(rename = "UserInfo")]
    user_info: DynamicMap<greennode_sdk::portal::UserInfo>,
}

fn get_user_info<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a GetUserInfoInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let result = Client::new(config).get_user_info(ctx, staged.clone()).await;
        dispatch::project_result(result, |output| UserInfoProjection {
            user_info: DynamicMap(output.user_info),
        })
    })
}
