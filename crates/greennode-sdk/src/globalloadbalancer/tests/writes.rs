use super::*;
use crate::testing::{Product, with_product_loopback, with_write_loopback};
use serde_json::{Value, json};

const ACCEPTED: &str = r#"{"globalLoadBalancer":{"id":"glb-1"}}"#;
const DETAIL: &str =
    r#"{"id":"glb-1","type":"Layer 4","name":"demo","package":"pkg-1","status":"ACTIVE"}"#;
const COLLECTION: &str = "/glb-controller/v1/global-load-balancers";
const GLB: &str = "glb-1";

fn write_options(server: &Server, token: &str) -> ConfigOptions {
    with_write_loopback(
        Product::GlobalLoadBalancer,
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(token))
            .with_endpoint_overrides(EndpointOverrides {
                global_load_balancer: Some(format!("{}/glb-controller/", server.origin)),
                ..Default::default()
            }),
        server,
    )
}

fn write_client(server: &Server, token: &str) -> Client {
    Client::new(&Config::new(write_options(server, token)).unwrap())
}

#[tokio::test]
async fn registered_products_can_use_separate_loopback_endpoints() {
    let glb = Server::start(vec![
        response("202 Accepted", ACCEPTED),
        response("200 OK", DETAIL),
    ]);
    let regional = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"project-1","region":"hcm-3"}]}"#,
    )]);
    let opts = with_product_loopback(Product::Vserver, write_options(&glb, TOKEN), &regional);
    let config = Config::new(opts).unwrap();
    let (ctx, _) = RequestContext::new();
    Client::new(&config)
        .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
        .await
        .unwrap();
    assert_eq!(glb.requests(), 2);
    let projects = crate::project::Client::new(&config)
        .list_projects(&ctx, crate::project::ListProjectsInput::default())
        .await
        .unwrap();
    assert_eq!(projects.items[0].id, "project-1");
    assert_eq!(regional.requests(), 1);
    assert!(regional.heads()[0].starts_with(b"GET /v1/projects "));
}

#[tokio::test]
async fn parent_create_posts_once_then_confirms_the_exact_detail() {
    for status in ["200 OK", "202 Accepted"] {
        let server = Server::start(vec![response(status, ACCEPTED), response("200 OK", DETAIL)]);
        let (ctx, _) = RequestContext::new();
        let mut input = CreateLoadBalancerInput::new("demo", "pkg-1");
        input.description = "sample".into();
        let output = write_client(&server, TOKEN)
            .create_load_balancer(&ctx, input)
            .await
            .unwrap();
        assert_eq!(output.load_balancer.id, GLB);
        assert_eq!(output.load_balancer.status, "ACTIVE");
        let heads = server.heads();
        assert!(heads[0].starts_with(format!("POST {COLLECTION} ").as_bytes()));
        assert!(heads[1].starts_with(format!("GET {COLLECTION}/{GLB} ").as_bytes()));
        let sent = serde_json::from_slice::<Value>(&server.bodies()[0]).ok();
        assert!(
            sent == Some(
                json!({"name": "demo", "package": "pkg-1", "paymentFlow": "automated", "type": "Layer 4", "description": "sample"})
            ),
            "create body differs"
        );
    }
}

