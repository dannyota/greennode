use super::*;

#[tokio::test]
async fn certificate_unclean_not_found_requires_inventory_reconciliation() {
    for dispatched in [false, true] {
        for (refusal, reconciled) in [
            (response("404 Not Found", "{}"), false),
            (
                response("404 Not Found", r#"{"unknown":"toy-secret"}"#),
                true,
            ),
            (Server::oversized("404 Not Found"), true),
        ] {
            let mut replies = Vec::new();
            if dispatched {
                replies.push(response("200 OK", r#"{"uuid":"cert-1","inUse":false}"#));
            }
            replies.extend([
                refusal,
                response(
                    "200 OK",
                    r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
                ),
            ]);
            let server = Server::start(replies);
            let failure = client(&server, CANARY)
                .delete_certificate(
                    &RequestContext::new().0,
                    DeleteCertificateInput {
                        certificate_id: "cert-1".into(),
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(
                failure.write_dispatch(),
                if dispatched {
                    WriteDispatch::MayHaveBeenSent
                } else {
                    WriteDispatch::NotSent
                }
            );
            assert_eq!(
                server.requests(),
                1 + usize::from(dispatched) + usize::from(reconciled)
            );
            assert!(failure.error().has_class(ErrorClass::NotFound));
        }
    }
}

#[tokio::test]
async fn certificate_unclean_404_with_present_or_incomplete_inventory_stays_uncertain() {
    for inventory in [
        r#"{"listData":[{"uuid":"cert-1"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
        r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":2,"totalItem":3}"#,
    ] {
        let server = Server::start(vec![
            response("200 OK", r#"{"uuid":"cert-1","inUse":false}"#),
            Server::oversized("404 Not Found"),
            response("200 OK", inventory),
        ]);
        let failure = client(&server, CANARY)
            .delete_certificate(
                &RequestContext::new().0,
                DeleteCertificateInput {
                    certificate_id: "cert-1".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.error().has_identity(ErrorClass::NotSettled));
        assert!(!failure.error().has_identity(ErrorClass::NotFound));
        assert_eq!(
            failure
                .error()
                .detail::<crate::HttpStatus>()
                .map(|status| status.as_u16()),
            Some(404)
        );
        assert_eq!(server.requests(), 3);
    }
}

#[tokio::test]
async fn certificate_inventory_retries_the_original_query_after_redirect() {
    let server = Server::start(vec![
        response("200 OK", r#"{"uuid":"cert-1","inUse":false}"#),
        Server::oversized("404 Not Found"),
        Response::Http {
            status: "307 Temporary Redirect".into(),
            headers: vec![
                (
                    "Location".into(),
                    "/regional/v2/proj-1/cas?name=other".into(),
                ),
                ("Content-Length".into(), "0".into()),
            ],
            body: Vec::new(),
        },
        response(
            "200 OK",
            r#"{"listData":[{"uuid":"cert-1"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
        ),
    ]);
    let failure = client_with_options(
        &server,
        CANARY,
        ConfigOptions::default().with_clock(crate::testing::retry_clock()),
    )
    .delete_certificate(
        &RequestContext::new().0,
        DeleteCertificateInput {
            certificate_id: "cert-1".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    for head in &heads[2..] {
        assert!(head.starts_with(b"GET /regional/v2/proj-1/cas?name=&page=1&size=10000 "));
    }
}

#[tokio::test]
async fn no_wait_load_balancer_delete_checks_discarded_transport_credentials() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("202 Accepted", r#"{"ignored":"toy-secret"}"#),
    ]);
    let failure = client(&server, CANARY)
        .delete_load_balancer(&RequestContext::new().0, delete_input(true))
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 2);
}
