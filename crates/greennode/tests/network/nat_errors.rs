use super::*;
use greennode_sdk::network::{
    Client, GetNatInstanceInput, ListNatInboundRulesInput, ListNatInstancesInput,
    ListNatPackagesInput, ListNatZonesInput, QuoteCreateNatInstanceInput,
};
use greennode_sdk::{Clock, Error, ErrorClass};
use tokio::sync::oneshot;

#[path = "nat_error_fixture.rs"]
mod fixture;
use fixture::{Observed, run_case};
#[path = "nat_error_shape.rs"]
mod shape;
use shape::expected_sdk;

fn region(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::UnsupportedRegion => "sgn-1",
        _ => "hcm-3",
    }
}

fn options() -> ConfigOptions {
    ConfigOptions::default().with_region("hcm-3".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Zones,
    Packages,
    Instances,
    Detail,
    Rules,
    Quote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Project,
    Region,
    Whitelist,
    AvailabilityZone,
    Package,
    Vpc,
    Final,
}

#[derive(Clone, Copy, Debug)]
enum Outcome {
    Truncated,
    Oversized,
    OversizedHeader,
    Created,
    Empty,
    Ambiguous,
    Cancelled,
    Deadline,
    Status(u16),
    SafeCode,
    UnsafeCode,
    WithheldNotFound,
    CancelledInvalidation,
    PeerInvalidResponse,
    Login,
    TokenCache,
    InvalidPath,
    NegativePage,
    UnsupportedRegion,
}

const PROJECT: &str = r#"{"projects":[{"projectId":"proj-1","region":"hcm-3"}]}"#;
const REGION: &str = r#"{"success":true,"data":[{"uuid":"zone-1","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#;
const ZONES: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/network/list_nat_zones.json");
const PACKAGES: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/network/list_nat_packages.json");
const INSTANCES: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/network/list_nat_instances.json");
const RULES: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/network/list_nat_inbound_rules.json");
const VPCS: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/network/nat_quote_vpcs.json");
const WHITELIST: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/network/nat_quote_whitelist.json");
const PROJECT_ERROR: &str = "vngcloud: no project found for region";
const PROJECT_AMBIGUOUS: &str = "vngcloud: multiple projects found for region";
const REGION_ERROR: &str = "vngcloud: invalid config: no unique NAT zone for selected region";
const WITHHELD: &str = "NAT response withheld";
const INVALID: &str = "NAT response had invalid structure";
const PROVIDER_CODE: &str = "CatalogDenied";
const PROVIDER_MESSAGE: &str = "provider refused";

type Report = (String, Option<u16>, String, String);

impl Target {
    fn operation(self) -> &'static str {
        match self {
            Self::Zones => "network.ListNATZones",
            Self::Packages => "network.ListNATPackages",
            Self::Instances => "network.ListNATInstances",
            Self::Detail => "network.GetNATInstance",
            Self::Rules => "network.ListNATInboundRules",
            Self::Quote => "network.QuoteCreateNATInstance",
        }
    }

    fn stages(self) -> &'static [Stage] {
        match self {
            Self::Quote => &[
                Stage::Project,
                Stage::Whitelist,
                Stage::AvailabilityZone,
                Stage::Package,
                Stage::Vpc,
                Stage::Final,
            ],
            _ => &[Stage::Project, Stage::Region, Stage::Final],
        }
    }

    async fn call(self, client: &Client, ctx: &RequestContext, outcome: Outcome) -> Option<Error> {
        macro_rules! run {
            ($call:expr) => {
                $call.await.err().map(|failure| failure.into_parts().0)
            };
        }
        if matches!(outcome, Outcome::InvalidPath) {
            return match self {
                Self::Detail => {
                    run!(client.get_nat_instance(ctx, GetNatInstanceInput::new("not/valid")))
                }
                Self::Zones => run!(client.list_nat_zones(
                    ctx,
                    ListNatZonesInput {
                        zone_id: "not/valid".into()
                    }
                )),
                Self::Packages => {
                    run!(client.list_nat_packages(ctx, ListNatPackagesInput::new("not/valid")))
                }
                Self::Quote => run!(client.quote_create_nat_instance(
                    ctx,
                    QuoteCreateNatInstanceInput::new(
                        "edge",
                        "zone-1",
                        "az-1",
                        "not/valid",
                        "vpc-1"
                    )
                )),
                _ => unreachable!(),
            };
        }
        if matches!(outcome, Outcome::NegativePage) {
            return run!(client.list_nat_instances(
                ctx,
                ListNatInstancesInput {
                    page: -1,
                    ..Default::default()
                }
            ));
        }
        match self {
            Self::Zones => run!(client.list_nat_zones(ctx, ListNatZonesInput::default())),
            Self::Packages => {
                run!(client.list_nat_packages(ctx, ListNatPackagesInput::new("az-1")))
            }
            Self::Instances => {
                run!(client.list_nat_instances(ctx, ListNatInstancesInput::default()))
            }
            Self::Detail => run!(client.get_nat_instance(ctx, GetNatInstanceInput::new("nat-1"))),
            Self::Rules => {
                run!(client.list_nat_inbound_rules(ctx, ListNatInboundRulesInput::new("nat-1")))
            }
            Self::Quote => run!(client.quote_create_nat_instance(
                ctx,
                QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-1", "package-1", "vpc-1")
            )),
        }
    }

    async fn cli(
        self,
        opts: ConfigOptions,
        ctx: &RequestContext,
        clock: &dyn Clock,
        outcome: Outcome,
    ) -> (ExitCode, Vec<u8>) {
        let path = |valid| match outcome {
            Outcome::InvalidPath => "not/valid",
            _ => valid,
        };
        let (command, args): (&str, &[&str]) = match self {
            Self::Zones if matches!(outcome, Outcome::InvalidPath) => {
                ("list-nat-zones", &["--zone-id", "not/valid"])
            }
            Self::Instances if matches!(outcome, Outcome::NegativePage) => {
                ("list-nat-instances", &["--page=-1"])
            }
            Self::Packages => (
                "list-nat-packages",
                &["--availability-zone-id", path("az-1")],
            ),
            Self::Detail => ("get-nat-instance", &["--nat-id", path("nat-1")]),
            Self::Rules => ("list-nat-inbound-rules", &["--nat-id", "nat-1"]),
            Self::Quote => (
                "quote-create-nat-instance",
                &[
                    "--name",
                    "edge",
                    "--zone-id",
                    "zone-1",
                    "--availability-zone-id",
                    "az-1",
                    "--package-id",
                    path("package-1"),
                    "--vpc-id",
                    "vpc-1",
                ],
            ),
            Self::Zones => ("list-nat-zones", &[]),
            Self::Instances => ("list-nat-instances", &[]),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let status = run_invocation(Invocation {
            args: [
                "greennode",
                "--region",
                region(outcome),
                "--read-only",
                "network",
                command,
            ]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
            source: &Source,
            config_options: opts,
            stdin: &mut std::io::empty(),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal: false,
            stdout_terminal: false,
            stderr_terminal: false,
            clock,
            context: ctx,
        })
        .await;
        (status, stderr)
    }
}

