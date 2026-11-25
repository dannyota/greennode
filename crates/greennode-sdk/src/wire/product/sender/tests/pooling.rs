use super::*;

#[test]
fn observed_error_does_not_store_response_headers() {
    assert_eq!(
        std::mem::size_of::<send::Observed>(),
        std::mem::size_of::<http::StatusCode>()
    );
}

#[tokio::test]
async fn nonidempotent_retry_uses_fresh_connections() {
    let server = Server::start(vec![
        response("429 Too Many Requests", &[]),
        response("200 OK", &[]),
    ]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let result = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &operation(Method::POST, redirect::Retry::NonIdempotent),
        Auth::None,
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn once_requests_use_fresh_connections() {
    let server = Server::start(vec![response("200 OK", &[]), response("200 OK", &[])]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let op = operation(Method::GET, redirect::Retry::Once);
    let request = request(&server.origin);
    for _ in 0..2 {
        let result = run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &op,
            Auth::None,
            &request,
            &sleeper,
            &mut || 0
        )
        .await
        .unwrap();
        assert_eq!(result.status, 200);
    }
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn repeatable_reads_use_pooled_connections() {
    let server = Server::start(vec![
        response("429 Too Many Requests", &[]),
        response("200 OK", &[]),
    ]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let result = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::None,
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn repeatable_writes_use_fresh_connections() {
    let server = Server::start(vec![
        response("429 Too Many Requests", &[]),
        response("200 OK", &[]),
    ]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let mut op = operation(Method::POST, redirect::Retry::Repeatable);
    op.class = redirect::Classification::Write;
    let result = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &op,
        Auth::None,
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(server.requests(), 2);
    assert_eq!(server.connections(), 2);
}
