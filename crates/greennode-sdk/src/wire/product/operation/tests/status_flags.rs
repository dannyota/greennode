use super::{Operation, Options};

const KEEP: u8 = 1;
const OVERFLOW: u8 = 2;
const NO_REDIRECT: u8 = 4;
const REPORT_REDIRECT: u8 = 8;
const RAW: u8 = 16;
const NO_FOLLOW: u8 = NO_REDIRECT | REPORT_REDIRECT;

const GO_FLAGS: &[(&str, &str, u8)] = &[
    ("DeleteDatabaseInstance", "GET", NO_FOLLOW),
    ("GetDatabaseInstance", "GET", NO_FOLLOW),
    ("ListDatabaseEngineVersions", "GET", NO_FOLLOW),
    ("ListDatabaseFlavors", "GET", NO_FOLLOW),
    ("ListDatabaseInstances", "GET", NO_FOLLOW),
    ("ListDatabaseNetworks", "GET", NO_FOLLOW),
    ("ListDatabaseStorageTypes", "GET", NO_FOLLOW),
    ("ListDatabaseZones", "GET", NO_FOLLOW),
    ("billing.CreateBudget", "POST", KEEP | NO_FOLLOW),
    ("billing.CreateBudgetThreshold", "POST", KEEP | NO_FOLLOW),
    ("billing.DeleteBudget", "DELETE", KEEP | NO_FOLLOW),
    ("billing.DeleteBudgetThreshold", "DELETE", KEEP | NO_FOLLOW),
    ("billing.GetBalances", "GET", KEEP),
    ("billing.GetBudget", "GET", KEEP),
    ("billing.GetCostOverview", "GET", KEEP),
    ("billing.GetCurrentPeriodCost", "GET", KEEP),
    ("billing.ListBudgetAlerts", "GET", KEEP),
    ("billing.ListBudgetThresholds", "GET", KEEP),
    ("billing.ListBudgets", "GET", KEEP),
    ("billing.ListCostResources", "GET", KEEP),
    ("billing.ListPayments", "GET", KEEP | NO_FOLLOW),
    ("billing.ListResources", "GET", KEEP | NO_FOLLOW),
    ("billing.ListTransactions", "GET", KEEP | NO_FOLLOW),
    ("billing.UpdateBudget", "PUT", KEEP | NO_FOLLOW),
    ("billing.UpdateBudgetThreshold", "PUT", KEEP | NO_FOLLOW),
    ("cdn.DeleteWebAccelerator", "DELETE", REPORT_REDIRECT),
    ("cdn.DisableWebAccelerator", "PUT", REPORT_REDIRECT),
    ("cdn.EnableWebAccelerator", "PUT", REPORT_REDIRECT),
    ("cdn.ListIPRanges", "GET", RAW),
    ("cdn.UpdateWebAccelerator", "PUT", REPORT_REDIRECT),
    ("compute.CreateSSHKey", "POST", NO_FOLLOW),
    ("compute.CreateServer", "POST", NO_FOLLOW),
    ("compute.GetServer", "GET", NO_FOLLOW),
    ("compute.GetServerConsoleLog", "GET", OVERFLOW | NO_FOLLOW),
    ("compute.ImportSSHKey", "POST", NO_FOLLOW),
    ("compute.ListSSHKeys", "GET", NO_FOLLOW),
    ("compute.GetSSHKey", "GET", NO_FOLLOW),
    ("compute.ListServerGroups", "GET", NO_FOLLOW),
    ("compute.ListServers", "GET", NO_FOLLOW),
    ("compute.RebootServer", "PUT", NO_FOLLOW),
    ("compute.RenameServer", "PUT", NO_FOLLOW),
    ("compute.ResizeServer", "PUT", NO_FOLLOW),
    ("compute.StartServer", "PUT", NO_FOLLOW),
    ("compute.StopServer", "PUT", NO_FOLLOW),
    ("containerregistry.CreateUser", "POST", REPORT_REDIRECT),
    ("database.CreateDatabaseInstance", "POST", KEEP | NO_FOLLOW),
    ("database.DeleteDatabaseInstance", "POST", KEEP | NO_FOLLOW),
    ("dns.UpdateHostedZone", "PUT", KEEP | REPORT_REDIRECT),
    ("globalloadbalancer.CreateListener", "GET", NO_FOLLOW),
    (
        "globalloadbalancer.CreateListener",
        "POST",
        KEEP | NO_FOLLOW,
    ),
    (
        "globalloadbalancer.CreateLoadBalancer",
        "POST",
        KEEP | NO_FOLLOW,
    ),
    ("globalloadbalancer.CreatePool", "GET", NO_FOLLOW),
    ("globalloadbalancer.CreatePool", "POST", KEEP | NO_FOLLOW),
    (
        "globalloadbalancer.DeleteListener",
        "DELETE",
        KEEP | NO_FOLLOW,
    ),
    ("globalloadbalancer.DeleteListener", "GET", NO_FOLLOW),
    (
        "globalloadbalancer.DeleteLoadBalancer",
        "DELETE",
        KEEP | NO_FOLLOW,
    ),
    ("globalloadbalancer.DeletePool", "DELETE", KEEP | NO_FOLLOW),
    ("globalloadbalancer.DeletePool", "GET", NO_FOLLOW),
    ("globalloadbalancer.WriteDetail", "GET", NO_FOLLOW),
    ("iam.CreateGroup", "POST", REPORT_REDIRECT),
    ("iam.CreatePolicy", "POST", REPORT_REDIRECT),
    ("iam.CreateServiceAccount", "POST", REPORT_REDIRECT),
    ("iam.CreateUser", "POST", KEEP | REPORT_REDIRECT),
    ("iam.DeleteUser", "DELETE", NO_FOLLOW),
    ("iam.DisableUserMFA", "POST", KEEP | REPORT_REDIRECT),
    ("iam.EnableUserMFA", "POST", KEEP | REPORT_REDIRECT),
    ("iam.ResetServiceAccountSecret", "POST", REPORT_REDIRECT),
    ("iam.ResetUserPassword", "POST", KEEP | REPORT_REDIRECT),
    ("iam.UpdatePolicy", "PUT", KEEP | REPORT_REDIRECT),
    ("iam.UpdateServiceAccount", "PATCH", KEEP),
    ("loadbalancer.AddPoolMember", "PUT", REPORT_REDIRECT),
    ("loadbalancer.CreateLoadBalancer", "POST", REPORT_REDIRECT),
    ("loadbalancer.RemovePoolMember", "PUT", REPORT_REDIRECT),
    ("loadbalancer.ResizeLoadBalancer", "PUT", REPORT_REDIRECT),
    ("loadbalancer.UpdatePoolMember", "PUT", REPORT_REDIRECT),
    ("monitor.GetDashboard", "GET", NO_FOLLOW),
    ("monitor.ListDashboards", "GET", NO_FOLLOW),
    ("monitor.ListSyntheticPackages", "GET", NO_FOLLOW),
    ("network.GetNATInstance", "GET", KEEP | OVERFLOW | NO_FOLLOW),
    ("network.GetNetworkACL", "GET", NO_FOLLOW),
    ("network.GetRouteTable", "GET", NO_FOLLOW),
    ("network.GetSecurityGroup", "GET", NO_FOLLOW),
    ("network.ListBandwidthPackages", "GET", OVERFLOW | NO_FOLLOW),
    (
        "network.ListNATInboundRules",
        "GET",
        KEEP | OVERFLOW | NO_FOLLOW,
    ),
    ("network.ListNATPackages", "GET", NO_FOLLOW),
    ("network.ListNATZones", "GET", NO_FOLLOW),
    ("network.ListNetworkACLs", "GET", NO_FOLLOW),
    ("network.ListSecurityGroupRules", "GET", NO_FOLLOW),
    ("network.ListSecurityGroups", "GET", NO_FOLLOW),
    ("network.ListServersBySecurityGroup", "GET", NO_FOLLOW),
    ("network.ListVNetworkRegions", "GET", NO_FOLLOW),
    ("network.QuoteCreateEndpoint", "GET", KEEP | NO_FOLLOW),
    ("network.QuoteCreateEndpoint", "POST", KEEP | NO_FOLLOW),
    ("network.QuoteCreateNATInstance", "GET", NO_FOLLOW),
    ("network.QuoteCreateNATInstance", "POST", NO_FOLLOW),
    ("portal.GetTagQuota", "GET", NO_FOLLOW),
    ("portal.GetUserInfo", "GET", NO_FOLLOW),
    ("portal.ListQuotaUsed", "GET", NO_FOLLOW),
    ("portal.ListZones", "GET", NO_FOLLOW),
    ("pricing.GetQuote", "POST", KEEP | NO_FOLLOW),
    ("project.ListProjects", "GET", NO_FOLLOW),
    ("tagging.TagResource", "PUT", KEEP | REPORT_REDIRECT),
    ("tagging.UntagResource", "PUT", KEEP | REPORT_REDIRECT),
    ("vks.CreateCluster", "POST", NO_FOLLOW | RAW),
    ("vks.DeleteCluster", "DELETE", NO_FOLLOW),
    ("vks.GetCluster", "GET", NO_FOLLOW),
    ("volume.CreateVolume", "POST", REPORT_REDIRECT),
    ("volume.ResizeVolume", "PUT", REPORT_REDIRECT),
];

