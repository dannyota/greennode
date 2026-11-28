use super::*;
use crate::WriteDispatch;

#[tokio::test]
async fn paid_endpoint_rejects_service_account_before_network() {
    let config = Config::new(options().with_project_id("proj-1".into())).unwrap();
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .create_endpoint(
            &ctx,
            CreateEndpointInput::new(
                "endpoint",
                "net-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "sub-bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
}

#[tokio::test]
async fn nat_delete_rejects_service_account_before_network() {
    let config = Config::new(options().with_project_id("proj-1".into())).unwrap();
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .delete_nat_instance(
            &ctx,
            DeleteNatInstanceInput::new("zone-1", "vpc-1", "nat-1"),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
}
#[tokio::test]
async fn endpoint_delete_refuses_attachment_alias_before_sending() {
    use crate::testing::with_write_loopback;
    let vserver = Server::start(vec![response(
        "200 OK",
        include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json"),
    )]);
    let row = r#"{"success":true,"page":1,"size":1000,"totalPage":1,"total":1,"data":[{"uuid":"enp-00000000-0000-0000-0000-0000000000c1","endpointName":"demo","endpointServiceId":"66b500000000000000000009","vpcId":"net-00000000-0000-0000-0000-000000000002","status":"ACTIVE","endpointType":"Gateway","zoneUuid":"HCM03-1C","enableDnsName":false,"service":{"uuid":"00000000-0000-0000-0000-0000000000a1","name":"vstorage.hcm03"},"subnet":{"uuid":"sub-00000000-0000-0000-0000-000000000003"},"packageId":{"uuid":"00000000-0000-0000-0000-0000000000b1"},"project":{"VServerProjectId":"pro-00000000-0000-0000-0000-000000000001"},"endpointResource":null,"endpointDomains":[]}]}"#.replace("endpointDomains", "EndpointDomains");
    let vnetwork = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_regions.json"),
        ),
        response("200 OK", &row),
    ]);
    let options = with_write_loopback(Product::Vserver, options(), &vserver);
    let config = Config::new(with_write_loopback(Product::Vnetwork, options, &vnetwork)).unwrap();
    let (ctx, _) = RequestContext::new();
    let input = DeleteEndpointInput::new(
        "enp-00000000-0000-0000-0000-0000000000c1",
        "net-00000000-0000-0000-0000-000000000002",
    );
    let failure = Client::new(&config)
        .delete_endpoint(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(vnetwork.heads().len(), 2);
}

#[tokio::test]
async fn endpoint_delete_reports_status_on_oversized_inventory_refusal() {
    use crate::testing::with_write_loopback;
    let vserver = Server::start(vec![response(
        "200 OK",
        include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json"),
    )]);
    let vnetwork = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_regions.json"),
        ),
        response("403 Forbidden", &"x".repeat((4 << 20) + 1)),
    ]);
    let options = with_write_loopback(Product::Vserver, options(), &vserver);
    let config = Config::new(with_write_loopback(Product::Vnetwork, options, &vnetwork)).unwrap();
    let (ctx, _) = RequestContext::new();
    let input = DeleteEndpointInput::new(
        "enp-00000000-0000-0000-0000-0000000000c1",
        "net-00000000-0000-0000-0000-000000000002",
    );
    let failure = Client::new(&config)
        .delete_endpoint(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.message()),
        (Some(403), "endpoint response withheld")
    );
    assert_eq!(vnetwork.heads().len(), 2);
}

const PAID_PROJECT: &str = "pro-00000000-0000-0000-0000-000000000001";
const PAID_ZONE: &str = "66b500000000000000000001";
pub(super) const NO_NATS: &str =
    r#"{"success":true,"page":1,"size":1000,"totalPage":0,"total":0,"data":[]}"#;

