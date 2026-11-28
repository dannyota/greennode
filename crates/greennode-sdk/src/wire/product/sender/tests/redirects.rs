use super::*;

#[tokio::test]
async fn cross_host_redirect_retries_original_and_never_sends_to_destination() {
    let other = Server::start(vec![response("200 OK", &[])]);
    let target = format!("{}/other", other.origin);
    let redirect = || response("302 Found", &[("Location", &target)]);
    let server = Server::start(vec![redirect(), redirect()]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::Redirect(_),
            ..
        })
    ));
    assert_eq!(server.requests(), 2);
    assert_eq!(sleeper.0.load(Ordering::SeqCst), 1);
    assert_eq!(other.requests(), 0);
}

#[tokio::test]
async fn every_repeatable_get_redirect_refusal_retries_the_original_endpoint() {
    for refusal in [
        "scheme", "limit", "cycle", "fragment", "userinfo", "invalid",
    ] {
        let server = Server::start_with(|address| {
            let targets = match refusal {
                "scheme" => vec![format!("http://{address}/unsafe")],
                "limit" => (0..10).map(|hop| format!("/hop-{hop}")).collect(),
                "cycle" => vec!["/".into()],
                "fragment" => vec!["/unsafe#fragment".into()],
                "userinfo" => vec![format!("https://user@{address}/unsafe")],
                _ => vec!["/bad%zz".into()],
            };
            targets
                .into_iter()
                .map(|target| response("302 Found", &[("Location", &target)]))
                .chain([response("200 OK", &[])])
                .collect()
        });
        let (ctx, clock, deadline) = ctx();
        let sleeper = Immediate::default();
        let result = run!(
            &client(&server),
            &ctx,
            &clock,
            deadline,
            &operation(Method::GET, redirect::Retry::Repeatable),
            Auth::None,
            &request(&server.origin),
            &sleeper,
            &mut || 0
        )
        .await;
        assert!(result.is_ok_and(|reply| reply.status == 200), "{refusal}");
        assert_eq!(server.requests(), if refusal == "limit" { 11 } else { 2 });
        assert!(server.heads().last().unwrap().starts_with(b"GET / "));
        assert_eq!(sleeper.0.load(Ordering::SeqCst), 1);
    }
}
