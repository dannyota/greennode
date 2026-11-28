use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::paid_writes::{
    ENDPOINT_REGIONS, ENDPOINT_ROW, NO_NATS, PAID_ENDPOINT, PAID_VPC, endpoint_session,
};
use super::*;
use crate::WriteDispatch;
use crate::testing::with_write_loopback;

const PROJECT: &str = include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json");
const LATER: &str = "new-toy-bearer";

struct Rotating {
    requests: AtomicUsize,
    rotate_at: usize,
    replacement: &'static str,
}

impl crate::CredentialsProvider for Rotating {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
        let requests = self.requests.fetch_add(1, Ordering::SeqCst);
        let marker = if requests < self.rotate_at {
            "initial-toy-bearer"
        } else {
            self.replacement
        };
        Box::pin(async move { Ok(crate::Token::new(Secret::new(marker), None)) })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

fn rotated_options(rotate_at: usize) -> ConfigOptions {
    rotating_options(rotate_at, LATER)
}

fn rotating_options(rotate_at: usize, replacement: &'static str) -> ConfigOptions {
    ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_credentials_provider(Arc::new(Rotating {
            requests: AtomicUsize::new(0),
            rotate_at,
            replacement,
        }))
}

fn endpoint_client(project: &Server, endpoint: &Server, rotate_at: usize) -> Client {
    let options = with_write_loopback(Product::Vserver, rotated_options(rotate_at), project);
    let options = with_write_loopback(Product::Vnetwork, options, endpoint);
    Client::new(&Config::new(options).unwrap())
}

#[tokio::test]
async fn endpoint_delete_rechecks_acceptance_receipt_after_final_inventory() {
    let project = Server::start(vec![response("200 OK", PROJECT)]);
    let ack = format!(r#"{{"success":true,"code":0,"message":"{LATER}"}}"#);
    let endpoint = Server::start(vec![
        response("200 OK", ENDPOINT_REGIONS),
        response("200 OK", ENDPOINT_ROW),
        response("200 OK", &ack),
        response("200 OK", NO_NATS),
    ]);
    let failure = endpoint_client(&project, &endpoint, 4)
        .delete_endpoint(
            &RequestContext::new().0,
            DeleteEndpointInput::new(PAID_ENDPOINT, PAID_VPC),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert!(!failure.partial().is_some_and(|partial| partial.deleted));
    assert_eq!(endpoint.requests(), 4);
}

#[tokio::test]
async fn endpoint_delete_inventory_charges_original_response_bytes() {
    let project = Server::start(vec![response("200 OK", PROJECT)]);
    let row = ENDPOINT_ROW.replace(r#""uuid":"enp-"#, r#""padding":"PLACEHOLDER","uuid":"enp-"#);
    let (before, after) = row.split_once("PLACEHOLDER").unwrap();
    let mut body = before.as_bytes().to_vec();
    body.resize(body.len() + (2 << 20), 0xff);
    body.extend_from_slice(after.as_bytes());
    let endpoint = Server::start(vec![
        response("200 OK", ENDPOINT_REGIONS),
        Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body,
        },
        response("200 OK", r#"{"success":true,"code":0}"#),
        response("200 OK", NO_NATS),
    ]);
    let output = endpoint_session(&project, &endpoint)
        .delete_endpoint(
            &RequestContext::new().0,
            DeleteEndpointInput::new(PAID_ENDPOINT, PAID_VPC),
        )
        .await
        .unwrap();
    assert!(output.deleted);
}

#[tokio::test]
async fn endpoint_unpriced_refusal_rechecks_placement_after_price_read() {
    let marker = format!(r#"{{"message":"{LATER}","#);
    let price = include_str!("../../../tests/fixtures/network/quote_create_nat.json")
        .replace("712400", "0");
    let project = Server::start(vec![response("200 OK", PROJECT)]);
    let vpc = include_str!("../../../tests/fixtures/network/endpoint_quote_vpcs.json")
        .replacen('{', &marker, 1);
    let endpoint = Server::start(vec![
        response("200 OK", ENDPOINT_REGIONS),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_zones.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_categories.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_services.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_packages.json"),
        ),
        response("200 OK", &vpc),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_subnets.json"),
        ),
        response("200 OK", NO_NATS),
        response("200 OK", &price),
    ]);
    let ctx = RequestContext::new().0;
    let failure = endpoint_client(&project, &endpoint, 9)
        .create_endpoint(
            &ctx,
            CreateEndpointInput::new("demo", PAID_VPC, "sub-00000000-0000-0000-0000-000000000003"),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(!failure.error().has_class(ErrorClass::Unpriced));
    assert_eq!(endpoint.requests(), 9);
}

#[tokio::test]
async fn nat_unpriced_refusal_rechecks_placement_after_price_read() {
    let ctx = RequestContext::new().0;
    let marker = format!(r#"{{"message":"{LATER}","#);
    let price = include_str!("../../../tests/fixtures/network/quote_create_nat.json")
        .replace("712400", "0");
    let whitelist = include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json")
        .replacen('{', &marker, 1);
    let network = Server::start(vec![
        response("200 OK", &whitelist),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_packages.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_vpcs.json"),
        ),
        response("200 OK", NO_NATS),
        response("200 OK", &price),
    ]);
    let options = with_write_loopback(
        Product::Vnetwork,
        rotated_options(5).with_project_id("proj-1".into()),
        &network,
    );
    let client = Client::new(&Config::new(options).unwrap());
    let failure = client
        .create_nat_instance(
            &ctx,
            CreateNatInstanceInput::new("demo", "zone-1", "az-1", "package-1", "vpc-1"),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(!failure.error().has_class(ErrorClass::Unpriced));
    assert_eq!(network.requests(), 6);
}

#[tokio::test]
async fn endpoint_preflight_remap_retains_rate_limit_retryability() {
    let project = Server::start(vec![response("200 OK", PROJECT)]);
    let endpoint = Server::start(
        (0..4)
            .map(|_| response("429 Too Many Requests", "{}"))
            .collect(),
    );
    let failure = endpoint_session(&project, &endpoint)
        .delete_endpoint(
            &RequestContext::new().0,
            DeleteEndpointInput::new(PAID_ENDPOINT, PAID_VPC),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.error().has_class(ErrorClass::RateLimited));
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.operation(), "network.DeleteEndpoint");
    assert!(api.retryable());
}

#[tokio::test]
async fn endpoint_create_refuses_rotated_credentials_in_query_identifiers_before_the_get() {
    let zone = include_str!("../../../tests/fixtures/network/endpoint_quote_zones.json")
        .replace("HCM03-1C", "az-aagb");
    let services = include_str!("../../../tests/fixtures/network/endpoint_quote_services.json")
        .replace(
            "00000000-0000-0000-0000-0000000000a1",
            "00000000-0000-0000-0000-0000000006a1",
        );
    let mut failures = Vec::new();
    for stage in ["direct zone", "service filter", "zone filter"] {
        let rotate_at = match stage {
            "direct zone" => 4,
            "service filter" => 6,
            _ => 7,
        };
        let project = Server::start(vec![response("200 OK", PROJECT)]);
        let endpoint = Server::start(vec![
            response("200 OK", ENDPOINT_REGIONS),
            response("200 OK", &zone),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/endpoint_quote_categories.json"),
            ),
            response(
                "200 OK",
                if stage == "service filter" {
                    &services
                } else {
                    include_str!("../../../tests/fixtures/network/endpoint_quote_services.json")
                },
            ),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/endpoint_quote_packages.json"),
            ),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/endpoint_quote_vpcs.json"),
            ),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/endpoint_quote_subnets.json"),
            ),
        ]);
        let options =
            with_write_loopback(Product::Vserver, rotating_options(rotate_at, "j"), &project);
        let options = with_write_loopback(Product::Vnetwork, options, &endpoint);
        let client = Client::new(&Config::new(options).unwrap());
        let mut input =
            CreateEndpointInput::new("edge", PAID_VPC, "sub-00000000-0000-0000-0000-000000000003");
        input.availability_zone_id = "az-aagb".into();
        let result = client
            .create_endpoint(&RequestContext::new().0, input)
            .await;
        if endpoint.requests() != rotate_at - 1
            || !result.is_err_and(|failure| {
                failure.write_dispatch() == WriteDispatch::NotSent
                    && failure.error().has_class(ErrorClass::InvalidInput)
            })
        {
            failures.push((stage, endpoint.requests()));
        }
    }
    assert!(
        failures.is_empty(),
        "unexpected selector requests: {failures:?}"
    );
}

