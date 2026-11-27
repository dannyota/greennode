use super::*;
use crate::testing::{Product, with_write_loopback};

const PASS: &str = "Synthetic_Password32";
const BEARER: &str = "synthetic-bearer";
const DETAIL: &[u8] = br#"{"code":200,"data":{"id":"db-1","name":"demo-db","status":"ACTIVE","datastoreType":"MySQL","datastoreVersion":"8.0","quotaPackageId":"1","publicAccess":false,"backupAuto":false,"zoneId":"zone-1","projectId":"project-1"}}"#;

fn fixture(server: &Server) -> Client {
    Client::new(
        &Config::new(with_write_loopback(
            Product::Database,
            ConfigOptions::default()
                .with_region("hcm-3".into())
                .with_project_id("project-1".into())
                .with_static_token(Secret::new(BEARER)),
            server,
        ))
        .unwrap(),
    )
}

fn create_input() -> CreateDatabaseInstanceInput {
    CreateDatabaseInstanceInput::new(
        "demo-db",
        "zone-1",
        1,
        "SSD",
        20,
        "vpc-1",
        "subnet-1",
        "appdb",
        "admin",
        Secret::new(PASS),
    )
}

fn catalogs() -> Vec<Response> {
    [
        br#"{"code":200,"data":[{"type":"MySQL","version":"8.0"}]}"#.as_slice(),
        br#"{"code":200,"data":[{"uuid":"zone-1","status":"ACTIVE"}]}"#,
        br#"{"code":200,"data":[{"id":1}]}"#,
        br#"{"code":200,"data":{"data":[{"type":"SSD","minVolumeSize":10,"maxVolumeSize":100}]}}"#,
        br#"{"code":200,"data":[{"uuid":"vpc-1","subnets":[{"uuid":"subnet-1"}]}]}"#,
    ]
    .map(|body| response("200 OK", body))
    .into()
}

#[tokio::test]
async fn create_checks_catalogs_sends_once_and_settles() {
    let mut responses = catalogs();
    responses.push(response(
        "200 OK",
        br#"{"code":200,"data":[{"resourceId":"db-1"}]}"#,
    ));
    responses.push(response("200 OK", DETAIL));
    let server = Server::start(responses);
    let (ctx, _) = RequestContext::new();
    let output = fixture(&server)
        .create_database_instance(&ctx, create_input())
        .await
        .unwrap();
    assert_eq!(output.instance.id, "db-1");
    assert_eq!(server.requests(), 7);
    let heads = server.heads();
    for (head, route) in heads.iter().zip([
        "GET /vdb-relational/v1/database-instances/datastore ",
        "GET /vdb-relational/v1/database-instances/zones ",
        "GET /vdb-relational/v1/database-instances/flavors?type=MySQL&version=8.0&zoneId=zone-1 ",
        "GET /vdb-relational/v1/database-instances/volume/types?zoneId=zone-1 ",
        "GET /vdb-relational/v1/database-instances/networks/subnets?zoneId=zone-1 ",
        "POST /vdb-relational/v1/payment/database-instances ",
        "GET /vdb-relational/v1/database-instances/id/db-1 ",
    ]) {
        assert!(head.starts_with(route.as_bytes()));
    }
    assert!(
        String::from_utf8_lossy(&heads[5])
            .to_ascii_lowercase()
            .contains("user-type: iam_user")
    );
    let bodies = server.bodies();
    let body: serde_json::Value = serde_json::from_slice(&bodies[5]).unwrap();
    assert!(
        body == serde_json::json!({
            "user": {"name":"admin","password":PASS,"databases":[{"name":"appdb"}]},
            "databases": [{"name":"appdb","characterSet":"utf8mb4","collate":"utf8mb4_general_ci"}],
            "name":"demo-db","volumeType":"SSD","volumeSize":20,"datastoreType":"MySQL",
            "datastoreVersion":"8.0","netIds":["subnet-1"],"publicAccess":false,
            "backupAuto":false,"packageId":"1","poc":false,"locateZoneId":"zone-1"
        })
    );
    assert!(
        bodies
            .iter()
            .enumerate()
            .all(|(i, body)| i == 5 || body.is_empty())
    );
}

