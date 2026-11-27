use serde::Serialize;

use super::wire::{Screen, ScreenId};
use super::{
    Action, Api, Client, DEFAULT_CAP, Group, GroupSummary, LIST_CAP, OBJECT_CAP, Page, PageQuery,
    Policy, PolicySummary, id_input, page_input, read_operation, require_identity,
};
use crate::wire::product::evidence::Evidence;
use crate::{Failure, List, RequestContext};

/// Inputs for reading the IAM action catalog.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ListActionsInput;

crate::error::debug_record!(ListActionsInput unit);

/// The IAM action catalog.
pub type ListActionsOutput = List<Action>;

/// Inputs for reading the IAM groups.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ListGroupsInput;

crate::error::debug_record!(ListGroupsInput unit);

/// The IAM groups visible to the caller.
pub type ListGroupsOutput = List<GroupSummary>;

page_input!(
    /// Inputs for listing IAM policies.
    ListPoliciesInput,
    /// Optional policy name filter.
    name,
    filtered
);

/// One page of IAM policies.
pub type ListPoliciesOutput = Page<PolicySummary>;

id_input!(
    /// Inputs for reading one IAM policy.
    GetPolicyInput,
    /// IAM policy ID.
    policy_id
);

/// The requested IAM policy and its statements.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetPolicyOutput {
    /// Policy details and statement document.
    #[serde(rename = "Policy")]
    pub policy: Policy,
}

crate::wire::product::output::public_fields!(GetPolicyOutput { policy });

id_input!(
    /// Inputs for reading the principals attached to one policy.
    ListPolicyAttachmentsInput,
    /// IAM policy ID.
    policy_id
);

/// Groups, users, and service accounts attached to one policy.
#[derive(Clone, PartialEq, Serialize)]
pub struct ListPolicyAttachmentsOutput {
    /// Groups attached to the policy.
    #[serde(rename = "Groups")]
    pub groups: Vec<GroupSummary>,
    /// IDs of users attached to the policy.
    #[serde(rename = "UserIDs")]
    pub user_ids: Vec<String>,
    /// IDs of service accounts attached to the policy.
    #[serde(rename = "ServiceAccountIDs")]
    pub service_account_ids: Vec<String>,
}

crate::wire::product::output::public_fields!(ListPolicyAttachmentsOutput {
    groups,
    user_ids,
    service_account_ids
});

id_input!(
    /// Inputs for reading one IAM group.
    GetGroupInput,
    /// IAM group ID.
    group_id
);

/// The requested IAM group and its members and policies.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetGroupOutput {
    /// Group details.
    #[serde(rename = "Group")]
    pub group: Group,
}

crate::wire::product::output::public_fields!(GetGroupOutput { group });

id_input!(
    /// Inputs for reading the groups that contain one user.
    ListUserGroupsInput,
    /// IAM user ID.
    user_id
);

/// The groups that contain one IAM user.
pub type ListUserGroupsOutput = List<Group>;

page_input!(
    /// Inputs for listing the policies attached to one IAM group.
    ListGroupPoliciesInput,
    /// IAM group ID.
    group_id,
    required
);

/// One page of policies attached to an IAM group.
pub type ListGroupPoliciesOutput = Page<PolicySummary>;

page_input!(
    /// Inputs for listing the policies attached to one IAM user.
    ListUserPoliciesInput,
    /// IAM user ID.
    user_id,
    required
);

/// One page of policies attached to an IAM user.
pub type ListUserPoliciesOutput = Page<PolicySummary>;

page_input!(
    /// Inputs for listing the policies attached to one service account.
    ListServiceAccountPoliciesInput,
    /// Service-account resource ID.
    service_account_id,
    required
);