#[derive(Clone, Copy)]
enum Baseline {
    SharedProject,
    CatalogProject,
    DetailProject,
    ListRegion,
    CatalogRegion,
    DetailRegion,
    List,
    Detail,
    Catalog,
}

fn expected(target: Target, stage: Stage, outcome: Outcome) -> Option<Report> {
    use Baseline::*;
    use Outcome::*;
    let caller = target.operation();
    let (baseline, operation) = match (target, stage) {
        (Target::Instances, Stage::Project) => (SharedProject, "project.ListProjects"),
        (Target::Detail | Target::Rules, Stage::Project) => (DetailProject, caller),
        (_, Stage::Project) => (CatalogProject, caller),
        (Target::Instances, Stage::Region) => (ListRegion, "network.ListVNetworkRegions"),
        (Target::Detail | Target::Rules, Stage::Region) => {
            (DetailRegion, "network.ListVNetworkRegions")
        }
        (_, Stage::Region) => (CatalogRegion, caller),
        (Target::Instances, Stage::Final) => (List, caller),
        (Target::Detail | Target::Rules, Stage::Final) => (Detail, caller),
        _ => (Catalog, caller),
    };
    let made = |id: &str, status, code: &str, message: &str| {
        Some((id.to_owned(), status, code.to_owned(), message.to_owned()))
    };
    let api = |status, code, message| made(operation, status, code, message);
    let bare = |code, message: &str| made("", None, code, message);
    let invalid = |message| api(Some(200), "InvalidResponse", message);
    let context = || bare("RequestFailed", &format!("{caller}: context canceled"));
    let transport = |cause| {
        made(
            operation,
            None,
            "",
            &format!("{operation}: request failed: {cause}"),
        )
    };
    let outcome = if matches!(outcome, OversizedHeader) {
        Oversized
    } else {
        outcome
    };
    match (baseline, outcome) {
        (_, Status(status)) => api(Some(status), http_identity(status).1, WITHHELD),
        (_, Deadline) => api(None, "", WITHHELD),
        (_, WithheldNotFound) => api(None, "", WITHHELD),
        (_, CancelledInvalidation) => api(Some(401), "Unauthorized", WITHHELD),
        (_, PeerInvalidResponse) => api(Some(403), "Forbidden", WITHHELD),
        (SharedProject, Login) => bare("LoginFailed", "signin page"),
        (_, InvalidPath) => bare(
            "InvalidUsage",
            match target {
                Target::Detail => {
                    "vngcloud: invalid input: network.GetNATInstance requires a path-safe NATID"
                }
                Target::Zones => {
                    "vngcloud: invalid input: network.ListNATZones requires ZoneID to match ^[A-Za-z0-9-]+$"
                }
                Target::Packages => {
                    "vngcloud: invalid input: network.ListNATPackages requires AvailabilityZoneID to match ^[A-Za-z0-9-]+$"
                }
                Target::Quote => {
                    "vngcloud: invalid input: network.QuoteCreateNATInstance requires PackageID to match ^[A-Za-z0-9-]+$"
                }
                _ => unreachable!(),
            },
        ),
        (_, NegativePage) => bare(
            "InvalidUsage",
            "vngcloud: invalid input: network.ListNATInstances requires nonnegative Page and Size",
        ),
        (_, UnsupportedRegion) => bare(
            "InvalidConfig",
            "vngcloud: invalid config: network.ListNATInstances supports hcm-3 and han-1",
        ),
        (_, Login) => bare("", &format!("{caller}: vngcloud: authentication failed")),
        (_, TokenCache) => bare("TokenCacheFailed", "vngcloud: token cache unavailable"),
        (SharedProject, Empty | Ambiguous) => {
            let (code, message) = project_selection(outcome);
            bare(code, &format!("{message}: hcm-3"))
        }
        (CatalogProject, Empty | Ambiguous) => {
            let (code, message) = project_selection(outcome);
            bare(code, &format!("{caller}: {message}"))
        }
        (DetailProject, Empty | Ambiguous) => {
            let (code, message) = project_selection(outcome);
            bare(code, &format!("{message}: {caller}"))
        }
        (CatalogRegion, Empty | Ambiguous) => bare(
            "InvalidConfig",
            &format!("{caller}: vngcloud: invalid config"),
        ),
        (ListRegion | DetailRegion, Empty | Ambiguous) => bare("InvalidConfig", REGION_ERROR),
        (List, Empty | Ambiguous) | (Detail, Empty | Ambiguous)
            if matches!(target, Target::Instances | Target::Rules) =>
        {
            None
        }
        (Detail, Empty | Ambiguous) => invalid(WITHHELD),
        (Catalog, Empty | Ambiguous) => match (target, stage, outcome) {
            (Target::Zones | Target::Packages, Stage::Final, _) => None,
            (Target::Quote, Stage::AvailabilityZone, _) => bare(
                "InvalidUsage",
                &format!("vngcloud: invalid input: {caller} requires one exact availability zone"),
            ),
            (Target::Quote, Stage::Package, _) => bare(
                "InvalidUsage",
                &format!(
                    "vngcloud: invalid input: {caller} requires one exact VND package in the availability zone"
                ),
            ),
            (Target::Quote, Stage::Vpc, Empty) => bare(
                "NotFound",
                &format!("vngcloud: resource not found: {caller}: VPC absent from selected scope"),
            ),
            _ => invalid(INVALID),
        },
        (SharedProject | ListRegion | List, Truncated) => transport("network request failed"),
        (SharedProject | ListRegion | List, Cancelled) => transport("canceled"),
        (SharedProject, Oversized) => bare("", "transport: response body exceeds limit"),
        (ListRegion, Oversized) => made(
            "network.ListNATInstances",
            Some(200),
            "InvalidResponse",
            "oversized regions envelope",
        ),
        (List, Oversized) => invalid("oversized list envelope"),
        (Detail | DetailProject | DetailRegion, Truncated) => invalid(WITHHELD),
        (Detail | DetailProject | DetailRegion, Oversized) => api(Some(200), "", WITHHELD),
        (Detail | DetailProject | DetailRegion, Cancelled) => api(None, "", WITHHELD),
        (Catalog | CatalogProject | CatalogRegion, Cancelled) => context(),
        (Catalog | CatalogProject | CatalogRegion, Truncated) => api(None, "", WITHHELD),
        (CatalogProject | CatalogRegion, Oversized) => invalid(WITHHELD),
        (Catalog, Oversized | Created) => invalid(INVALID),
        (SharedProject | List, Created) => api(Some(201), "", "Created"),
        (_, Created) => api(Some(201), "", WITHHELD),
        (SharedProject, SafeCode) => api(Some(403), "Forbidden", "Forbidden"),
        (SharedProject, UnsafeCode) => api(
            Some(403),
            "InvalidResponse",
            "invalid discovery response; body withheld",
        ),
        (ListRegion, SafeCode) => api(Some(403), PROVIDER_CODE, WITHHELD),
        (List, SafeCode) => api(Some(403), PROVIDER_CODE, PROVIDER_MESSAGE),
        (List, UnsafeCode) => api(Some(403), "Forbidden", PROVIDER_MESSAGE),
        (_, SafeCode | UnsafeCode) => api(Some(403), "Forbidden", WITHHELD),
        _ => unreachable!(),
    }
}