#[tokio::test]
async fn create_does_not_send_the_password_in_an_instance_name() {
    let mut replies = catalogs();
    replies.push(response(
        "200 OK",
        br#"{"code":200,"data":[{"resourceId":"db-1"}]}"#,
    ));
    replies.push(response("200 OK", DETAIL));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let mut input = create_input();
    input.name = PASS.into();
    let result = fixture(&server).create_database_instance(&ctx, input).await;
    assert!(
        server
            .heads()
            .iter()
            .all(|head| !head.starts_with(b"POST "))
    );
    let failure = result.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.partial().is_none());
}

#[tokio::test]
async fn create_confirmation_withholds_an_encoded_password_in_instance_name() {
    let detail = std::str::from_utf8(DETAIL)
        .unwrap()
        .replace("demo-db", &PASS.replace('_', "%5f"));
    let mut replies = catalogs();
    replies.push(response(
        "200 OK",
        br#"{"code":200,"data":[{"resourceId":"db-1"}]}"#,
    ));
    replies.push(response("200 OK", detail.as_bytes()));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let outcome = fixture(&server)
        .create_database_instance(&ctx, create_input())
        .await;
    assert!(outcome.is_err());
    let failure = outcome.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let partial = &failure.partial().unwrap().instance;
    assert_eq!(partial.id, "db-1");
    assert!(partial.name.is_empty());
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn create_rejects_untrusted_provenance_before_catalogs() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_database_instance(&ctx, create_input())
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidConfig));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(server.heads().is_empty());
}

#[tokio::test]
async fn create_uncertainty_does_not_replay_and_withholds_reflected_detail() {
    for (status, reply, detail) in [
        ("503 Service Unavailable", "{}", None),
        (
            "200 OK",
            r#"{"code":200,"data":[{"resourceId":"db-1"}]}"#,
            Some(
                String::from_utf8(DETAIL.to_vec())
                    .unwrap()
                    .replace("demo-db", PASS),
            ),
        ),
    ] {
        let mut responses = catalogs();
        responses.push(response(status, reply.as_bytes()));
        if let Some(detail) = &detail {
            responses.push(response("200 OK", detail.as_bytes()));
        }
        let server = Server::start(responses);
        let (ctx, _) = RequestContext::new();
        let failure = fixture(&server)
            .create_database_instance(&ctx, create_input())
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::NotSettled);
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(
            failure.partial().map(|out| out.instance.id.as_str()),
            detail.as_ref().map(|_| "db-1")
        );
        assert!(!format!("{failure:?}").contains(PASS));
        assert_eq!(server.requests(), 6 + usize::from(detail.is_some()));
    }
}

#[tokio::test]
async fn create_definitive_rejections_keep_status_and_allowlisted_fields() {
    for (status, number, code) in [
        ("400 Bad Request", 400, "BadRequest"),
        ("401 Unauthorized", 401, "Unauthorized"),
        ("403 Forbidden", 403, "Forbidden"),
        ("404 Not Found", 404, "NotFound"),
        ("409 Conflict", 409, "Conflict"),
        ("429 Too Many Requests", 429, "Throttled"),
    ] {
        let mut responses = catalogs();
        let body = if number == 400 {
            serde_json::json!({"code": BEARER, "message": PASS, "fieldError": [
                {"field":"name"}, {"field":"user.password","message":PASS},
                {"field":"databases[0].name"}, {"field":BEARER}, {"field":"password"}
            ]})
            .to_string()
        } else {
            serde_json::json!({"code": BEARER, "message": PASS}).to_string()
        };
        responses.push(response(status, body.as_bytes()));
        let server = Server::start(responses);
        let (ctx, _) = RequestContext::new();
        let failure = fixture(&server)
            .create_database_instance(&ctx, create_input())
            .await
            .unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.operation(), "database.CreateDatabaseInstance");
        assert_eq!(api.status(), Some(number));
        assert!(api.code() == code);
        assert!(
            api.message()
                == if number == 400 {
                    "invalid input rejected by the API: Name, Password, DatabaseName"
                } else {
                    "request rejected by the API"
                }
        );
        assert!(failure.partial().is_none());
        assert_eq!(server.requests(), 6);
    }
}