/// One page of policies attached to a service account.
pub type ListServiceAccountPoliciesOutput = Page<PolicySummary>;

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Lists supported IAM actions and their resource forms.
    pub async fn list_actions(
        &self,
        ctx: &RequestContext,
        _: ListActionsInput,
    ) -> Result<ListActionsOutput, Failure<ListActionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.ListActions",
                "policies-api/v1/actions",
                LIST_CAP,
                false,
            );
            let items = self
                .read(ctx, Api::Policies, &operation, &[], &[("product", "iam")])
                .await?;
            Ok(List { items })
        })
    }

    /// Lists one zero-based page of policies with an optional name filter.
    pub async fn list_policies(
        &self,
        ctx: &RequestContext,
        input: ListPoliciesInput,
    ) -> Result<ListPoliciesOutput, Failure<ListPoliciesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.ListPolicies",
                "policies-api/v1/policies",
                DEFAULT_CAP,
                true,
            );
            let page = PageQuery::new(input.page, input.size);
            self.read(
                ctx,
                Api::Policies,
                &operation,
                &[],
                &page.pairs(&input.name),
            )
            .await
        })
    }

    /// Reads and verifies the identity of one policy.
    pub async fn get_policy(
        &self,
        ctx: &RequestContext,
        input: GetPolicyInput,
    ) -> Result<GetPolicyOutput, Failure<GetPolicyOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.GetPolicy",
                "policies-api/v1/policies/{}",
                OBJECT_CAP,
                false,
            );
            let (id, mut evidence) = (input.policy_id.as_str(), Evidence::new());
            let mut policy: Policy = self
                .fetch(
                    ctx,
                    Api::Policies,
                    &operation,
                    &[("PolicyID", id)],
                    &[],
                    &mut evidence,
                )
                .await?;
            require_identity(&policy.id, id, "iam.GetPolicy: policy identity is unproven")?;
            policy.screen(&evidence);
            super::screened_output(GetPolicyOutput { policy }, &evidence)
                .map_err(|error| super::failure(*error))
        })
    }

    /// Lists the groups, users, and service accounts attached to one policy.
    pub async fn list_policy_attachments(
        &self,
        ctx: &RequestContext,
        input: ListPolicyAttachmentsInput,
    ) -> Result<ListPolicyAttachmentsOutput, Failure<ListPolicyAttachmentsOutput>> {
        crate::wire::product::exit::finish!(async {
            const OP: &str = "iam.ListPolicyAttachments";
            let mut evidence = Evidence::new();
            let ids = &[("PolicyID", input.policy_id.as_str())];
            let mut groups: Vec<GroupSummary> = self
                .fetch(
                    ctx,
                    Api::Policies,
                    &read_operation(OP, "policies-api/v1/policies/{}/groups", DEFAULT_CAP, true),
                    ids,
                    &[],
                    &mut evidence,
                )
                .await?;
            let mut user_ids: Vec<String> = self
                .fetch(
                    ctx,
                    Api::Policies,
                    &read_operation(
                        OP,
                        "policies-api/v1/policies/{}/iam-users",
                        DEFAULT_CAP,
                        true,
                    ),
                    ids,
                    &[],
                    &mut evidence,
                )
                .await?;
            let mut service_account_ids: Vec<String> = self
                .fetch(
                    ctx,
                    Api::Policies,
                    &read_operation(
                        OP,
                        "policies-api/v1/policies/{}/service-accounts",
                        DEFAULT_CAP,
                        true,
                    ),
                    ids,
                    &[],
                    &mut evidence,
                )
                .await?;
            groups.screen(&evidence);
            user_ids.screen_id(&evidence);
            service_account_ids.screen_id(&evidence);
            super::screened_output(
                ListPolicyAttachmentsOutput {
                    groups,
                    user_ids,
                    service_account_ids,
                },
                &evidence,
            )
            .map_err(|error| super::failure(*error))
        })
    }

    /// Lists IAM groups visible to the caller.
    pub async fn list_groups(
        &self,
        ctx: &RequestContext,
        _: ListGroupsInput,
    ) -> Result<ListGroupsOutput, Failure<ListGroupsOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.ListGroups",
                "policies-api/v1/groups",
                DEFAULT_CAP,
                true,
            );
            let items = self.read(ctx, Api::Policies, &operation, &[], &[]).await?;
            Ok(List { items })
        })
    }

    /// Reads and verifies the identity of one group.
    pub async fn get_group(
        &self,
        ctx: &RequestContext,
        input: GetGroupInput,
    ) -> Result<GetGroupOutput, Failure<GetGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            let group = self
                .fetch_group(ctx, &input.group_id, false, &mut Evidence::new())
                .await?;
            Ok(GetGroupOutput { group })
        })
    }

    /// Lists the groups containing an IAM user.
    pub async fn list_user_groups(
        &self,
        ctx: &RequestContext,
        input: ListUserGroupsInput,
    ) -> Result<ListUserGroupsOutput, Failure<ListUserGroupsOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.ListUserGroups",
                "policies-api/v1/user-attachments/iam-users/{}/groups",
                DEFAULT_CAP,
                true,
            );
            let items = self
                .read(
                    ctx,
                    Api::Policies,
                    &operation,
                    &[("UserID", &input.user_id)],
                    &[],
                )
                .await?;
            Ok(List { items })
        })
    }

    /// Lists one zero-based page of policies attached to an IAM group.
    pub async fn list_group_policies(
        &self,
        ctx: &RequestContext,
        input: ListGroupPoliciesInput,
    ) -> Result<ListGroupPoliciesOutput, Failure<ListGroupPoliciesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.ListGroupPolicies",
                "policies-api/v1/groups/{}/policies",
                DEFAULT_CAP,
                true,
            );
            let page = PageQuery::new(input.page, input.size);
            self.read(
                ctx,
                Api::Policies,
                &operation,
                &[("GroupID", &input.group_id)],
                &page.pairs(""),
            )
            .await
        })
    }

    /// Lists one zero-based page of policies attached to an IAM user.
    pub async fn list_user_policies(
        &self,
        ctx: &RequestContext,
        input: ListUserPoliciesInput,
    ) -> Result<ListUserPoliciesOutput, Failure<ListUserPoliciesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.ListUserPolicies",
                "policies-api/v1/user-attachments/iam-users/{}/policies",
                DEFAULT_CAP,
                true,
            );
            let page = PageQuery::new(input.page, input.size);
            self.read(
                ctx,
                Api::Policies,
                &operation,
                &[("UserID", &input.user_id)],
                &page.pairs(""),
            )
            .await
        })
    }

    /// Lists one zero-based page of policies attached to a service account.
    pub async fn list_service_account_policies(
        &self,
        ctx: &RequestContext,
        input: ListServiceAccountPoliciesInput,
    ) -> Result<ListServiceAccountPoliciesOutput, Failure<ListServiceAccountPoliciesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "iam.ListServiceAccountPolicies",
                "policies-api/v1/user-attachments/service-accounts/{}/policies",
                DEFAULT_CAP,
                true,
            );
            let page = PageQuery::new(input.page, input.size);
            self.read(
                ctx,
                Api::Policies,
                &operation,
                &[("ServiceAccountID", &input.service_account_id)],
                &page.pairs(""),
            )
            .await
        })
    }
}
