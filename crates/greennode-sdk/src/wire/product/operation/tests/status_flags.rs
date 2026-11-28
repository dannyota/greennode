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
    ("network.AddNetworkACLRule", "PUT", NO_FOLLOW),
    ("network.AddRoute", "PUT", NO_FOLLOW),
    ("network.AssociateNetworkACLSubnet", "PUT", NO_FOLLOW),
    ("network.CreateEndpoint", "GET", KEEP | NO_FOLLOW),
    ("network.CreateEndpoint", "POST", KEEP | NO_FOLLOW),
    ("network.CreateNATInstance", "GET", NO_FOLLOW),
    ("network.CreateNATInstance", "POST", NO_FOLLOW),
    ("network.DeleteEndpoint", "DELETE", KEEP | NO_FOLLOW),
    ("network.DeleteEndpoint", "GET", KEEP | NO_FOLLOW),
    ("network.DeleteNATInstance", "DELETE", NO_FOLLOW),
    ("network.DeleteNATInstance", "GET", NO_FOLLOW),
    ("network.DeleteNetworkACL", "DELETE", NO_FOLLOW),
    ("network.DisassociateNetworkACLSubnet", "PUT", NO_FOLLOW),
    ("network.EnableVPCPrivateDNS", "PATCH", NO_FOLLOW),
    ("network.RemoveNetworkACLRule", "PUT", NO_FOLLOW),
    ("network.RemoveRoute", "PUT", NO_FOLLOW),
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
        ("billingresources.GetAccount", "GET") => KEEP | NO_FOLLOW,
        (_, "PUT") if operation.template == "gateway/api/v1/resources/autoRenew" => {
            KEEP | NO_FOLLOW
        }
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

#[test]
fn sdk_status_decisions_use_private_response_evidence() {
    use regex_lite::Regex;
    use std::collections::BTreeMap;
    use std::path::Path;

    let diagnostics = [
        ("billing/budget_writes.rs", "reconcile", 1),
        ("billing/history/security.rs", "safe_request_error", 2),
        ("billing_resources/mod.rs", "screened_sign_in", 2),
        ("containerregistry/mod.rs", "create_failure", 1),
        ("containerregistry/mod.rs", "redact_failure", 1),
        ("containerregistry/user_writes.rs", "create_user", 1),
        ("database/write.rs", "preflight", 1),
        ("database/write.rs", "safe_rejection", 1),
        ("dns/writes/mod.rs", "create_failure", 1),
        ("error/display.rs", "fmt", 3),
        ("error/provenance.rs", "response_status", 1),
        ("iam/mod.rs", "screen_error", 1),
        ("iam/wire.rs", "account_error", 2),
        ("monitor/quota/status.rs", "unavailable_row", 1),
        ("monitor/quota/wire.rs", "get_status", 1),
        ("network/endpoint_quote.rs", "endpoint_error", 1),
        ("network/endpoint_quote.rs", "endpoint_interruption", 1),
        ("network/mutations.rs", "error_description", 1),
        ("network/wire.rs", "nat_safe_error", 1),
        ("portal/envelope.rs", "withheld", 1),
        ("pricing/failure.rs", "row", 1),
        ("vks/mod.rs", "safe_error", 2),
        ("wire/product/absence.rs", "keep_not_found_diagnostic", 1),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let declarations = Regex::new(concat!(
        r"(?m)^(?<attributes>(?:[ \t]*#\[[^\n]*\][ \t]*\n)*)[ \t]*",
        r"(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe)\s+)*fn\s+(?<name>\w+)"
    ))
    .unwrap();
    let test_attribute = Regex::new(r"#\[(?:\w+::)*test(?:\(|\])").unwrap();
    let status = Regex::new(concat!(
        r"\.\s*(?:response_)?status\s*\(\s*\)",
        r"|\bApiError\s*::\s*(?:response_)?status\b"
    ))
    .unwrap();
    let mut pending = vec![root.clone()];
    let mut found = BTreeMap::new();
    let mut unexpected = Vec::new();
    while let Some(file) = pending.pop() {
        if file.is_dir() {
            pending.extend(file.read_dir().unwrap().map(|entry| entry.unwrap().path()));
            continue;
        }
        let path = file.strip_prefix(&root).unwrap();
        if file.extension().is_none_or(|ext| ext != "rs")
            || path.iter().any(|part| part == "tests")
            || path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("tests.rs"))
            || path == Path::new("quote/catalog/corpus.rs")
        {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap();
        let mut functions = declarations.captures_iter(&source).peekable();
        let mut function = "";
        let mut is_test = false;
        for call in status.find_iter(&source) {
            while functions
                .peek()
                .is_some_and(|declaration| declaration.get(0).unwrap().start() < call.start())
            {
                let declaration = functions.next().unwrap();
                function = declaration.name("name").unwrap().as_str();
                is_test = test_attribute.is_match(&declaration["attributes"]);
            }
            if is_test {
                continue;
            }
            if path == Path::new("error/parts.rs")
                && source[..call.start()].ends_with(r#".field("status", &self"#)
            {
                continue;
            }
            let expected = diagnostics
                .iter()
                .find(|&&(file, name, _)| path == Path::new(file) && name == function);
            let permitted = if let Some(&(file, name, expected)) = expected {
                let count = found.entry((file, name)).or_insert(0_usize);
                *count += 1;
                *count <= expected
            } else {
                false
            };
            if !permitted {
                let line = source[..call.start()]
                    .bytes()
                    .filter(|&byte| byte == b'\n')
                    .count()
                    + 1;
                unexpected.push(format!("{}:{line} ({function})", path.display()));
            }
        }
    }
    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "public status used outside diagnostics at {}",
        unexpected.join(", ")
    );
}
