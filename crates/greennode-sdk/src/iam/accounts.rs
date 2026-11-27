use serde::Serialize;

use super::wire::Screen;
use super::{
    Api, Client, GetCallerIdentityOutput, LIST_CAP, OBJECT_CAP, Page, PageQuery, ServiceAccount,
    User, UserDetail, id_input, invalid_input, page_input, read_operation, refusal,
    require_identity,
};
use crate::wire::product::evidence::Evidence;
use crate::{ErrorClass, Failure, RequestContext};

/// Inputs for reading the current caller's identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GetCallerIdentityInput;

id_input!(
    /// Inputs for reading one IAM user.
    GetUserInput,
    /// User ID from the IAM accounts API.
    user_id
);

/// Details of the requested IAM user.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetUserOutput {
    /// Safe user details and MFA state.
    #[serde(rename = "User")]
    pub user: UserDetail,
}

page_input!(
    /// Inputs for listing IAM users.
    ListUsersInput,
    /// Optional substring of the user name.
    name,
    filtered
);

/// One page of IAM users.
pub type ListUsersOutput = Page<User>;

id_input!(
    /// Inputs for reading one service account.
    GetServiceAccountInput,
    /// Service-account resource ID.
    service_account_id
);

/// Details of the requested service account.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetServiceAccountOutput {
    /// Service-account metadata.
    #[serde(rename = "ServiceAccount")]
    pub service_account: ServiceAccount,
}

page_input!(
    /// Inputs for listing IAM service accounts.
    ListServiceAccountsInput,
    /// Optional service-account name filter.
    name,
    filtered
);

/// One page of IAM service accounts.
pub type ListServiceAccountsOutput = Page<ServiceAccount>;

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Reads the identity associated with the current sign-in.
    pub async fn get_caller_identity(
        &self,
        ctx: &RequestContext,
        _: GetCallerIdentityInput,
    ) -> Result<GetCallerIdentityOutput, Failure<GetCallerIdentityOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.GetCallerIdentity",
                "accounts-api/v1/auth/userinfo",
                OBJECT_CAP,
                false,
            );
            self.read(ctx, Api::Accounts, &operation, &[], &[]).await
        })
    }

    /// Reads one IAM user, retaining MFA state and withholding enrollment material.
    pub async fn get_user(
        &self,
        ctx: &RequestContext,
        input: GetUserInput,
    ) -> Result<GetUserOutput, Failure<GetUserOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = super::Operation {
                sensitive: true,
                ..read_operation(
                    "iam.GetUser",
                    "accounts-api/v1/iam-users/{}",
                    OBJECT_CAP,
                    false,
                )
            };
            let (id, mut evidence) = (input.user_id.as_str(), Evidence::new());
            let user: Option<UserDetail> = self
                .fetch(
                    ctx,
                    Api::Accounts,
                    &operation,
                    &[("UserID", id)],
                    &[],
                    &mut evidence,
                )
                .await?;
            let mut user = user.ok_or_else(|| {
                refusal(ErrorClass::Api, "", "iam: iam.GetUser: missing user object")
            })?;
            user.screen(&evidence);
            super::screened_output(GetUserOutput { user }, &evidence)
                .map_err(|error| super::failure(*error))
        })
    }

    /// Lists one zero-based page of IAM users and optionally filters by name.
    pub async fn list_users(
        &self,
        ctx: &RequestContext,
        input: ListUsersInput,
    ) -> Result<ListUsersOutput, Failure<ListUsersOutput>> {
        crate::wire::product::exit::finish!(async {
            const OP: &str = "iam.ListUsers";
            if input.name.len() > 4096 {
                return Err(invalid_input(format!(
                    "{OP}: Name must be valid UTF-8 and at most 4096 bytes"
                )));
            }
            if input
                .name
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
            {
                return Err(invalid_input(format!(
                    "{OP}: Name must not contain whitespace or controls"
                )));
            }
            let operation = read_operation(
                OP,
                "accounts-api/v1/iam-users",
                LIST_CAP,
                input.name.is_empty(),
            );
            let page = PageQuery::new(input.page, input.size);
            self.read(
                ctx,
                Api::Accounts,
                &operation,
                &[],
                &page.pairs(&input.name),
            )
            .await
        })
    }

    /// Reads and verifies the identity of one service account.
    pub async fn get_service_account(
        &self,
        ctx: &RequestContext,
        input: GetServiceAccountInput,
    ) -> Result<GetServiceAccountOutput, Failure<GetServiceAccountOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = super::Operation {
                sensitive: true,
                ..read_operation(
                    "iam.GetServiceAccount",
                    "accounts-api/v1/service-accounts/{}",
                    OBJECT_CAP,
                    false,
                )
            };
            let (id, mut evidence) = (input.service_account_id.as_str(), Evidence::new());
            let mut account: ServiceAccount = self
                .fetch(
                    ctx,
                    Api::Accounts,
                    &operation,
                    &[("ServiceAccountID", id)],
                    &[],
                    &mut evidence,
                )
                .await?;
            require_identity(
                &account.id,
                id,
                "iam.GetServiceAccount: service account identity is unproven",
            )?;
            account.screen(&evidence);
            super::screened_output(
                GetServiceAccountOutput {
                    service_account: account,
                },
                &evidence,
            )
            .map_err(|error| super::failure(*error))
        })
    }

    /// Lists one zero-based page of service accounts and optionally filters by name.
    pub async fn list_service_accounts(
        &self,
        ctx: &RequestContext,
        input: ListServiceAccountsInput,
    ) -> Result<ListServiceAccountsOutput, Failure<ListServiceAccountsOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = super::Operation {
                sensitive: true,
                ..read_operation(
                    "iam.ListServiceAccounts",
                    "accounts-api/v1/service-accounts",
                    LIST_CAP,
                    false,
                )
            };
            let page = PageQuery::new(input.page, input.size);
            self.read(
                ctx,
                Api::Accounts,
                &operation,
                &[],
                &page.pairs(&input.name),
            )
            .await
        })
    }
}
