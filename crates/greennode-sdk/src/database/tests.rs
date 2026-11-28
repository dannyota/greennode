use super::*;

#[cfg(feature = "test-support")]
mod writes;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

fn response(status: &str, body: &[u8]) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}
fn client(server: &Server) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("synthetic-bearer"))
            .with_endpoint_overrides(EndpointOverrides {
                database: Some(server.origin.clone()),
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
async fn instance_detail_checks_id_and_errors() {
    let server = Server::start(vec![
        response(
            "200 OK",
            br#"{"code":200,"data":{"id":"db_one","name":"db","status":"ACTIVE"}}"#,
        ),
        response(
            "200 OK",
            br#"{"code":200,"data":{"id":"other","name":"db","status":"ACTIVE"}}"#,
        ),
        response("404 Not Found", b"{}"),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let input = || GetDatabaseInstanceInput {
        id: "db_one".into(),
    };
    assert_eq!(
        client
            .get_database_instance(&ctx, input())
            .await
            .unwrap()
            .instance
            .id,
        "db_one"
    );
    let error = client
        .get_database_instance(&ctx, input())
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    let error = client
        .get_database_instance(&ctx, input())
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::NotFound);
    for id in ["", "../bad", "a.b"] {
        let error = client
            .get_database_instance(&ctx, GetDatabaseInstanceInput { id: id.into() })
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    }
    assert_eq!(server.requests(), 3);
    assert!(server.heads()[0].starts_with(b"GET /vdb-relational/v1/database-instances/id/db_one "));
}

#[tokio::test]
async fn database_rejects_reflection_and_missing_subnets() {
    let server = Server::start(vec![
        response(
            "200 OK",
            br#"{"code":200,"data":[{"type":"mysql","version":"synthetic-bearer"}]}"#,
        ),
        response("200 OK", br#"{"code":200,"data":[{"uuid":"network-1"}]}"#),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .list_database_engine_versions(&ctx, ListDatabaseEngineVersionsInput)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    let failure = client
        .list_database_networks(
            &ctx,
            ListDatabaseNetworksInput {
                zone_id: "zone-1".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn database_reads_and_api_errors() {
    let cases: [(&[u8], &str); 6] = [
        (br#"{"code":200,"data":[{"type":"mysql","version":"8"}]}"#, "/vdb-relational/v1/database-instances/datastore"),
        (br#"{"code":200,"data":[{"uuid":"zone-1","status":"ACTIVE"}]}"#, "/vdb-relational/v1/database-instances/zones"),
        (br#"{"code":200,"data":[{"id":1}]}"#, "/vdb-relational/v1/database-instances/flavors?type=mysql&version=8&zoneId=zone-1"),
        (br#"{"code":200,"data":{"projectId":"project-1","data":[{"type":"SSD","minVolumeSize":10,"maxVolumeSize":100}]}}"#, "/vdb-relational/v1/database-instances/volume/types?zoneId=zone-1"),
        (br#"{"code":200,"data":[{"uuid":"network-1","subnets":[{"uuid":"subnet-1","networkUuid":"network-1"}]}]}"#, "/vdb-relational/v1/database-instances/networks/subnets?zoneId=zone-1"),
        (br#"{"code":200,"data":{"projectId":"project-1","data":[{"id":"instance-1","name":"db","status":"ACTIVE"}],"pageObject":{"number":1}}}"#, "/vdb-relational/v1/database-instances?pageNumber=1&pageSize=100"),
    ];
    for (index, (body, path)) in cases.into_iter().enumerate() {
        let server = Server::start(vec![
            response("200 OK", body),
            response("403 Forbidden", br#"{"message":"private details"}"#),
        ]);
        let client = client(&server);
        let (ctx, _) = RequestContext::new();
        for ok in [true, false] {
            let (success, class) = match index {
                0 => {
                    let result = client
                        .list_database_engine_versions(&ctx, ListDatabaseEngineVersionsInput)
                        .await;
                    (
                        result.as_ref().is_ok_and(|v| v.items[0].version == "8"),
                        result.err().map(|e| e.error().class()),
                    )
                }
                1 => {
                    let result = client
                        .list_database_zones(&ctx, ListDatabaseZonesInput)
                        .await;
                    (
                        result.as_ref().is_ok_and(|v| v.items[0].uuid == "zone-1"),
                        result.err().map(|e| e.error().class()),
                    )
                }
                2 => {
                    let result = client
                        .list_database_flavors(
                            &ctx,
                            ListDatabaseFlavorsInput {
                                engine: "mysql".into(),
                                engine_version: "8".into(),
                                zone_id: "zone-1".into(),
                            },
                        )
                        .await;
                    (
                        result.as_ref().is_ok_and(|v| v.items[0].id == 1),
                        result.err().map(|e| e.error().class()),
                    )
                }
                3 => {
                    let result = client
                        .list_database_storage_types(
                            &ctx,
                            ListDatabaseStorageTypesInput {
                                zone_id: "zone-1".into(),
                            },
                        )
                        .await;
                    (
                        result.as_ref().is_ok_and(|v| {
                            v.project_id == "project-1" && v.items[0].min_volume_size == 10
                        }),
                        result.err().map(|e| e.error().class()),
                    )
                }
                4 => {
                    let result = client
                        .list_database_networks(
                            &ctx,
                            ListDatabaseNetworksInput {
                                zone_id: "zone-1".into(),
                            },
                        )
                        .await;
                    (
                        result
                            .as_ref()
                            .is_ok_and(|v| v.items[0].subnets[0].uuid == "subnet-1"),
                        result.err().map(|e| e.error().class()),
                    )
                }
                _ => {
                    let result = client
                        .list_database_instances(&ctx, ListDatabaseInstancesInput::default())
                        .await;
                    (
                        result
                            .as_ref()
                            .is_ok_and(|v| v.items[0].id == "instance-1" && v.page.number == 1),
                        result.err().map(|e| e.error().class()),
                    )
                }
            };
            if ok {
                assert!(success, "operation {index} failed");
            } else {
                assert_eq!(class, Some(ErrorClass::PermissionDenied));
            }
        }
        assert_eq!(server.requests(), 2);
        assert!(server.heads()[0].starts_with(format!("GET {path} ").as_bytes()));
    }
}

#[tokio::test]
async fn database_detail_reports_an_oversized_200_as_invalid_response() {
    let server = Server::start(vec![Server::oversized("200 OK")]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .get_database_instance(&ctx, GetDatabaseInstanceInput { id: "db-1".into() })
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.operation(), api.status(), api.code()),
        ("GetDatabaseInstance", Some(200), "InvalidResponse")
    );
}

#[tokio::test]
async fn database_detail_oversized_redirect_is_withheld_without_status() {
    let server = Server::start(vec![Server::oversized("302 Found")]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .get_database_instance(&ctx, GetDatabaseInstanceInput { id: "db-1".into() })
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.operation(), api.status(), api.code(), api.message()),
        (
            "GetDatabaseInstance",
            None,
            "",
            "request failed; response withheld"
        )
    );
}
