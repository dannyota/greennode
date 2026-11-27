use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::Value;

use super::policy_guard::{Guard, Protection, protection_error};
use super::writes::write_operation;
use super::{Api, Client};
use crate::wire::product::redirect::Retry;
use crate::{Failure, RequestContext};

macro_rules! pre_guard {
    (account_policy, $guard:ident) => {
        $guard.reject_service_account_caller()
    };
    ($other:ident, $guard:ident) => {
        Ok::<(), Failure<_>>(())
    };
}

macro_rules! attachment {
    ($(#[$doc:meta])* $method:ident, $input:ident, $output:ident, $first:ident, $second:ident,
     $guard:ident, $op:literal, $field1:literal, $field2:literal, $method_type:ident, $retry:ident, $route:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $input {
            /// Required IAM identifier using ASCII letters, digits, or hyphens.
            pub $first: String,
            /// Required IAM identifier using ASCII letters, digits, or hyphens.
            pub $second: String,
        }

        impl $input {
            /// Sets both required identifiers.
            pub fn new($first: impl Into<String>, $second: impl Into<String>) -> Self {
                Self { $first: $first.into(), $second: $second.into() }
            }
        }

        /// A completed IAM attachment or membership change.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        pub struct $output {}

        crate::wire::product::output::public_fields!($output {});

        #[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
        impl Client {
            $(#[$doc])*
            pub async fn $method(&self, ctx: &RequestContext, input: $input) -> Result<$output, Failure<$output>> {
                crate::wire::product::exit::finish!(async {
                    let first = input.$first.as_str();
                    let second = input.$second.as_str();
                    let mut evidence = self.write_evidence(ctx, $op, &[($field1, first), ($field2, second)], &[])?;
                    let mut guard = Guard::new(self, ctx, $op, &mut evidence).await?;
                    pre_guard!($guard, guard)?;
                    guard.load_actions().await?;
                    $guard(&mut guard, $op, first, second).await?;
                    let operation = write_operation($op, Method::$method_type, $route, Retry::$retry, &[StatusCode::NO_CONTENT]);
                    self.write::<Value, _>(ctx, Api::Policies, &operation, &[first, second], None, &mut evidence).await?;
                    Ok($output {})
                })
            }
        }
    };
}

attachment!(
    /// Adds an unprotected IAM user to an unprotected group.
    add_user_to_group,
    AddUserToGroupInput,
    AddUserToGroupOutput,
    group_id,
    user_id,
    membership,
    "iam.AddUserToGroup",
    "GroupID",
    "UserID",
    POST,
    NonIdempotent,
    "policies-api/v1/groups/{}/iam-users/{}"
);
attachment!(
    /// Removes an unprotected IAM user from an unprotected group.
    remove_user_from_group,
    RemoveUserFromGroupInput,
    RemoveUserFromGroupOutput,
    group_id,
    user_id,
    membership,
    "iam.RemoveUserFromGroup",
    "GroupID",
    "UserID",
    DELETE,
    Repeatable,
    "policies-api/v1/groups/{}/iam-users/{}"
);
attachment!(
    /// Attaches a non-privileged policy to an unprotected group.
    attach_group_policy,
    AttachGroupPolicyInput,
    AttachGroupPolicyOutput,
    policy_id,
    group_id,
    group_policy,
    "iam.AttachGroupPolicy",
    "PolicyID",
    "GroupID",
    POST,
    NonIdempotent,
    "policies-api/v1/policies/{}/groups/{}"
);
attachment!(
    /// Detaches a non-privileged policy from an unprotected group.
    detach_group_policy,
    DetachGroupPolicyInput,
    DetachGroupPolicyOutput,
    policy_id,
    group_id,
    group_policy,
    "iam.DetachGroupPolicy",
    "PolicyID",
    "GroupID",
    DELETE,
    Repeatable,
    "policies-api/v1/policies/{}/groups/{}"
);
attachment!(
    /// Attaches a non-privileged policy to an unprotected IAM user.
    attach_user_policy,
    AttachUserPolicyInput,
    AttachUserPolicyOutput,
    policy_id,
    user_id,
    user_policy,
    "iam.AttachUserPolicy",
    "PolicyID",
    "UserID",
    POST,
    NonIdempotent,
    "policies-api/v1/policies/{}/iam-users/{}"
);
attachment!(
    /// Detaches a non-privileged policy from an unprotected IAM user.
    detach_user_policy,
    DetachUserPolicyInput,
    DetachUserPolicyOutput,
    policy_id,
    user_id,
    user_policy,
    "iam.DetachUserPolicy",
    "PolicyID",
    "UserID",
    DELETE,
    Repeatable,
    "policies-api/v1/policies/{}/iam-users/{}"
);

attachment!(
    /// Attaches a non-privileged policy to an unprotected service account.
    attach_service_account_policy,
    AttachServiceAccountPolicyInput,
    AttachServiceAccountPolicyOutput,
    policy_id,
    service_account_id,
    account_policy,
    "iam.AttachServiceAccountPolicy",
    "PolicyID",
    "ServiceAccountID",
    POST,
    NonIdempotent,
    "policies-api/v1/policies/{}/service-accounts/{}"
);
attachment!(
    /// Detaches a non-privileged policy from an unprotected service account.
    detach_service_account_policy,
    DetachServiceAccountPolicyInput,
    DetachServiceAccountPolicyOutput,
    policy_id,
    service_account_id,
    account_policy,
    "iam.DetachServiceAccountPolicy",
    "PolicyID",
    "ServiceAccountID",
    DELETE,
    Repeatable,
    "policies-api/v1/policies/{}/service-accounts/{}"
);

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
async fn account_policy<O>(
    guard: &mut Guard<'_>,
    op: &str,
    policy_id: &str,
    account_id: &str,
) -> Result<(), Failure<O>> {
    protection_error(
        op,
        guard.account(account_id).await?,
        "the service account has a privileged policy attached",
    )?;
    require_non_privileged_policy(guard, op, policy_id).await
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
async fn membership<O>(
    guard: &mut Guard<'_>,
    op: &str,
    group_id: &str,
    user_id: &str,
) -> Result<(), Failure<O>> {
    let user = guard.user(user_id).await?;
    let group = guard.group(group_id).await?;
    protection_error(op, user.max(group), "the user or the group is protected")
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
async fn group_policy<O>(
    guard: &mut Guard<'_>,
    op: &str,
    policy_id: &str,
    group_id: &str,
) -> Result<(), Failure<O>> {
    protection_error(op, guard.group(group_id).await?, "the group is protected")?;
    require_non_privileged_policy(guard, op, policy_id).await
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
async fn user_policy<O>(
    guard: &mut Guard<'_>,
    op: &str,
    policy_id: &str,
    user_id: &str,
) -> Result<(), Failure<O>> {
    protection_error(op, guard.user(user_id).await?, "the user is protected")?;
    require_non_privileged_policy(guard, op, policy_id).await
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
async fn require_non_privileged_policy<O>(
    guard: &mut Guard<'_>,
    op: &str,
    policy_id: &str,
) -> Result<(), Failure<O>> {
    let policy = guard.policy(policy_id).await?;
    if guard.privileged(&policy.statements)? {
        protection_error(
            op,
            Protection::Privileged,
            "the policy grants an IAM write action",
        )?;
    }
    Ok(())
}
