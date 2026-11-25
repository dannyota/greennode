use super::*;
use crate::wire::product::{
    redirect::Retry,
    retry::{self, Outcome},
};

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