#[tokio::test]
async fn nat_create_refuses_a_rotated_credential_in_the_package_zone_query() {
    let zones = include_str!("../../../tests/fixtures/network/list_nat_zones.json")
        .replace("az-1", "az-aZgb");
    let network = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json"),
        ),
        response("200 OK", &zones),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_packages.json"),
        ),
    ]);
    let options = with_write_loopback(
        Product::Vnetwork,
        rotating_options(2, "f").with_project_id("proj-1".into()),
        &network,
    );
    let client = Client::new(&Config::new(options).unwrap());
    let result = client
        .create_nat_instance(
            &RequestContext::new().0,
            CreateNatInstanceInput::new("edge", "zone-1", "az-aZgb", "package-1", "vpc-1"),
        )
        .await;
    assert_eq!(network.requests(), 2);
    let failure = result.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
}

#[tokio::test]
async fn endpoint_list_refuses_rotated_credentials_in_both_inventory_filters() {
    for (vpc_id, uuid) in [("net-aagb", ""), ("", "enp-aagb")] {
        let endpoint = Server::start(vec![
            response("200 OK", ENDPOINT_REGIONS),
            response("200 OK", NO_NATS),
        ]);
        let options = with_product_loopback(
            Product::Vnetwork,
            rotating_options(1, "j").with_project_id("pro-1".into()),
            &endpoint,
        );
        let client = Client::new(&Config::new(options).unwrap());
        let input = ListEndpointsInput {
            vpc_id: vpc_id.into(),
            uuid: uuid.into(),
            ..Default::default()
        };
        let result = client.list_endpoints(&RequestContext::new().0, input).await;
        assert_eq!(endpoint.requests(), 1);
        assert!(
            result
                .unwrap_err()
                .error()
                .has_class(ErrorClass::InvalidInput)
        );
    }
}

