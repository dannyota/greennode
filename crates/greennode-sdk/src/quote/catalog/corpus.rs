use std::collections::BTreeMap;

use http::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::Catalog;
use crate::testing::{Response, Server};
use crate::{Config, ConfigOptions, EndpointOverrides, Error, RequestContext, Secret};

#[derive(Deserialize)]
struct Case {
    name: String,
    service: String,
    replies: Vec<Reply>,
    requests: usize,
    items: Option<Value>,
    error: Option<Outcome>,
    identity: Option<String>,
    repeat: Option<RepeatedRow>,
    queries: Vec<String>,
    #[serde(default)]
    statuses: BTreeMap<usize, u16>,
    #[serde(default)]
    surrogates: usize,
    #[serde(default)]
    implicit_project: bool,
    #[serde(default)]
    wire_padding: BTreeMap<usize, usize>,
    local_error: Option<String>,
    #[serde(default)]
    nested_arrays: BTreeMap<usize, usize>,
}

impl Case {
    fn uses_byte_limit(&self) -> bool {
        self.surrogates != 0 || !self.wire_padding.is_empty()
    }

    fn group(&self) -> &str {
        if self.uses_byte_limit() || self.repeat.is_some() {
            return &self.name;
        }
        match self.service.as_str() {
            "volume" | "volume-partial" => "volume",
            "compute" | "compute-partial" | "compute-non-public" | "images" => "compute",
            "estimate" | "estimate-storage" | "estimate-partial" | "estimate-limit"
            | "instance" => "estimates",
            "storage" | "regions" => "storage",
            "project" | "network" => "project_and_network",
            _ => panic!("unknown catalogue corpus service"),
        }
    }
}

#[derive(Deserialize)]
struct RepeatedRow {
    request: usize,
    count: usize,
    row: String,
}

struct CaseFailure {
    error: Box<Error>,
    partial: Option<Value>,
}

impl From<Box<Error>> for CaseFailure {
    fn from(error: Box<Error>) -> Self {
        Self {
            error,
            partial: None,
        }
    }
}

fn project<T>(
    result: Result<T, crate::Failure<T>>,
    project: impl Fn(T) -> Value,
) -> Result<Option<Value>, CaseFailure> {
    match result {
        Ok(output) => Ok(Some(project(output))),
        Err(failure) => {
            let (error, partial, _) = failure.into_parts();
            Err(CaseFailure {
                error: Box::new(error),
                partial: partial.map(project),
            })
        }
    }
}