pub(crate) fn assert_matches_go(operation: &Operation, options: &Options<'_>) {
    let id = operation.id;
    let method = operation.method.as_str();
    if id.starts_with("test.") {
        return;
    }
    let declared = declared_flags(operation);
    let expected = match (id, method) {
        _ if options.pacing.is_some() => NO_FOLLOW,
        ("loadbalancer.CreateLoadBalancer" | "loadbalancer.ResizeLoadBalancer", "POST")
            if operation.template == "v1/price" =>
        {
            0
        }
        ("network.ListVNetworkRegions", "GET") if operation.cap == 4 << 20 => return,
        (
            "monitor.QuoteCreateSyntheticQuota"
            | "monitor.QuoteRenewSyntheticQuota"
            | "monitor.GetQuotaStatus"
            | "monitor.GetNotificationQuota",
            _,
        ) => monitor_flags(id, method, operation.template),
        _ => GO_FLAGS
            .iter()
            .find(|&&(op, verb, _)| (op, verb) == (id, method))
            .map_or(0, |&(_, _, flags)| flags),
    };
    assert_eq!(declared, expected, "{id} {method} {}", operation.template);
}

pub(crate) fn assert_nat_region_matches_go(operation: &Operation, detail_lookup: bool) {
    let expected = NO_FOLLOW | if detail_lookup { KEEP | OVERFLOW } else { 0 };
    assert_eq!(
        declared_flags(operation),
        expected,
        "{} {}",
        operation.id,
        operation.template
    );
}

