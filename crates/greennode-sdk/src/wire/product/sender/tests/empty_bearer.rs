use super::*;

#[tokio::test]
async fn empty_configured_bearer_is_an_auth_401_without_a_send() {
    let server = Server::start(vec![]);
    let source = Provider {
        empty: true,
        ..Provider::default()
    };
    let auth = PublicSource(&source);
    let (ctx, clock, deadline) = ctx();
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&auth),
        &request(&format!("{}/items", server.origin)),
        &Immediate::default(),
        &mut || 0
    )
    .await;
    let Err(failure) = result else {
        panic!("empty bearer was accepted");
    };
    let Error::Provider(error) = failure.error else {
        panic!("missing auth failure");
    };
    assert_eq!(error.class(), crate::ErrorClass::Auth);
    assert_eq!(
        error.api_error().and_then(crate::ApiError::status),
        Some(401)
    );
    assert!(server.heads().is_empty());
    assert_eq!(source.count.load(Ordering::SeqCst), 1);
    assert!(source.rejected.lock().unwrap().is_empty());
}
