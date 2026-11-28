use super::*;

#[tokio::test]
async fn server_creation_rechecks_acknowledgement_before_using_confirmation() {
    for status in ["ACTIVE", "ERROR"] {
        let server = TestServer::start(vec![
            inventory(),
            response(r#"{"optimumPrice":20}"#),
            http(
                "202 Accepted",
                r#"{"data":{"uuid":"server-1"},"note":"later-receipt-bearer"}"#,
            ),
            response(&serde_json::json!({"data":{"uuid":"server-1","status":status}}).to_string()),
        ]);
        let failure = receipt_client(&server, 3)
            .create_server(&RequestContext::new().0, server_order())
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::NotSettled);
        assert!(failure.partial().is_none());
        assert_eq!(server.requests(), 3);
    }
}

#[tokio::test]
async fn duplicate_server_name_rechecks_all_inventory_receipts() {
    let first = r#"{"listData":[{"uuid":"server-2","name":"later-receipt-bearer"}],"page":1,"pageSize":10000,"totalPage":2,"totalItem":2}"#;
    let second = serde_json::json!({"listData":[{"uuid":"server-3","name":server_order().name}],"page":2,"pageSize":10000,"totalPage":2,"totalItem":2});
    let server = TestServer::start(vec![response(first), response(&second.to_string())]);
    let failure = receipt_client(&server, 1)
        .create_server(&RequestContext::new().0, server_order())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::RefusedGuard>()
            .is_some()
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn create_server_malformed_acknowledgement_is_statusless() {
    let server = TestServer::start(vec![
        inventory(),
        response(r#"{"optimumPrice":20}"#),
        http("202 Accepted", "{"),
    ]);
    let failure = server_order_client(&server)
        .create_server(&RequestContext::new().0, server_order())
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::DecodeFailure>()
            .is_some()
    );
    assert_eq!(failure.error().api_error().unwrap().status(), None);
    assert_eq!(server.requests(), 3);
}
