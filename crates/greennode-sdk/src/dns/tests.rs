use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

const ZONES: &str = include_str!("../../tests/fixtures/dns/list_hosted_zones.json");
const ZONE: &str = include_str!("../../tests/fixtures/dns/get_hosted_zone.json");
const RECORDS: &str = include_str!("../../tests/fixtures/dns/list_records.json");
const RECORD: &str = include_str!("../../tests/fixtures/dns/get_record.json");
const BEARER: &str = "synthetic-sdk-dns-bearer";

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn client(server: &Server, bearer: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(bearer))
            .with_project_id("unused-project".into())
            .with_endpoint_overrides(EndpointOverrides {
                dns: Some(format!("{}/vdns-api/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn dns_uses_the_global_endpoint_and_baseline_routes() {
    let server = Server::start(vec![
        response("200 OK", ZONES),
        response("200 OK", ZONES),
        response("200 OK", ZONE),
        response("200 OK", RECORDS),
        response("200 OK", RECORD),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let zones = client
        .list_hosted_zones(
            &ctx,
            ListHostedZonesInput {
                name: "app + root".into(),
                page: 2,
                size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(zones.items[0].id, "zone-1");
    assert_eq!(zones.items[0].assoc_vpc_map_region[0].region, "hcm-3");
    let _ = client
        .list_hosted_zones(
            &ctx,
            ListHostedZonesInput {
                page: -1,
                size: -2,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let zone = client
        .get_hosted_zone(&ctx, GetHostedZoneInput::new("zone-1"))
        .await
        .unwrap();
    assert_eq!(zone.hosted_zone.associated_vpc_ids, ["vpc-1"]);
    let mut input = ListRecordsInput::new("zone-1");
    input.name = "www".into();
    let records = client.list_records(&ctx, input).await.unwrap();
    assert_eq!(records.items[0].value[0].value, "<ip>");
    let record = client
        .get_record(&ctx, GetRecordInput::new("zone-1", "record-1"))
        .await
        .unwrap();
    assert_eq!(record.record.hosted_zone_id, "zone-1");
    let heads = server.heads();
    assert_eq!(heads.len(), 5);
    for (head, path) in heads.iter().zip([
        "hosted-zone?name=app+%2B+root&page=2&size=10",
        "hosted-zone?page=1&size=10000",
        "hosted-zone/zone-1",
        "hosted-zone/zone-1/record?name=www",
        "hosted-zone/zone-1/record/record-1",
    ]) {
        assert!(head.starts_with(format!("GET /vdns-api/v1/dns/{path} ").as_bytes()));
        assert!(
            !head
                .windows(b"unused-project".len())
                .any(|part| part == b"unused-project")
        );
    }
}

#[tokio::test]
async fn incomplete_record_list_keeps_redacted_sdk_partial_without_following_pages() {
    let bearer = "730019";
    for (total_page, total_item) in [(2, 1), (1, 730019)] {
        let body = format!(
            r#"{{"listData":[{{"recordId":"record-1","value":[{{"value":"reflected {bearer}"}}]}}],"page":{bearer},"pageSize":20,"totalPage":{total_page},"totalItem":{total_item}}}"#
        );
        let server = Server::start(vec![response("200 OK", &body)]);
        let client = client(&server, bearer);
        let (ctx, _) = RequestContext::new();
        let failure = client
            .list_records(&ctx, ListRecordsInput::new("zone-1"))
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotAWrite);
        let partial = failure.partial().unwrap();
        let output = serde_json::to_value(partial).unwrap();
        assert_eq!(output["Items"][0]["recordId"], "record-1");
        assert_eq!(output["Items"][0]["value"][0]["value"], "<redacted>");
        assert_eq!(output["Page"], "<redacted>");
        assert!(!output.to_string().contains(bearer));
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn malformed_json_and_wrong_shapes_have_distinct_sdk_details() {
    let server = Server::start(vec![
        response("200 OK", "{"),
        response("200 OK", r#"{"data":{"ttl":"invalid"}}"#),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    for detail in [DecodeError::MalformedJson, DecodeError::InvalidShape] {
        let failure = client
            .get_record(&ctx, GetRecordInput::new("zone-1", "record-1"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().detail::<DecodeError>(), Some(&detail));
        assert!(failure.partial().is_none());
    }
}

#[tokio::test]
async fn nested_and_offset_times_keep_go_representation() {
    let body = r#"{"data":{"hostedZoneId":"zone-1","createdAt":"2026-01-01T10:00:03.4500+07:00","deletedAt":null,"updatedAt":"2026-01-01T03:00:03Z"}}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let output = client
        .get_hosted_zone(&ctx, GetHostedZoneInput::new("zone-1"))
        .await
        .unwrap();
    let value = serde_json::to_value(output).unwrap();
    assert_eq!(
        value["HostedZone"]["createdAt"],
        "2026-01-01T10:00:03.45+07:00"
    );
    assert!(value["HostedZone"]["deletedAt"].is_null());
    assert_eq!(value["HostedZone"]["updatedAt"], "2026-01-01T03:00:03Z");
}