#[tokio::test]
async fn nat_delete_keeps_truncated_response_unsettled_after_empty_inventory() {
    use crate::testing::with_write_loopback;
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_vpc_picker.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_write_inventory.json"),
        ),
        Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), "100".into())],
            body: b"{}".to_vec(),
        },
        response("200 OK", NO_NATS),
    ]);
    let config = Config::new(with_write_loopback(
        Product::Vnetwork,
        options().with_project_id(PAID_PROJECT.into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let error = Client::new(&config)
        .delete_nat_instance(
            &ctx,
            DeleteNatInstanceInput::new(PAID_ZONE, "vpc-1", "nat-1"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().class(), ErrorClass::NotSettled);
    assert_eq!(server.heads().len(), 4);
}

#[tokio::test]
async fn nat_delete_refuses_alias_and_wrong_type_in_ack_paging() {
    use crate::testing::with_write_loopback;
    for payload in [
        r#"{"success":true,"code":0,"Page":1}"#,
        r#"{"success":true,"code":0,"page":"1"}"#,
    ] {
        let server = Server::start(vec![
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/nat_vpc_picker.json"),
            ),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/nat_write_inventory.json"),
            ),
            response("200 OK", payload),
        ]);
        let config = Config::new(with_write_loopback(
            Product::Vnetwork,
            options().with_project_id(PAID_PROJECT.into()),
            &server,
        ))
        .unwrap();
        let (ctx, _) = RequestContext::new();
        let mut input = DeleteNatInstanceInput::new(PAID_ZONE, "vpc-1", "nat-1");
        input.no_wait = true;
        let error = Client::new(&config)
            .delete_nat_instance(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), ErrorClass::NotSettled);
        assert_eq!(server.heads().len(), 3);
    }
}

