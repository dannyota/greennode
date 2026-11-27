use super::*;
use crate::wire::product::options::AttemptCheck;

struct Signin;
impl BearerSource for Signin {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            Ok(BearerToken {
                token: Token::new(Secret::new("toy-first"), None),
                obtained_at: None,
                signin_credentials: Some(std::sync::Arc::new(SentCredentials::for_test(vec![
                    Secret::new("toy-signin"),
                ]))),
            })
        })
    }
    fn rejected<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

fn refuse_if(
    secret: &'static str,
) -> impl Fn(&SentCredentials) -> Option<Box<crate::Error>> + Sync {
    move |credentials| {
        credentials
            .reflects(secret.as_bytes())
            .then(|| Box::new(crate::Error::new(crate::ErrorClass::Api)))
    }
}

fn write(check: AttemptCheck<'_>) -> Operation<'_> {
    Operation {
        attempt_check: Some(check),
        class: redirect::Classification::Write,
        no_redirect: true,
        ..operation(Method::POST, redirect::Retry::Repeatable)
    }
}

#[tokio::test]
async fn first_attempt_compares_the_sign_in_credentials() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let check = refuse_if("toy-signin");
    let failure = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &write(AttemptCheck(&check)),
        Auth::Bearer(&Signin),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(failure.error, Error::Provider(_)));
    assert_eq!(failure.dispatch, crate::WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn bearer_refreshed_after_a_401_is_compared_without_the_401_status() {
    let server = Server::start(vec![
        response("401 Unauthorized", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let source = Provider::default();
    let check = refuse_if("toy-next");
    let failure = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &write(AttemptCheck(&check)),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(failure.error, Error::Provider(_)));
    assert_eq!(failure.status, None);
    assert_eq!(failure.dispatch, crate::WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 1);
}
