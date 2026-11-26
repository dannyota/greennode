use super::*;
use crate::WriteDispatch;

const CERT: &str = "-----BEGIN CERTIFICATE-----\nY2E=\n-----END CERTIFICATE-----\n";
const KEY: &str = "-----BEGIN PRIVATE KEY-----\ndG95LWtleQ==\n-----END PRIVATE KEY-----\n";
const PASSPHRASE: &str = "toy-passphrase";

fn tls_input() -> ImportCertificateInput {
    let mut input = ImportCertificateInput::new("toy-tls", CERTIFICATE_TYPE_TLS, CERT);
    input.private_key = Secret::new(KEY);
    input.passphrase = Secret::new(PASSPHRASE);
    input
}

#[tokio::test]
async fn import_uses_regional_post_once_and_withholds_key_and_retry_diagnostics() {
    let server = Server::start(vec![
        response(
            "201 Created",
            r#"{"data":{"uuid":"cert_1","name":"toy%2Dpassphrase"}}"#,
        ),
        response(
            "503 Service Unavailable",
            r#"{"code":"dG95LWtleQ==","message":"partial secret echo"}"#,
        ),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let mut misuse = tls_input();
    misuse.certificate_chain = KEY.into();
    assert_eq!(
        client
            .import_certificate(&ctx, misuse)
            .await
            .unwrap_err()
            .write_dispatch(),
        WriteDispatch::NotSent
    );
    let input = tls_input();
    let debug = format!("{input:?}");
    assert!(!debug.contains(PASSPHRASE) && !debug.contains("PRIVATE KEY"));
    let imported = client
        .import_certificate(&ctx, input.clone())
        .await
        .unwrap();
    assert_eq!(imported.certificate.uuid, "cert_1");
    assert_eq!(imported.certificate.name, "<redacted>");
    let failure = client.import_certificate(&ctx, input).await.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(
        failure.error().api_error().unwrap().message(),
        IMPORT_CERTIFICATE_WITHHELD_MESSAGE
    );
    let debug = format!("{failure:?}");
    assert!(!debug.contains("partial secret echo") && !debug.contains("dG95LWtleQ=="));
    assert_eq!(server.requests(), 2);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"POST /regional/v2/proj-1/cas "))
    );
    let bodies = server.bodies();
    let submitted: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
    assert!(submitted["privateKey"].as_str() == Some(KEY));
    assert!(submitted["passphrase"].as_str() == Some(PASSPHRASE));
    assert!(submitted.get("certificateChain").is_none());
}

fn create() -> CreateLoadBalancerInput {
    CreateLoadBalancerInput {
        name: "toy-lb".into(),
        package_id: "package-1".into(),
        r#type: "Layer 4".into(),
        scheme: "Internal".into(),
        subnet_id: "subnet-1".into(),
        zone_id: "zone-1".into(),
        max_price: 9.0,
        no_wait: true,
    }
}