#[tokio::test]
async fn nat_delete_accepts_empty_picker_zones_and_numeric_inventory_code() {
    use crate::testing::with_write_loopback;
    let picker = r#"{"success":true,"page":1,"size":1000,"totalPage":1,"total":1,"data":[{"uuid":"vpc-1","projectUuid":"pro-00000000-0000-0000-0000-000000000001","regionId":"66b500000000000000000001","zones":[]}]}"#;
    let inventory = include_str!("../../../tests/fixtures/network/nat_write_inventory.json")
        .replacen('{', r#"{"code":200,"#, 1);
    let server = Server::start(vec![
        response("200 OK", picker),
        response("200 OK", &inventory),
        response("200 OK", r#"{"success":true,"code":0}"#),
    ]);
    let config = Config::new(with_write_loopback(
        Product::Vnetwork,
        options().with_project_id(PAID_PROJECT.into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let mut input = DeleteNatInstanceInput::new(PAID_ZONE, "vpc-1", "nat-1");
    input.no_wait = true;
    Client::new(&config)
        .delete_nat_instance(&ctx, input)
        .await
        .unwrap();
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn nat_delete_refuses_a_picker_row_without_zones() {
    use crate::testing::with_write_loopback;
    let picker = r#"{"success":true,"page":1,"size":1000,"totalPage":1,"total":1,"data":[{"uuid":"vpc-1","projectUuid":"pro-00000000-0000-0000-0000-000000000001","regionId":"66b500000000000000000001"}]}"#;
    let inventory = include_str!("../../../tests/fixtures/network/nat_write_inventory.json")
        .replacen('{', r#"{"code":200,"#, 1);
    let server = Server::start(vec![
        response("200 OK", picker),
        response("200 OK", &inventory),
        response("200 OK", r#"{"success":true,"code":0}"#),
    ]);
    let config = Config::new(with_write_loopback(
        Product::Vnetwork,
        options().with_project_id(PAID_PROJECT.into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let mut input = DeleteNatInstanceInput::new(PAID_ZONE, "vpc-1", "nat-1");
    input.no_wait = true;
    let failure = Client::new(&config)
        .delete_nat_instance(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn nat_delete_refuses_non_path_identifiers_in_inventory() {
    use crate::testing::with_write_loopback;
    for (original, invalid) in [("vpc-1", "vpc_1"), ("package-1", "package_1")] {
        let row = include_str!("../../../tests/fixtures/network/nat_write_inventory.json")
            .replace(original, invalid);
        let server = Server::start(vec![
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/nat_vpc_picker.json"),
            ),
            response("200 OK", &row),
            response("200 OK", r#"{"success":true,"code":0}"#),
        ]);
        let options = with_write_loopback(
            Product::Vnetwork,
            options().with_project_id(PAID_PROJECT.into()),
            &server,
        );
        let client = Client::new(&Config::new(options).unwrap());
        let mut input = DeleteNatInstanceInput::new(PAID_ZONE, "vpc-1", "nat-1");
        input.no_wait = true;
        let failure = client
            .delete_nat_instance(&RequestContext::new().0, input)
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse"
        );
        assert_eq!(server.requests(), 2);
    }
}

pub(super) const ENDPOINT_ROW: &str = r#"{"success":true,"page":1,"size":1000,"totalPage":1,"total":1,"data":[{"uuid":"enp-00000000-0000-0000-0000-0000000000c1","endpointName":"demo","endpointServiceId":"66b500000000000000000009","vpcId":"net-00000000-0000-0000-0000-000000000002","status":"ACTIVE","endpointType":"Gateway","zoneUuid":"HCM03-1C","enableDnsName":false,"service":{"uuid":"00000000-0000-0000-0000-0000000000a1","name":"vstorage.hcm03"},"subnet":{"uuid":"sub-00000000-0000-0000-0000-000000000003"},"packageId":{"uuid":"00000000-0000-0000-0000-0000000000b1"},"project":{"VServerProjectId":"pro-00000000-0000-0000-0000-000000000001"},"endpointResource":null,"endpointDomains":[]}]}"#;
pub(super) const ENDPOINT_REGIONS: &str =
    include_str!("../../../tests/fixtures/network/endpoint_quote_regions.json");
pub(super) const PAID_ENDPOINT: &str = "enp-00000000-0000-0000-0000-0000000000c1";
pub(super) const PAID_VPC: &str = "net-00000000-0000-0000-0000-000000000002";

pub(super) fn endpoint_session(project: &Server, endpoint: &Server) -> Client {
    use crate::testing::with_write_loopback;
    let options = with_write_loopback(Product::Vserver, options(), project);
    let options = with_write_loopback(Product::Vnetwork, options, endpoint);
    Client::new(&Config::new(options).unwrap())
}

#[tokio::test]
async fn endpoint_delete_discovers_mapping_on_every_operation() {
    let project_body = include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json");
    let project = Server::start(vec![
        response("200 OK", project_body),
        response("200 OK", project_body),
    ]);
    let empty = r#"{"success":true,"page":1,"size":1000,"totalPage":0,"total":0,"data":[]}"#;
    let remapped = ENDPOINT_REGIONS.replace(PAID_ZONE, "66b500000000000000000002");
    let endpoint = Server::start(vec![
        response("200 OK", ENDPOINT_REGIONS),
        response("200 OK", ENDPOINT_ROW),
        response("200 OK", r#"{"success":true,"code":0}"#),
        response("200 OK", empty),
        response("200 OK", &remapped),
        response("200 OK", ENDPOINT_ROW),
        response("200 OK", r#"{"success":true,"code":0}"#),
        response("200 OK", empty),
    ]);
    let client = endpoint_session(&project, &endpoint);
    let (ctx, _) = RequestContext::new();
    for _ in 0..2 {
        assert!(
            client
                .delete_endpoint(&ctx, DeleteEndpointInput::new(PAID_ENDPOINT, PAID_VPC))
                .await
                .unwrap()
                .deleted
        );
    }
    let heads = endpoint.heads();
    assert!(heads[4].starts_with(b"GET /vnetwork/v1/regions "));
    assert!(heads[6].starts_with(b"DELETE /vnetwork/v1/66b500000000000000000002/"));
}

#[tokio::test]
async fn endpoint_delete_refuses_bad_ack_paging_and_retains_429_retry_hint() {
    let project_body = include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json");
    for body in [
        r#"{"success":true,"code":0,"Page":1}"#,
        r#"{"success":true,"code":0,"page":"1"}"#,
    ] {
        let project = Server::start(vec![response("200 OK", project_body)]);
        let endpoint = Server::start(vec![
            response("200 OK", ENDPOINT_REGIONS),
            response("200 OK", ENDPOINT_ROW),
            response("200 OK", body),
        ]);
        let client = endpoint_session(&project, &endpoint);
        let (ctx, _) = RequestContext::new();
        let failure = client
            .delete_endpoint(&ctx, DeleteEndpointInput::new(PAID_ENDPOINT, PAID_VPC))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::NotSettled);
        assert_eq!(endpoint.heads().len(), 3);
    }
    let project = Server::start(vec![response("200 OK", project_body)]);
    let endpoint = Server::start(vec![
        response("200 OK", ENDPOINT_REGIONS),
        response("200 OK", ENDPOINT_ROW),
        response(
            "429 Too Many Requests",
            r#"{"code":"custom","message":"provider text"}"#,
        ),
    ]);
    let client = endpoint_session(&project, &endpoint);
    let (ctx, _) = RequestContext::new();
    let error = client
        .delete_endpoint(&ctx, DeleteEndpointInput::new(PAID_ENDPOINT, PAID_VPC))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message(), api.retryable()),
        (Some(429), "Throttled", "endpoint response withheld", true)
    );
    assert_eq!(endpoint.heads().len(), 3);
}

#[tokio::test]
async fn endpoint_create_normalizes_go_surrogates_before_ack_decoding() {
    let project = Server::start(vec![response(
        "200 OK",
        include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json"),
    )]);
    let state = ENDPOINT_ROW.replace("demo", "demo�");
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
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_vpcs.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/endpoint_quote_subnets.json"),
        ),
        response("200 OK", NO_NATS),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/quote_create_nat.json"),
        ),
        response(
            "201 Created",
            r#"{"success":true,"code":0,"data":{"uuid":"enp-00000000-0000-0000-0000-0000000000c1","name":"demo\ud800"}}"#,
        ),
        response("200 OK", &state),
    ]);
    let client = endpoint_session(&project, &endpoint);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateEndpointInput::new(
        "demo�",
        PAID_VPC,
        "sub-00000000-0000-0000-0000-000000000003",
    );
    input.max_price = 712400.0;
    let output = client.create_endpoint(&ctx, input).await.unwrap();
    assert_eq!(output.endpoint_id, PAID_ENDPOINT);
    assert_eq!(endpoint.heads().len(), 11);
}

#[tokio::test]
async fn nat_delete_guard_omits_the_status_of_truncated_vpc_inventory() {
    use crate::testing::with_write_loopback;
    let server = Server::start(vec![Response::Http {
        status: "403 Forbidden".into(),
        headers: vec![("Content-Length".into(), "100".into())],
        body: b"{}".to_vec(),
    }]);
    let config = Config::new(with_write_loopback(
        Product::Vnetwork,
        options().with_project_id(PAID_PROJECT.into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let error = Client::new(&config)
        .delete_nat_instance(
            &ctx,
            DeleteNatInstanceInput::new(PAID_ZONE, "vpc-1", "nat-1"),
        )
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.message()),
        (None, "NAT response withheld")
    );
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.heads().len(), 1);
}

const NAT_AUTO: &str = r#"{"code":200,"data":{"data":[{"product":"vserver","artifactType":"nat","artifactId":"nat-1","status":"active","billingType":"PREPAID","channel":3,"renewType":"AUTO-RENEW","renewPeriod":1,"billingElements":[{"sku":"nat.s-standard","quantity":1}]}]}}"#;
const NAT_MANUAL: &str = r#"{"code":200,"data":{"data":[{"product":"vserver","artifactType":"nat","artifactId":"nat-1","status":"active","billingType":"PREPAID","channel":3,"renewType":"MANUAL","renewPeriod":null,"billingElements":[{"sku":"nat.s-standard","quantity":1}]}]}}"#;

fn nat_order_server(acknowledgement: &str) -> Server {
    Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json"),
        ),
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
            &include_str!("../../../tests/fixtures/network/nat_quote_vpcs.json")
                .replace("proj-1", PAID_PROJECT)
                .replace("zone-1", PAID_ZONE)
                .replace("az-1", "az-home"),
        ),
        response("200 OK", NO_NATS),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/quote_create_nat.json"),
        ),
        response("201 Created", acknowledgement),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_write_inventory.json"),
        ),
    ])
}

async fn create_nat(
    vnetwork: &Server,
    billing: &Server,
) -> Result<CreateNatInstanceOutput, Box<crate::Failure<CreateNatInstanceOutput>>> {
    use crate::testing::with_write_loopback;
    let options = with_write_loopback(
        Product::Vnetwork,
        options().with_project_id(PAID_PROJECT.into()),
        vnetwork,
    );
    let config = Config::new(with_write_loopback(Product::Billing, options, billing)).unwrap();
    let mut input = CreateNatInstanceInput::new("example", PAID_ZONE, "az-1", "package-1", "vpc-1");
    input.max_price = 712400.0;
    Client::new(&config)
        .create_nat_instance(&RequestContext::new().0, input)
        .await
        .map_err(Box::new)
}

#[tokio::test]
async fn nat_create_reconciles_without_mutating_billing_after_unconfirmed_acceptance() {
    let vnetwork = nat_order_server(r#"{"success":false,"code":0}"#);
    let billing = Server::start(vec![response("200 OK", NAT_AUTO)]);
    let failure = create_nat(&vnetwork, &billing).await.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.partial().unwrap().auto_renew, Some(true));
    assert_eq!(vnetwork.requests(), 8);
    assert_eq!(billing.requests(), 1);
}

#[tokio::test]
async fn nat_create_orders_once_and_confirms_manual_billing_after_renewal_write() {
    let vnetwork = nat_order_server(r#"{"success":true,"code":0}"#);
    let billing = Server::start(vec![
        response("200 OK", NAT_AUTO),
        response("200 OK", r#"{"code":200,"data":{"accountId":123456}}"#),
        response(
            "200 OK",
            r#"{"code":200,"data":{"successAll":true,"errorAutoRenewResources":[]}}"#,
        ),
        response("200 OK", NAT_MANUAL),
    ]);
    let output = create_nat(&vnetwork, &billing).await.unwrap();
    assert_eq!(output.nat_instance.unwrap().uuid, "nat-1");
    assert_eq!(output.total_price, 712400.0);
    assert_eq!(output.auto_renew, Some(false));
    let requests = vnetwork.heads();
    assert_eq!(requests.len(), 8);
    assert!(
        requests[6]
            .starts_with(format!("POST /vnetwork/v1/{PAID_ZONE}/{PAID_PROJECT}/nats ").as_bytes())
    );
    let order: serde_json::Value = serde_json::from_slice(&vnetwork.bodies()[6]).unwrap();
    assert!(
        order
            == serde_json::json!({"resourceType":"nat","action":"create","resourceInfo":{"isPoc":false,"isEnableAutoRenew":false,"isBuyMorePoc":false,"natName":"example","packageUuid":"package-1","vpcUuid":"vpc-1","regionUuid":PAID_ZONE,"projectUuid":PAID_PROJECT},"tagDetails":[]}),
        "NAT order request differed"
    );
    let requests = billing.heads();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].starts_with(b"PUT /gateway/api/v1/resources/autoRenew "));
    let update: serde_json::Value = serde_json::from_slice(&billing.bodies()[2]).unwrap();
    assert!(
        update
            == serde_json::json!([{"product":"vserver","artifactType":"nat","artifactId":"nat-1","channel":3,"autoRenewInfo":{"isEnable":false,"period":43200}}]),
        "NAT renewal request differed"
    );
}