#[tokio::test]
async fn endpoint_tags_refuses_a_rotated_credential_in_the_resource_query() {
    let project = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":918}]}"#,
    )]);
    let endpoint = Server::start(vec![response("200 OK", r#"{"data":[]}"#)]);
    let options = with_product_loopback(Product::Vserver, rotating_options(1, "f"), &project);
    let options = with_product_loopback(Product::Vnetwork, options, &endpoint);
    let client = Client::new(&Config::new(options).unwrap());
    let result = client
        .list_endpoint_tags(
            &RequestContext::new().0,
            ListEndpointTagsInput::new("enp-aZgb"),
        )
        .await;
    assert_eq!(project.requests(), 1);
    assert_eq!(endpoint.requests(), 0);
    assert!(
        result
            .unwrap_err()
            .error()
            .has_class(ErrorClass::InvalidInput)
    );
}

#[tokio::test]
async fn endpoint_tags_refuses_a_rotated_credential_in_the_portal_user_header() {
    let project = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":1668}]}"#,
    )]);
    let endpoint = Server::start(vec![response("200 OK", r#"{"data":[]}"#)]);
    let options = with_product_loopback(Product::Vserver, rotating_options(1, "f"), &project);
    let options = with_product_loopback(Product::Vnetwork, options, &endpoint);
    let client = Client::new(&Config::new(options).unwrap());
    let result = client
        .list_endpoint_tags(
            &RequestContext::new().0,
            ListEndpointTagsInput::new("enp-clean"),
        )
        .await;
    assert_eq!(project.requests(), 1);
    assert_eq!(endpoint.requests(), 0);
    assert!(
        result
            .unwrap_err()
            .error()
            .has_class(ErrorClass::InvalidInput)
    );
}