fn project_selection(outcome: Outcome) -> (&'static str, &'static str) {
    match outcome {
        Outcome::Empty => ("RequestFailed", PROJECT_ERROR),
        _ => ("InvalidConfig", PROJECT_AMBIGUOUS),
    }
}

fn http_identity(status: u16) -> (ErrorClass, &'static str) {
    match status {
        401 => (ErrorClass::Auth, "Unauthorized"),
        403 => (ErrorClass::PermissionDenied, "Forbidden"),
        404 => (ErrorClass::NotFound, "NotFound"),
        429 => (ErrorClass::RateLimited, "Throttled"),
        _ => (ErrorClass::Api, ""),
    }
}

fn inventory(target: Target, stage: Stage, ambiguous: bool) -> String {
    match stage {
        Stage::Project if ambiguous => {
            r#"{"projects":[{"projectId":"proj-1"},{"projectId":"proj-2"}]}"#.to_owned()
        }
        Stage::Project => r#"{"projects":[]}"#.to_owned(),
        Stage::Region => inventory_rows(REGION, ambiguous, true),
        Stage::Whitelist | Stage::Final if matches!(target, Target::Detail | Target::Quote) => {
            if ambiguous {
                r#"{"success":true,"code":0,"data":[{},{}]}"#.to_owned()
            } else {
                r#"{"success":true,"code":0,"data":null}"#.to_owned()
            }
        }
        Stage::AvailabilityZone => inventory_rows(ZONES, ambiguous, true),
        Stage::Package => inventory_rows(PACKAGES, ambiguous, true),
        Stage::Vpc => inventory_rows(VPCS, ambiguous, true),
        Stage::Final => inventory_rows(
            match target {
                Target::Zones => ZONES,
                Target::Packages => PACKAGES,
                Target::Instances => INSTANCES,
                Target::Rules => RULES,
                _ => unreachable!(),
            },
            ambiguous,
            false,
        ),
        Stage::Whitelist => unreachable!(),
    }
}

