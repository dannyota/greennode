use super::*;
use crate::wire::product::{
    redirect::Retry,
    retry::{self, Outcome},
};

#[tokio::test]
async fn expected_statuses_control_body_reads() {
    let expected = [http::StatusCode::CREATED, http::StatusCode::ACCEPTED];
    for (status, body_is_read) in [
        ("201 Created", true),
        ("202 Accepted", true),
        ("200 OK", false),
    ] {
        let server = Server::start(vec![response(status, &[("Content-Length", "2")], b"ok")]);
        let clock = crate::SystemClock;
        let client = client(&server);
        let mut evidence = send::Evidence::default();
        let result = send::once_with_expected(
            &client,
            req(Method::GET, &server.origin),
            10,
            Encoding::Identity,
            &clock,
            clock.monotonic_now() + Duration::from_secs(5),
            (&mut evidence, &expected),
        )
        .await
        .unwrap();
        assert_eq!(result.body == b"ok", body_is_read);
        assert_eq!(evidence.status, Some(result.status));
    }
}

#[tokio::test]
async fn post_head_error_is_never_a_retryable_send_failure() {
    let server = Server::start(vec![Response::RawClose(
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 9\r\n\r\nx".to_vec(),
    )]);
    let result = send(&client(&server), req(Method::GET, &server.origin), 100).await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("expected body error"),
    };
    let outcome = error.retry_outcome().unwrap();
    assert!(matches!(outcome, Outcome::Body));
    assert!(
        retry::decide(
            Retry::Repeatable,
            0,
            &retry::Config::default(),
            outcome,
            std::time::UNIX_EPOCH,
            0
        )
        .is_none()
    );
    assert!(
        retry::decide(
            Retry::Repeatable,
            0,
            &retry::Config::default(),
            Outcome::Status(503, None),
            std::time::UNIX_EPOCH,
            0
        )
        .is_some()
    );
}