#[derive(Deserialize)]
struct Reply {
    path: String,
    body: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Outcome {
    operation: String,
    status: u16,
    code: String,
    message: String,
}

async fn run(case: &Case, server: &Server) -> Result<Option<Value>, CaseFailure> {
    let endpoint = || Some(format!("{}/", server.origin));
    let options = ConfigOptions::default()
        .with_static_token(Secret::new("toy-catalogue-bearer"))
        .with_region("hcm-3".into())
        .with_endpoint_overrides(EndpointOverrides {
            vserver: endpoint(),
            portal: endpoint(),
            storage: endpoint(),
            ..Default::default()
        })
        .with_clock(crate::testing::retry_clock());
    let options = if case.implicit_project {
        options
    } else {
        options.with_project_id("project-1".into())
    };
    let config = Config::new(options).unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let items = match case.service.as_str() {
        "volume" | "volume-partial" => {
            return project(
                crate::volume::Client::new(&config)
                    .list_volume_prices(
                        &ctx,
                        crate::volume::ListVolumePricesInput {
                            encryption: "aes_256".into(),
                            allow_partial: case.service == "volume-partial",
                            ..Default::default()
                        },
                    )
                    .await,
                |out| {
                    if case.service == "volume-partial" {
                        serde_json::json!([out.items, out.metadata.failures])
                    } else {
                        serde_json::to_value(out.items).unwrap()
                    }
                },
            );
        }
        "estimate-limit" => {
            use crate::pricing::{Client, EstimateInput, EstimatePublicIp};
            let input = EstimateInput {
                max_requests: 1,
                public_ips: [("first", "public"), ("second", "public-vm")]
                    .into_iter()
                    .map(|(key, kind)| EstimatePublicIp {
                        key: key.into(),
                        kind: "public-vip".into(),
                        r#type: kind.into(),
                        quantity: 1,
                    })
                    .collect(),
                ..Default::default()
            };
            return project(Client::new(&config).estimate(&ctx, input).await, |out| {
                serde_json::to_value(out.items).unwrap()
            });
        }
        "compute" | "compute-non-public" | "compute-partial" => {
            return project(
                crate::compute::Client::new(&config)
                    .list_flavor_prices(
                        &ctx,
                        crate::compute::ListFlavorPricesInput {
                            include_non_public: case.service == "compute-non-public",
                            allow_partial: case.service == "compute-partial",
                            ..Default::default()
                        },
                    )
                    .await,
                |out| {
                    if case.service == "compute-partial" {
                        serde_json::json!([out.items, out.metadata.failures])
                    } else {
                        serde_json::to_value(out.items).unwrap()
                    }
                },
            );
        }
        "estimate-partial" => {
            use crate::pricing::{Client, EstimateInput, EstimatePublicIp, EstimateStorage};
            let input = EstimateInput {
                public_ips: vec![EstimatePublicIp {
                    key: "first".into(),
                    kind: "public-vip".into(),
                    r#type: "public".into(),
                    quantity: 1,
                }],
                storage: [("refused", 1), ("later", 2)]
                    .into_iter()
                    .map(|(key, quota_gb)| EstimateStorage {
                        key: key.into(),
                        r#type: "Gold".into(),
                        quantity: 1,
                        quota_gb,
                        ..Default::default()
                    })
                    .collect(),
                allow_partial: true,
                ..Default::default()
            };
            return project(Client::new(&config).estimate(&ctx, input).await, |out| {
                serde_json::json!([out.items, out.metadata.failures, out.total])
            });
        }
        "project" => {
            return project(
                crate::project::Client::new(&config)
                    .list_projects(&ctx, Default::default())
                    .await,
                |out| serde_json::to_value(out.items).unwrap(),
            );
        }
        "network" => serde_json::to_value(
            crate::network::Client::new(&config)
                .list_network_prices(&ctx, Default::default())
                .await
                .map_err(|failure| Box::new(failure.into_parts().0))?
                .items,
        ),
        "instance" | "estimate" | "estimate-storage" => {
            use crate::quote::request::{Product, Request, decode};
            const OP: &str = "pricing.GetQuote";
            let storage = case.service == "estimate-storage";
            let (body, template, product) = if storage {
                (
                    crate::quote::input::storage(1, 2, 1),
                    "billing-api/v2/price",
                    Product::Storage(Some("region-1")),
                )
            } else {
                (
                    crate::quote::input::public_ip("public"),
                    "v1/price",
                    Product::Portal,
                )
            };
            let raw = Catalog::new(config.session(), &ctx, 10)
                .exchange(Request {
                    operation: OP,
                    template,
                    product,
                    path: &[],
                    query: if storage {
                        &[("region_id", "region-1")]
                    } else {
                        &[]
                    },
                    body: Some(&body),
                })
                .await?;
            let value = super::value(OP, &raw)?;
            if case.service == "instance" {
                serde_json::to_value(decode::instance(OP, &value)?)
            } else {
                let quote = decode::estimate(OP, &value, storage)?;
                let breakdown: Vec<_> = quote.breakdown.into_iter().map(|line| {
                    serde_json::json!({"Name":line.name,"MonthlyPrice":line.monthly_price})
                }).collect();
                Ok(serde_json::json!([
                    quote.price,
                    quote.breakdown_mismatch,
                    breakdown
                ]))
            }
        }
        "images" => {
            let rows = Catalog::new(config.session(), &ctx, 10)
                .images("project-1", "zone-1")
                .await?;
            let image = &rows[0];
            Ok(serde_json::json!([
                image.id,
                image.image_type,
                image.image_version,
                image.unlicensed_ubuntu(),
                image.root_minimum()
            ]))
        }
        "regions" => {
            let rows = Catalog::new(config.session(), &ctx, 10)
                .storage_regions()
                .await?;
            Ok(Value::Array(
                rows.iter()
                    .map(|row| serde_json::json!({"regionId":row.region_id,"regionName":row.region_name}))
                    .collect(),
            ))
        }
        "storage" => {
            let out = Catalog::new(config.session(), &ctx, 10)
                .storage_catalog("pricing.Estimate", "region-1")
                .await?;
            Ok(serde_json::json!([
                out.min_quota,
                out.max_quota,
                out.max_projects,
                out.iam_checkout,
                out.types[0].allow_period,
                out.types[0].billable_resources.len()
            ]))
        }
        _ => panic!("unknown catalogue service"),
    };
    Ok(Some(items.unwrap()))
}

fn normalize_items(value: &Value) -> Value {
    match value {
        Value::Array(rows) => Value::Array(rows.iter().map(normalize_items).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), normalize_items(value)))
                .collect(),
        ),
        Value::Number(number) => crate::output_number::format(
            &number.to_string(),
            false,
            crate::output_number::Style::Fixed,
        )
        .and_then(|decimal| decimal.parse().ok())
        .map_or_else(|| value.clone(), Value::Number),
        _ => value.clone(),
    }
}