fn inventory_rows(fixture: &str, ambiguous: bool, duplicate: bool) -> String {
    let mut value: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let count = if ambiguous { 2 } else { 0 };
    let rows = value["data"].as_array_mut().unwrap();
    if ambiguous {
        let first = rows[0].clone();
        rows.clear();
        rows.push(first.clone());
        let mut rest = first;
        if !duplicate {
            rest["uuid"] = "other-1".into();
        }
        rows.push(rest);
    } else {
        rows.clear();
    }
    if value.get("total").is_some() {
        value["total"] = count.into();
        value["totalPage"] = i32::from(ambiguous).into();
    }
    value.to_string()
}

fn good(target: Target, stage: Stage) -> &'static str {
    match stage {
        Stage::Project => PROJECT,
        Stage::Region => REGION,
        Stage::Whitelist => WHITELIST,
        Stage::AvailabilityZone => ZONES,
        Stage::Package => PACKAGES,
        Stage::Vpc => VPCS,
        Stage::Final => match target {
            Target::Zones => ZONES,
            Target::Packages => PACKAGES,
            Target::Instances => INSTANCES,
            Target::Detail => {
                include_str!("../../../greennode-sdk/tests/fixtures/network/get_nat_instance.json")
            }
            Target::Rules => RULES,
            Target::Quote => {
                include_str!("../../../greennode-sdk/tests/fixtures/network/quote_create_nat.json")
            }
        },
    }
}