#[tokio::test]
async fn conflicting_or_reflected_acceptance_ids_never_start_confirmation() {
    let token = "toy-id";
    for body in [
        r#"{"globalLoadBalancer":{"id":"glb-1","ID":"glb-2"}}"#,
        r#"{"globalLoadBalancer":{"id":"toy-id"}}"#,
        r#"{"globalLoadBalancer":{"id":"dG95LWlk"}}"#,
    ] {
        let server = Server::start(vec![response("202 Accepted", body)]);
        let (ctx, _) = RequestContext::new();
        let error = write_client(&server, token)
            .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
            .await
            .unwrap_err();
        assert!(error.error().has_identity(ErrorClass::NotSettled));
        assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(error.partial().is_none());
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn create_uses_the_accepted_id_as_partial_on_terminal_error_and_bad_confirmation() {
    for (body, class) in [
        (DETAIL.replace("ACTIVE", "ERROR"), ErrorClass::Failed),
        (DETAIL.replace("demo", "different"), ErrorClass::NotSettled),
    ] {
        let server = Server::start(vec![
            response("202 Accepted", ACCEPTED),
            response("200 OK", body),
        ]);
        let (ctx, _) = RequestContext::new();
        let error = write_client(&server, TOKEN)
            .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
            .await
            .unwrap_err();
        assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(error.error().has_identity(class));
        let partial = &error.partial().unwrap().load_balancer;
        assert_eq!(partial.id, GLB);
        assert!(partial.name.is_empty());
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn configured_credential_in_request_fields_is_refused_before_product_io() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let token = "toy-token";
    let mut input = CreateLoadBalancerInput::new("demo", "pkg-1");
    input.description = "a toy%2Dtoken value".into();
    let failure = write_client(&server, token)
        .create_load_balancer(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn create_keeps_safe_provider_refusals_and_never_replays() {
    for (status, body, class, message, observed) in [
        (
            "402 Payment Required",
            r#"{"message":"Insufficient credits for this order"}"#,
            ErrorClass::Api,
            "Insufficient credits for this order",
            Some(402),
        ),
        (
            "401 Unauthorized",
            r#"{"message":"authorization: toy%2Dtoken"}"#,
            ErrorClass::Auth,
            "provider response withheld",
            Some(401),
        ),
        (
            "500 Internal Server Error",
            r#"{"message":"server error"}"#,
            ErrorClass::NotSettled,
            "globalloadbalancer.CreateLoadBalancer: inspect inventory and billing before another mutation: vngcloud: write outcome not confirmed: create request unconfirmed",
            None,
        ),
    ] {
        let server = Server::start(vec![response(status, body)]);
        let (ctx, _) = RequestContext::new();
        let error = write_client(&server, "toy-token")
            .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
            .await
            .unwrap_err();
        assert!(error.error().has_class(class));
        let api = error.error().api_error().unwrap();
        assert_eq!(api.status(), observed);
        assert!(api.message() == message);
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn truncated_create_forbidden_is_classified_and_hides_reflected_status() {
    for token in ["toy-token", "403"] {
        let server = Server::start(vec![Response::RawClose(
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\n\r\n{}".to_vec(),
        )]);
        let (ctx, _) = RequestContext::new();
        let failure = write_client(&server, token)
            .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
            .await
            .unwrap_err();
        assert!(failure.error().has_class(ErrorClass::PermissionDenied));
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.code(), "Forbidden");
        assert_eq!(api.status(), if token == "403" { None } else { Some(403) });
    }
}

#[tokio::test]
async fn truncated_rate_limit_is_not_retryable() {
    for (reply, retryable) in [
        (response("429 Too Many Requests", "{}"), true),
        (
            Response::RawClose(
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 9\r\n\r\n{}".to_vec(),
            ),
            false,
        ),
    ] {
        let server = Server::start(vec![reply]);
        let (ctx, _) = RequestContext::new();
        let failure = write_client(&server, TOKEN)
            .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
            .await
            .unwrap_err();
        assert!(failure.error().has_class(ErrorClass::RateLimited));
        assert_eq!(failure.error().api_error().unwrap().retryable(), retryable);
    }
}

#[tokio::test]
async fn oversized_not_found_preserves_delete_status() {
    let body = "x".repeat((8 << 20) + 1);
    let server = Server::start(vec![response("404 Not Found", body.clone())]);
    let (ctx, _) = RequestContext::new();
    let failure = write_client(&server, TOKEN)
        .delete_load_balancer(&ctx, DeleteLoadBalancerInput::new(GLB))
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotFound));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);

    let server = Server::start(vec![
        response("200 OK", DETAIL),
        response("202 Accepted", ""),
        response("404 Not Found", body),
    ]);
    let output = write_client(&server, TOKEN)
        .delete_load_balancer(&ctx, DeleteLoadBalancerInput::new(GLB))
        .await
        .unwrap();
    assert!(output.deleted);
}

#[tokio::test]
async fn delete_confirms_only_a_complete_not_found_after_a_write() {
    for absent in [true, false] {
        let check = if absent {
            response("404 Not Found", "{}")
        } else {
            Response::RawClose(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\n{}".to_vec())
        };
        let server = Server::start(vec![
            response("200 OK", DETAIL),
            response("202 Accepted", ""),
            check,
        ]);
        let (ctx, _) = RequestContext::new();
        let result = write_client(&server, TOKEN)
            .delete_load_balancer(&ctx, DeleteLoadBalancerInput::new(GLB))
            .await;
        if absent {
            assert!(result.unwrap().deleted);
        } else {
            let error = result.unwrap_err();
            assert!(error.error().has_identity(ErrorClass::NotSettled));
            assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
            assert!(!error.partial().unwrap().deleted);
        }
        let heads = server.heads();
        assert!(heads[0].starts_with(format!("GET {COLLECTION}/{GLB} ").as_bytes()));
        assert!(heads[1].starts_with(format!("DELETE {COLLECTION}/{GLB} ").as_bytes()));
        assert!(heads[2].starts_with(format!("GET {COLLECTION}/{GLB} ").as_bytes()));
    }
}

#[tokio::test]
async fn a_preflight_not_found_is_not_a_delete_confirmation() {
    let server = Server::start(vec![response("404 Not Found", "{}")]);
    let (ctx, _) = RequestContext::new();
    let failure = write_client(&server, TOKEN)
        .delete_load_balancer(&ctx, DeleteLoadBalancerInput::new(GLB))
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotFound));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn output_redacts_typed_numbers_and_booleans() {
    let (ctx, _) = RequestContext::new();
    let create = Server::start(vec![
        response("202 Accepted", ACCEPTED),
        response("200 OK", DETAIL),
    ]);
    let output = write_client(&create, "0")
        .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(output).unwrap()["LoadBalancer"]["UserID"],
        "<redacted>"
    );

    let delete = Server::start(vec![
        response("200 OK", DETAIL),
        response("202 Accepted", ""),
        response("404 Not Found", "{}"),
    ]);
    let output = write_client(&delete, "true")
        .delete_load_balancer(&ctx, DeleteLoadBalancerInput::new(GLB))
        .await
        .unwrap();
    assert!(output.deleted);
    assert_eq!(
        serde_json::to_value(output).unwrap()["Deleted"],
        "<redacted>"
    );
}

#[tokio::test]
async fn a_read_fixture_cannot_unlock_glb_writes() {
    let glb = Server::start(vec![]);
    let database = Server::start(vec![]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new(TOKEN));
    let options = with_product_loopback(Product::GlobalLoadBalancer, options, &glb);
    let options = with_write_loopback(Product::Database, options, &database);
    let client = Client::new(&Config::new(options).unwrap());
    let (ctx, _) = RequestContext::new();
    let error = client
        .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
        .await
        .unwrap_err();
    assert!(error.error().has_identity(ErrorClass::InvalidConfig));
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(glb.requests(), 0);
    assert_eq!(database.requests(), 0);
}
