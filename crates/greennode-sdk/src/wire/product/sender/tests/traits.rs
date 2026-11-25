use super::*;

#[test]
fn sender_future_is_send() {
    fn assert_send<T: Send>(_: T) {}
    let server = Server::start(vec![]);
    let (ctx, clock, deadline) = ctx();
    let client = client(&server);
    let op = operation(Method::GET, redirect::Retry::Repeatable);
    let request = request(&server.origin);
    let sleeper = Immediate::default();
    let mut jitter = || 0;
    let future = run!(
        &client,
        &ctx,
        &clock,
        deadline,
        &op,
        Auth::None,
        &request,
        &sleeper,
        &mut jitter
    );
    assert_send(future);
}