fn affected(
    target: Target,
    stage: Stage,
    outcome: Outcome,
) -> (Response, Option<tokio::sync::oneshot::Receiver<()>>) {
    use Outcome::*;
    let cap = if matches!(target, Target::Instances) && stage == Stage::Project {
        8 << 20
    } else {
        4 << 20
    };
    let reply = match outcome {
        Truncated => Response::RawClose(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{}".to_vec()),
        Oversized => {
            let payload = good(target, stage);
            let length = cap + 1 + payload.len();
            let mut raw =
                format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{length:x}\r\n")
                    .into_bytes();
            raw.resize(raw.len() + cap + 1, b' ');
            raw.extend_from_slice(payload.as_bytes());
            raw.extend_from_slice(b"\r\n0\r\n\r\n");
            Response::RawClose(raw)
        }
        OversizedHeader => Response::RawClose(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", cap + 1).into_bytes(),
        ),
        Created => response("201 Created", "{}"),
        Empty | Ambiguous => response(
            "200 OK",
            &inventory(target, stage, matches!(outcome, Ambiguous)),
        ),
        Cancelled | Deadline => {
            let (sent, ready) = tokio::sync::oneshot::channel();
            return (Response::StallBeforeHead(sent), Some(ready));
        }
        SafeCode => response(
            "403 Forbidden",
            r#"{"code":"CatalogDenied","message":"provider refused"}"#,
        ),
        UnsafeCode => response(
            "403 Forbidden",
            r#"{"code":"toy-secret","message":"provider refused"}"#,
        ),
        Status(429) => Response::Http {
            status: "429 Too Many Requests".into(),
            headers: vec![
                ("Content-Length".into(), "2".into()),
                ("Retry-After".into(), "1".into()),
            ],
            body: b"{}".to_vec(),
        },
        Status(status) => response(
            match status {
                401 => "401 Unauthorized",
                403 => "403 Forbidden",
                404 => "404 Not Found",
                _ => unreachable!(),
            },
            "{}",
        ),
        WithheldNotFound => response("404 Not Found", "{}"),
        PeerInvalidResponse => response(
            "403 Forbidden",
            r#"{"code":"InvalidResponse","message":"provider refused"}"#,
        ),
        CancelledInvalidation => {
            Response::RawClose(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\n\r\n{}".to_vec())
        }
        Login | TokenCache | InvalidPath | NegativePage | UnsupportedRegion => unreachable!(),
    };
    (reply, None)
}

fn expected_cli(want: Option<&Report>, outcome: Outcome) -> (ExitCode, Vec<u8>) {
    let Some((operation, status, raw_code, message)) = want else {
        return (ExitCode::SUCCESS, Vec::new());
    };
    let code = match (outcome, raw_code.as_str()) {
        (Outcome::WithheldNotFound, _) => "NotFound",
        (_, "") => "RequestFailed",
        (_, code) => code,
    };
    let exit = match (outcome, code) {
        (Outcome::Login | Outcome::Status(401), _) => 3,
        (_, "InvalidConfig" | "InvalidUsage") => 2,
        (_, "NotFound") => 4,
        _ => 1,
    };
    let mut error = serde_json::json!({"code": code, "message": message});
    if let Some(status) = status {
        error["status"] = (*status).into();
    }
    if !operation.is_empty() {
        error["operation"] = operation.clone().into();
    }
    let bytes = format!("{}\n", serde_json::json!({"error": error})).into_bytes();
    (ExitCode::from(exit), bytes)
}

#[tokio::test]
async fn nat_zone_and_package_errors_match_go() {
    nat_error_matrix_matches_go([Target::Zones, Target::Packages]).await;
}
#[tokio::test]
async fn nat_instance_and_detail_errors_match_go() {
    nat_error_matrix_matches_go([Target::Instances, Target::Detail]).await;
}
#[tokio::test]
async fn nat_rule_and_quote_errors_match_go() {
    nat_error_matrix_matches_go([Target::Rules, Target::Quote]).await;
}

async fn nat_error_matrix_matches_go(targets: [Target; 2]) {
    let mut mismatches = Vec::new();
    let regular = [
        Outcome::Truncated,
        Outcome::Oversized,
        Outcome::Created,
        Outcome::Empty,
        Outcome::Ambiguous,
        Outcome::Cancelled,
        Outcome::SafeCode,
        Outcome::UnsafeCode,
    ];
    let mut cases = Vec::new();
    for target in targets {
        for &stage in target.stages() {
            cases.extend(regular.map(|outcome| (target, stage, outcome)));
        }
    }
    cases.extend([
        (Target::Instances, Stage::Region, Outcome::OversizedHeader),
        (Target::Zones, Stage::Project, Outcome::WithheldNotFound),
        (Target::Zones, Stage::Region, Outcome::WithheldNotFound),
        (Target::Detail, Stage::Final, Outcome::CancelledInvalidation),
        (Target::Detail, Stage::Final, Outcome::PeerInvalidResponse),
        (Target::Rules, Stage::Final, Outcome::PeerInvalidResponse),
        (Target::Zones, Stage::Project, Outcome::Login),
        (Target::Detail, Stage::Final, Outcome::Deadline),
        (Target::Instances, Stage::Project, Outcome::Login),
        (Target::Detail, Stage::Project, Outcome::InvalidPath),
        (Target::Zones, Stage::Project, Outcome::InvalidPath),
        (Target::Packages, Stage::Project, Outcome::InvalidPath),
        (Target::Quote, Stage::Project, Outcome::InvalidPath),
        (Target::Instances, Stage::Project, Outcome::NegativePage),
        (
            Target::Instances,
            Stage::Project,
            Outcome::UnsupportedRegion,
        ),
    ]);
    for stage in [Stage::Project, Stage::Region] {
        cases.extend(
            [401, 403, 404, 429].map(|status| (Target::Zones, stage, Outcome::Status(status))),
        );
    }
    #[cfg(unix)]
    for target in [Target::Zones, Target::Instances] {
        cases.push((target, Stage::Project, Outcome::TokenCache));
    }
    for (target, stage, outcome) in cases
        .into_iter()
        .filter(|(target, _, _)| targets.contains(target))
    {
        let want = expected(target, stage, outcome);
        let (want_exit, want_stderr) = expected_cli(want.as_ref(), outcome);
        let want_sdk = expected_sdk(want.as_ref(), target, stage, outcome);
        let (sdk, sdk_retry_time) = run_case(target, stage, outcome, false).await;
        let Observed::Sdk(sdk) = sdk else {
            unreachable!()
        };
        let (cli, cli_retry_time) = run_case(target, stage, outcome, true).await;
        let Observed::Cli(exit, stderr) = cli else {
            unreachable!()
        };
        let duration = Some(std::time::Duration::from_secs(3));
        if matches!(outcome, Outcome::Status(429))
            && [sdk_retry_time, cli_retry_time] != [duration; 2]
        {
            mismatches.push(format!(
                "{target:?} {stage:?}: retries did not advance the injected clock"
            ));
        }
        if sdk != want_sdk || exit != want_exit || stderr != want_stderr {
            mismatches.push(format!("{target:?} {stage:?} {outcome:?}: SDK match {}, CLI expected exit {want_exit:?}, stderr {}", sdk == want_sdk, String::from_utf8_lossy(&want_stderr)));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