#[tokio::test]
async fn create_refuses_unpriced_and_over_cap_quotes_and_sends_each_accepted_order_once() {
    let server = Server::start(vec![
        response("200 OK", r#"{"optimumPrice":0}"#),
        response("200 OK", r#"{"optimumPrice":10}"#),
        response("200 OK", r#"{"optimumPrice":9}"#),
        response(
            "503 Service Unavailable",
            r#"{"message":"order status unknown"}"#,
        ),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    for class in [ErrorClass::Unpriced, ErrorClass::PriceAboveMax] {
        let failure = client
            .create_load_balancer(&ctx, create())
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), class);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    }
    let failure = client
        .create_load_balancer(&ctx, create())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    assert!(
        heads[..3]
            .iter()
            .all(|head| head.starts_with(b"POST /billing/v1/price "))
    );
    assert!(heads[3].starts_with(b"POST /regional/v2/proj-1/loadBalancers "));
    let bodies = server.bodies();
    let order: serde_json::Value = serde_json::from_slice(&bodies[3]).unwrap();
    assert_eq!(
        order,
        serde_json::json!({"name":"toy-lb","packageId":"package-1","scheme":"Internal","subnetId":"subnet-1","type":"Layer 4","zoneId":"zone-1","autoScalable":false,"isPoc":false})
    );
}

#[tokio::test]
async fn create_uses_no_ambiguous_id_for_confirmation_and_rejects_truncated_404() {
    let server = Server::start(vec![
        response("200 OK", r#"{"optimumPrice":9}"#),
        response("202 Accepted", r#"{"uuid":"lb-1","UUID":"lb-2"}"#),
        response("200 OK", r#"{"optimumPrice":9}"#),
        response("202 Accepted", r#"{"uuid":"lb-1"}"#),
        Response::RawClose(b"HTTP/1.1 404 Not Found\r\nContent-Length: 40\r\n\r\n{}".to_vec()),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let input = CreateLoadBalancerInput {
        no_wait: false,
        ..create()
    };
    let failure = client
        .create_load_balancer(&ctx, input.clone())
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 2);
    let failure = client.create_load_balancer(&ctx, input).await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.partial().unwrap().load_balancer.uuid, "lb-1");
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn resize_skips_the_current_package_and_requires_a_complete_acceptance() {
    let current = r#"{"data":{"uuid":"lb-1","packageId":"package-1","progressStatus":"CREATED"}}"#;
    let server = Server::start(vec![
        response("200 OK", current),
        response("200 OK", current),
        response("200 OK", current),
        response("200 OK", r#"{"optimumPrice":-3}"#),
        Response::RawClose(b"HTTP/1.1 202 Accepted\r\nContent-Length: 40\r\n\r\n{}".to_vec()),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let mut input = ResizeLoadBalancerInput {
        load_balancer_id: "lb-1".into(),
        package_id: "package-1".into(),
        no_wait: true,
        ..Default::default()
    };
    assert!(
        !client
            .resize_load_balancer(&ctx, input.clone())
            .await
            .unwrap()
            .changed
    );
    assert_eq!(server.requests(), 1);
    input.package_id = "package-2".into();
    let failure = client.resize_load_balancer(&ctx, input).await.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    let heads = server.heads();
    assert_eq!(heads.len(), 5);
    assert!(heads[3].starts_with(b"POST /billing/v1/price "));
    assert!(heads[4].starts_with(b"PUT /regional/v2/proj-1/loadBalancers/lb-1/resize "));
}

#[tokio::test]
async fn regression_import_screens_hex_passphrase_in_metadata_and_error_code() {
    let subject = format!(
        "before-{}-after",
        data_encoding::HEXLOWER.encode(PASSPHRASE.as_bytes())
    );
    let code = format!(
        "before-{}-after",
        data_encoding::HEXUPPER.encode(PASSPHRASE.as_bytes())
    );
    let server = Server::start(vec![
        response(
            "201 Created",
            serde_json::json!({"data": {"uuid": "cert-1", "subject": subject}}).to_string(),
        ),
        response(
            "400 Bad Request",
            serde_json::json!({"code": code}).to_string(),
        ),
        response("400 Bad Request", r#"{"code":"ToyRefusal"}"#),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let output = client.import_certificate(&ctx, tls_input()).await.unwrap();
    let reflected = client
        .import_certificate(&ctx, tls_input())
        .await
        .unwrap_err();
    let safe = client
        .import_certificate(&ctx, tls_input())
        .await
        .unwrap_err();
    let reflected = reflected.error().api_error().unwrap();
    assert!(output.certificate.subject == "<redacted>" && reflected.code() != code);
    assert_eq!(safe.error().api_error().unwrap().code(), "ToyRefusal");
}

#[tokio::test]
async fn regression_import_allows_base64_shared_by_public_certificate_and_key() {
    let server = Server::start(vec![response(
        "201 Created",
        r#"{"data":{"uuid":"cert-1"}}"#,
    )]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let mut input = tls_input();
    input.certificate =
        "-----BEGIN CERTIFICATE-----\nY2VydGZpeHR1cmU0\ndG95LWtleQ==\n-----END CERTIFICATE-----\n"
            .into();
    input.certificate_chain = input.certificate.clone();
    let mut wrong_file = input.clone();
    wrong_file.certificate = KEY.into();
    let rejected = client
        .import_certificate(&ctx, wrong_file)
        .await
        .unwrap_err();
    assert_eq!(rejected.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
    assert!(client.import_certificate(&ctx, input).await.is_ok());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn regression_create_quote_sends_exact_priced_fields_and_go_defaults() {
    let server = Server::start(vec![
        response("200 OK", r#"{"optimumPrice":9}"#),
        response("200 OK", r#"{"optimumPrice":9}"#),
        response("202 Accepted", r#"{"uuid":"lb-1"}"#),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    client
        .quote_create_load_balancer(&ctx, create())
        .await
        .unwrap();
    client.create_load_balancer(&ctx, create()).await.unwrap();
    let expected = serde_json::json!({"resourceType":"load-balancer","action":"create","resourceInfo":{
        "packageId":"package-1","zoneId":"zone-1","isBuyMorePoc":false,"period":1,"isPoc":false
    }});
    assert_eq!(server.requests(), 3);
    for (head, body) in server.heads().iter().zip(server.bodies()).take(2) {
        assert!(head.starts_with(b"POST /billing/v1/price "));
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body, expected);
    }
}

#[tokio::test]
async fn regression_resize_no_wait_rejects_foreign_id_and_accepts_empty_acknowledgement() {
    let server = Server::start(
        [r#"{"uuid":"lb-other"}"#, "", r#"{"uuid":"lb-1"}"#]
            .into_iter()
            .flat_map(resize_replies)
            .collect(),
    );
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let input = resize(true);
    let failure = client
        .resize_load_balancer(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    for _ in 0..2 {
        let output = client
            .resize_load_balancer(&ctx, input.clone())
            .await
            .unwrap();
        assert!(output.changed);
        assert_eq!(output.load_balancer.uuid, "lb-1");
        assert_eq!(output.load_balancer.package_id, "package-2");
    }
    assert_eq!(server.requests(), 12);
}

#[tokio::test]
async fn regression_paid_load_balancers_require_strict_go_ids_before_io() {
    let server = Server::start(vec![]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    for input in [
        CreateLoadBalancerInput {
            package_id: "package_1".into(),
            ..create()
        },
        CreateLoadBalancerInput {
            subnet_id: "subnet_1".into(),
            ..create()
        },
    ] {
        let error = client
            .create_load_balancer(&ctx, input.clone())
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), ErrorClass::InvalidInput);
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(
            client
                .quote_create_load_balancer(&ctx, input)
                .await
                .unwrap_err()
                .error()
                .class(),
            ErrorClass::InvalidInput
        );
    }
    let input = ResizeLoadBalancerInput {
        package_id: "package_2".into(),
        ..resize(true)
    };
    let error = client
        .resize_load_balancer(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        client
            .quote_resize_load_balancer(&ctx, input)
            .await
            .unwrap_err()
            .error()
            .class(),
        ErrorClass::InvalidInput
    );
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn regression_resize_error_status_precedes_the_confirmation_package_requirement() {
    let mut replies = Vec::new();
    for confirmation in [
        r#"{"data":{"uuid":"lb-1","progressStatus":"ERROR"}}"#,
        r#"{"data":{"uuid":"lb-1","progressStatus":"CREATED"}}"#,
    ] {
        replies.extend(resize_replies(""));
        replies.push(response("200 OK", confirmation));
    }
    let server = Server::start(replies);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    for class in [ErrorClass::Failed, ErrorClass::NotSettled] {
        let error = client
            .resize_load_balancer(&ctx, resize(false))
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), class);
        assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    }
    assert_eq!(server.requests(), 10);
}

fn resize(no_wait: bool) -> ResizeLoadBalancerInput {
    ResizeLoadBalancerInput {
        load_balancer_id: "lb-1".into(),
        package_id: "package-2".into(),
        no_wait,
        ..Default::default()
    }
}

fn resize_replies(acknowledgement: &str) -> [Response; 4] {
    let current = r#"{"data":{"uuid":"lb-1","packageId":"package-1","progressStatus":"CREATED"}}"#;
    [
        response("200 OK", current),
        response("200 OK", current),
        response("200 OK", r#"{"optimumPrice":-3}"#),
        response("202 Accepted", acknowledgement),
    ]
}