fn declared_flags(operation: &Operation) -> u8 {
    [
        (operation.keep_status, KEEP),
        (operation.overflow_overrides_status, OVERFLOW),
        (operation.no_redirect, NO_REDIRECT),
        (operation.report_redirect, REPORT_REDIRECT),
        (operation.raw_body, RAW),
    ]
    .into_iter()
    .filter_map(|(set, flag)| set.then_some(flag))
    .fold(0, |flags, flag| flags | flag)
}

fn monitor_flags(id: &str, method: &str, route: &str) -> u8 {
    match (id, method, route) {
        (
            "monitor.QuoteCreateSyntheticQuota"
            | "monitor.QuoteRenewSyntheticQuota"
            | "monitor.GetQuotaStatus",
            "GET",
            "billing-api/v2/synthetic/quota-class"
            | "billing-api/v2/synthetic/quota-class/{}/packages"
            | "billing-api/v1/settings",
        )
        | ("monitor.QuoteCreateSyntheticQuota", "GET", "billing-api/v1/synthetic/quota")
        | (
            "monitor.QuoteCreateSyntheticQuota",
            "POST",
            "billing-api/v2/synthetic/prices/created-price",
        )
        | (
            "monitor.GetQuotaStatus",
            "GET",
            "billing-api/v1/synthetic/quota"
            | "billing-api/v2/synthetic/{}/quota-detail"
            | "billing-api/v1/email/quota"
            | "billing-api/v1/sms/quota"
            | "billing-api/v1/email/packages"
            | "billing-api/v1/sms/packages",
        ) => NO_FOLLOW,
        (
            "monitor.QuoteRenewSyntheticQuota",
            "GET",
            "billing-api/v1/synthetic/quota" | "billing-api/v2/synthetic/{}/quota-detail",
        )
        | (
            "monitor.QuoteRenewSyntheticQuota",
            "POST",
            "billing-api/v2/synthetic/prices/created-price",
        )
        | (
            "monitor.GetNotificationQuota",
            "GET",
            "billing-api/v1/email/quota" | "billing-api/v1/sms/quota",
        ) => 0,
        _ => panic!("unlisted policy for {id} {method} {route}"),
    }
}

#[test]
fn go_operation_flag_table_has_unique_rows() {
    let keys: std::collections::BTreeSet<_> = GO_FLAGS
        .iter()
        .map(|&(id, method, _)| (id, method))
        .collect();
    assert_eq!(keys.len(), GO_FLAGS.len());
}

#[test]
#[should_panic(expected = "monitor.QuoteCreateSyntheticQuota GET")]
fn monitor_catalog_guard_cannot_use_the_post_quote_redirect_policy() {
    let operation = Operation {
        id: "monitor.QuoteCreateSyntheticQuota",
        template: "billing-api/v2/synthetic/quota-class",
        ..super::declaration()
    };
    assert_matches_go(&operation, &Options::DEFAULT);
}