#[test]
fn corpus_comparison_preserves_exact_integers() {
    let parse = |value| serde_json::from_str::<Value>(value).unwrap();
    assert_eq!(
        normalize_items(&parse("100.0")),
        normalize_items(&parse("1e2"))
    );
    assert_ne!(
        normalize_items(&parse("9007199254740992")),
        normalize_items(&parse("9007199254740993"))
    );
    assert_eq!(
        normalize_items(&parse("9007199254740993.0")),
        normalize_items(&parse("9007199254740993"))
    );
}

const CORPUS: &str = include_str!("../../../tests/fixtures/price_catalogue/go-v1.8.0.json");

macro_rules! catalogue_groups {
    ($($test:ident => $group:literal),+ $(,)?) => {
        $(#[tokio::test]
        async fn $test() {
            check_cases(|case| case.group() == $group).await;
        })+

        #[test]
        fn every_catalogue_case_has_one_group() {
            let cases: Vec<Case> = serde_json::from_str(CORPUS).unwrap();
            for case in &cases {
                assert_eq!([$($group),+].into_iter().filter(|group| *group == case.group()).count(), 1, "{}", case.name);
            }
        }
    };
}

catalogue_groups!(
    catalogue_volume_matches_go => "volume",
    catalogue_compute_matches_go => "compute",
    catalogue_estimates_match_go => "estimates",
    catalogue_storage_matches_go => "storage",
    catalogue_project_and_network_match_go => "project_and_network",
    catalogue_late_invalid_row_matches_go => "compute-partial/late-invalid-row-does-not-consume-budget",
    catalogue_raw_escaped_pages_match_go => "volume/raw-escaped-pages-exceed-budget",
    catalogue_retry_body_budget_matches_go => "volume/retry-body-does-not-consume-budget",
    catalogue_raw_escaped_pages_before_duplicate_match_go => "volume/raw-escaped-pages-exceed-budget-before-duplicate",
    catalogue_whitespace_budget_matches_go => "volume/implicit-project-whitespace-excluded-from-budget",
);