fn delete_reads(backups: &[u8]) -> Vec<Response> {
    [
        DETAIL,
        br#"{"code":200,"data":{"projectId":"project-1","data":[{"id":"other","name":"other","status":"ACTIVE"}],"pageObject":{"number":1,"size":1,"totalElements":2,"totalPages":2}}}"#,
        br#"{"code":200,"data":{"projectId":"project-1","data":[{"id":"db-1","name":"demo-db","status":"ACTIVE"}],"pageObject":{"number":2,"size":1,"totalElements":2,"totalPages":2}}}"#,
        br#"{"code":200,"data":[{"uuid":"zone-1","status":"ACTIVE"}]}"#,
        backups,
    ].map(|body| response("200 OK", body)).into()
}

#[tokio::test]
async fn database_delete_rechecks_all_preflight_reads_with_the_write_bearer() {
    use std::{
        future::{Future, ready},
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Rotating(AtomicUsize);
    impl crate::CredentialsProvider for Rotating {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, Error>> + Send + 'a>> {
            let token = if self.0.fetch_add(1, Ordering::Relaxed) < 5 {
                BEARER
            } else {
                "toy-delete-bearer"
            };
            Box::pin(ready(Ok(crate::Token::new(Secret::new(token), None))))
        }

        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
            Box::pin(ready(Ok(())))
        }
    }
    for read in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
        let mut replies = delete_reads(br#"{"code":200,"data":[]}"#);
        if let Some(read) = read {
            let Response::Http { body, .. } = &replies[read] else {
                unreachable!()
            };
            let mut decoded: serde_json::Value = serde_json::from_slice(body).unwrap();
            decoded["note"] = "toy-delete-bearer".into();
            replies[read] = response("200 OK", decoded.to_string().as_bytes());
        }
        replies.push(response("204 No Content", b""));
        replies.push(response("404 Not Found", b"{}"));
        let server = Server::start(replies);
        let client = Client::new(
            &Config::new(with_write_loopback(
                Product::Database,
                ConfigOptions::default()
                    .with_region("hcm-3".into())
                    .with_project_id("project-1".into())
                    .with_credentials_provider(Arc::new(Rotating(0.into()))),
                &server,
            ))
            .unwrap(),
        );
        let result = client
            .delete_database_instance(
                &RequestContext::new().0,
                DeleteDatabaseInstanceInput::new("db-1"),
            )
            .await;
        assert_eq!(
            result
                .map(|output| output.deleted)
                .map_err(|error| error.write_dispatch()),
            if read.is_some() {
                Err(WriteDispatch::NotSent)
            } else {
                Ok(true)
            },
            "{read:?}"
        );
        assert_eq!(server.requests(), if read.is_some() { 5 } else { 7 });
    }
}

#[tokio::test]
async fn delete_proves_project_across_pages_and_checks_backups() {
    for backed_up in [false, true] {
        let mut responses = delete_reads(if backed_up {
            br#"{"code":200,"data":[{"id":"backup-1"}]}"#
        } else {
            br#"{"code":200,"data":[]}"#
        });
        if !backed_up {
            responses.extend([
                response("204 No Content", b""),
                response("404 Not Found", b"{}"),
            ]);
        }
        let server = Server::start(responses);
        let (ctx, _) = RequestContext::new();
        let result = fixture(&server)
            .delete_database_instance(&ctx, DeleteDatabaseInstanceInput::new("db-1"))
            .await;
        assert_eq!(server.requests(), if backed_up { 5 } else { 7 });
        let heads = server.heads();
        assert!(
            heads[2].starts_with(
                b"GET /vdb-relational/v1/database-instances?pageNumber=2&pageSize=100 "
            )
        );
        assert!(heads[4].starts_with(b"GET /vdb-relational/v1/backups/insId/db-1 "));
        if backed_up {
            let failure = result.unwrap_err();
            assert_eq!(
                failure.error().detail::<DeleteRefused>(),
                Some(&DeleteRefused)
            );
            assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        } else {
            assert!(result.unwrap().deleted);
            assert!(
                heads[5].starts_with(b"POST /vdb-relational/v1/database-instances/db-1/delete ")
            );
            let body: serde_json::Value = serde_json::from_slice(&server.bodies()[5]).unwrap();
            assert_eq!(
                body,
                serde_json::json!({"databaseInstances":[{"instancesId":"db-1","config":{"createFinalBackup":false,"deleteAllBackup":false}}],"action":"delete","resType":"dbaas"})
            );
        }
    }
}

#[tokio::test]
async fn delete_refuses_noncanonical_safety_fields() {
    let detail = std::str::from_utf8(DETAIL).unwrap();
    for (step, body) in [
        (0, detail.replace("\"zoneId\"", "\"Replicas\":[\"replica-1\"],\"zoneId\"")),
        (0, detail.replace("\"zoneId\"", "\"Owner\":false,\"zoneId\"")),
        (0, detail.replace("projectId", "ProjectID")),
        (2, r#"{"code":200,"data":{"projectId":"project-1","data":[{"id":"db-1","name":"demo-db","status":"ACTIVE","ProjectID":"project-other"}],"pageObject":{"number":2,"size":1,"totalElements":2,"totalPages":2}}}"#.to_owned()),
    ] {
        let mut responses = delete_reads(br#"{"code":200,"data":[]}"#);
        responses[step] = response("200 OK", body.as_bytes());
        responses.extend([response("204 No Content", b""), response("404 Not Found", b"{}")]);
        let server = Server::start(responses);
        let (ctx, _) = RequestContext::new();
        let result = fixture(&server).delete_database_instance(&ctx, DeleteDatabaseInstanceInput::new("db-1")).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), step + 1);
    }
}

#[tokio::test]
async fn delete_requires_an_integer_backup_envelope_code() {
    let mut responses =
        delete_reads(br#"{"code":{"$serde_json::private::Number":"200"},"data":[]}"#);
    responses.extend([
        response("204 No Content", b""),
        response("404 Not Found", b"{}"),
    ]);
    let server = Server::start(responses);
    let (ctx, _) = RequestContext::new();
    let result = fixture(&server)
        .delete_database_instance(&ctx, DeleteDatabaseInstanceInput::new("db-1"))
        .await;
    assert!(result.is_err());
    let failure = result.unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn delete_does_not_confirm_absence_from_a_truncated_404() {
    let mut responses = delete_reads(br#"{"code":200,"data":[]}"#);
    responses.extend([
        response("204 No Content", b""),
        Response::RawClose(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\n{}".to_vec()),
    ]);
    let server = Server::start(responses);
    let (ctx, _) = RequestContext::new();
    let result = fixture(&server)
        .delete_database_instance(&ctx, DeleteDatabaseInstanceInput::new("db-1"))
        .await;
    assert!(result.is_err());
    let failure = result.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(
        failure.partial().map(|out| (out.id.as_str(), out.deleted)),
        Some(("db-1", false))
    );
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn create_does_not_poll_a_conflicting_receipt_id() {
    let mut responses = catalogs();
    responses.push(response(
        "200 OK",
        br#"{"code":200,"data":[{"resourceId":"db-1","ResourceId":"db-2"}]}"#,
    ));
    responses.push(response("200 OK", DETAIL));
    let server = Server::start(responses);
    let (ctx, _) = RequestContext::new();
    let result = fixture(&server)
        .create_database_instance(&ctx, create_input())
        .await;
    assert!(result.is_err());
    let failure = result.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 6);
}