async fn check_cases(select: impl Fn(&Case) -> bool) {
    let cases: Vec<Case> = serde_json::from_str(CORPUS).unwrap();
    let mut mismatches = Vec::new();
    for case in cases.iter().filter(|case| select(case)) {
        let server = Server::start(
            case.replies
                .iter()
                .enumerate()
                .map(|(index, reply)| {
                    let mut body = case
                        .repeat
                        .as_ref()
                        .filter(|repeat| repeat.request == index)
                        .map_or_else(
                            || reply.body.clone(),
                            |repeat| {
                                format!(
                                    "[{}{}]",
                                    format!("{},", repeat.row).repeat(repeat.count),
                                    reply.body
                                )
                            },
                        );
                    if body.contains("@surrogates@") {
                        body = body.replacen("@surrogates@", &r"\uD800".repeat(case.surrogates), 1);
                    }
                    if let Some(&size) = case.wire_padding.get(&index) {
                        let extra = size - body.trim_ascii().len() + "@padding@".len();
                        body = body.replacen("@padding@", &"x".repeat(extra), 1);
                    }
                    if let Some(&depth) = case.nested_arrays.get(&index) {
                        body = format!("{}{}{}", "[".repeat(depth), body, "]".repeat(depth));
                    }
                    let status = case.statuses.get(&index).map_or(StatusCode::OK, |&status| {
                        StatusCode::from_u16(status).unwrap()
                    });
                    Response::Http {
                        status: format!(
                            "{} {}",
                            status.as_u16(),
                            status.canonical_reason().unwrap()
                        ),
                        headers: vec![
                            ("Content-Length".into(), body.len().to_string()),
                            ("Connection".into(), "close".into()),
                        ],
                        body: body.into_bytes(),
                    }
                })
                .collect(),
        );
        let result = run(case, &server).await;
        if server.requests() != case.requests {
            mismatches.push(format!(
                "{}: request count {} != {}",
                case.name,
                server.requests(),
                case.requests
            ));
        }
        assert_eq!(case.queries.len(), case.requests, "{}", case.name);
        for (index, (head, reply)) in server.heads().iter().zip(&case.replies).enumerate() {
            let target = head.split(|byte| *byte == b' ').nth(1).unwrap();
            let mut parts = target.splitn(2, |byte| *byte == b'?');
            let path = parts.next().unwrap();
            let query = parts.next().unwrap_or(b"");
            if path != reply.path.as_bytes() {
                mismatches.push(format!(
                    "{}: unexpected request path on request {}",
                    case.name,
                    index + 1
                ));
            }
            if case
                .queries
                .get(index)
                .is_none_or(|expected| query != expected.as_bytes())
            {
                mismatches.push(format!(
                    "{}: unexpected query on request {}",
                    case.name,
                    index + 1
                ));
            }
        }
        match result {
            Ok(items) if case.error.is_none() && case.identity.is_none() => {
                if items.as_ref().map(normalize_items) != case.items.as_ref().map(normalize_items) {
                    mismatches.push(format!("{}: items differ", case.name));
                }
            }
            Ok(_) => mismatches.push(format!("{}: accepted invalid catalogue", case.name)),
            Err(CaseFailure { error, partial }) => {
                if partial.as_ref().map(normalize_items) != case.items.as_ref().map(normalize_items)
                {
                    mismatches.push(format!("{}: partial items differ", case.name));
                }
                let expected_identity = match case.identity.as_deref() {
                    Some("DiscoveryIncomplete") => Some(crate::ErrorClass::DiscoveryIncomplete),
                    Some("PermissionDenied") => Some(crate::ErrorClass::PermissionDenied),
                    Some("EstimateLimit") => Some(crate::ErrorClass::EstimateLimit),
                    None => None,
                    _ => panic!("unknown error identity"),
                };
                if expected_identity.is_some_and(|class| !error.has_identity(class)) {
                    mismatches.push(format!("{}: missing error identity", case.name));
                }
                if case.local_error.as_ref().is_some_and(|expected| {
                    error.to_string() != *expected || error.has_class(crate::ErrorClass::Api)
                }) {
                    mismatches.push(format!(
                        "{}: unexpected local error display or class",
                        case.name
                    ));
                }
                let observed = error.api_error().map(|api| Outcome {
                    operation: api.operation().into(),
                    status: api.status().unwrap_or_default(),
                    code: api.code().into(),
                    message: api.message().into(),
                });
                if (case.error.is_none() && case.identity.is_none()) || observed != case.error {
                    mismatches.push(format!(
                        "{}: unexpected error {:?}",
                        case.name,
                        observed
                            .as_ref()
                            .map(|api| (&api.operation, api.status, &api.code))
                    ));
                }
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
