use super::*;
use crate::wire::product::options::PathIds;
use crate::{ErrorClass, Failure, Secret, WriteDispatch};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Default, serde::Deserialize)]
struct Json(Value);

impl std::fmt::Debug for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Json([redacted])")
    }
}

impl Output for Json {
    const SCHEMA: Shape<'static> = Shape::Any;
}

enum EvidenceSource {
    Token(&'static str),
    FailedSignin,
}

impl sender::BearerSource for EvidenceSource {
    fn token<'a>(
        &'a self,
        _: &'a crate::RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<sender::BearerToken, crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            let Self::Token(token) = self else {
                return Err(crate::Error::new(ErrorClass::Auth));
            };
            let signin = sender::SentCredentials::for_test(vec![Secret::new("toy-signin")]);
            Ok(sender::BearerToken {
                token: crate::Token::new(Secret::new(*token), None),
                obtained_at: None,
                signin_credentials: Some(Arc::new(signin)),
            })
        })
    }

    fn rejected<'a>(
        &'a self,
        _: &'a crate::RequestContext,
        _: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[allow(clippy::result_large_err)]
async fn call_evidence(
    server: &Server,
    op: &Operation,
    options: &Options<'_>,
    path: &str,
    token: &'static str,
    evidence: &mut Evidence,
) -> Result<(StatusCode, Json), Failure<Json>> {
    call_evidence_with(
        server,
        op,
        options,
        path,
        &[],
        EvidenceSource::Token(token),
        evidence,
    )
    .await
}

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
async fn call_evidence_with(
    server: &Server,
    op: &Operation,
    options: &Options<'_>,
    path: &str,
    query: &[(&str, &str)],
    source: EvidenceSource,
    evidence: &mut Evidence,
) -> Result<(StatusCode, Json), Failure<Json>> {
    let clients = sender::Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
    };
    let (ctx, _) = crate::RequestContext::new();
    let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let base = crate::endpoint::Base::parse(&server.origin).unwrap();
    let configured = sender::SentCredentials::for_test(vec![Secret::new("toy-configured")]);
    execute_with_evidence(
        sender::Runtime {
            configured_credentials: Some(&configured),
            client: &clients,
            ctx: &ctx,
            clock: &clock,
            deadline: clock.monotonic_now() + Duration::from_secs(5),
            sleeper: &Immediate,
            jitter: &mut || 0,
        },
        &base,
        op,
        options,
        &[path],
        query,
        &source,
        bytes::Bytes::new(),
        evidence,
    )
    .await
}

#[tokio::test]
async fn evidence_accumulates_across_success_and_send_failure() {
    let server = Server::start(vec![
        response("200 OK", br#"{"id":"one"}"#),
        response("200 OK", &[b'x'; 101]),
    ]);
    let mut evidence = Evidence::new();
    let op = declaration();
    let options = Options::default();
    assert!(
        call_evidence(&server, &op, &options, "one", "toy-first", &mut evidence)
            .await
            .is_ok()
    );
    assert!(evidence.reflected_in(&json!("toy-first")));
    assert!(evidence.reflected_in(&json!("toy-signin")));
    assert!(evidence.reflected_in(&json!("toy-configured")));
    assert!(
        call_evidence(&server, &op, &options, "one", "toy-second", &mut evidence)
            .await
            .is_err()
    );
    assert!(evidence.reflected_in(&json!("toy-first")));
    assert!(evidence.reflected_in(&json!("toy-second")));
    assert!(!evidence.reflected_in(&json!("public")));
}

#[tokio::test]
async fn later_call_rejects_a_reflected_service_secret() {
    let body = br#"{"id":"issued-pw"}"#;
    let server = Server::start(vec![response("200 OK", body), response("200 OK", body)]);
    let mut op = declaration();
    op.reject_reflected_credentials = true;
    let options = Options::default();
    let mut evidence = Evidence::new();
    let (_, output) = call_evidence(&server, &op, &options, "one", "toy-first", &mut evidence)
        .await
        .unwrap();
    evidence.add_secret(&Secret::new(output.0["id"].as_str().unwrap()));
    let failure = call_evidence(&server, &op, &options, "one", "toy-second", &mut evidence)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::Api);
    assert!(failure.partial().is_none());
}

#[tokio::test]
async fn settlement_failure_preserves_sanitized_decoded_output_and_dispatch() {
    let server = Server::start(vec![
        response(
            "200 OK",
            br#"{"id":"one","secret":"issued-pw","count":1e6,"echo":"toy-second"}"#,
        ),
        response("403 Forbidden", br#"{"secret":"never-output"}"#),
    ]);
    let write = Operation {
        method: Method::POST,
        class: redirect::Classification::Write,
        ..declaration()
    };
    let options = Options::default();
    let mut evidence = Evidence::new();
    let (_, output) = call_evidence(&server, &write, &options, "one", "toy-first", &mut evidence)
        .await
        .unwrap();
    evidence.add_secret(&Secret::new(output.0["secret"].as_str().unwrap()));
    evidence.add_secret(&Secret::new("1000000"));
    let failure = call_evidence(
        &server,
        &declaration(),
        &options,
        "one",
        "toy-second",
        &mut evidence,
    )
    .await
    .unwrap_err();
    assert!(failure.partial().is_none());
    let failure = evidence.with_partial(failure, output.0, WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.error().class(), ErrorClass::PermissionDenied);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let partial = failure.partial().unwrap();
    assert!(!evidence.reflected_in(partial));
    assert!(
        partial
            == &json!({
                "id": "one",
                "secret": "<redacted>",
                "count": "<redacted>",
                "echo": "<redacted>"
            })
    );
}

#[tokio::test]
async fn fixed_header_is_sent_and_invalid_declarations_are_refused() {
    let server = Server::start(vec![response("200 OK", br#"{"id":"one"}"#)]);
    let mut evidence = Evidence::new();
    let op = declaration();
    let options = Options {
        headers: &[
            ("user-type", "IAM_USER"),
            ("region", "r-1"),
            ("region_id", "r-1"),
        ],
        ..Options::default()
    };
    assert!(
        call_evidence(&server, &op, &options, "one", "toy-token", &mut evidence)
            .await
            .is_ok()
    );
    let head = String::from_utf8_lossy(&server.heads()[0]).to_ascii_lowercase();
    assert!(head.contains("user-type: iam_user"));
    assert!(head.contains("region: r-1"));
    assert!(head.contains("region_id: r-1"));
    const INVALID: &[&[(&str, &str)]] = &[
        &[("authorization", "v")],
        &[("region", "a b")],
        &[("region_id", "")],
        &[(
            "region",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )],
        &[("user-type", "a\nb")],
        &[("user-type", "a"), ("user-type", "b")],
    ];
    for headers in INVALID {
        let options = Options {
            headers,
            ..Options::default()
        };
        let failure = call_evidence(&server, &op, &options, "one", "toy-token", &mut evidence)
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    }
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn fixed_header_cannot_echo_any_known_secret() {
    let server = Server::start(vec![response("200 OK", br#"{"id":"one"}"#)]);
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("toy-issued"));
    let op = declaration();
    const REFLECTED: &[&[(&str, &str)]] = &[
        &[("user-type", "toy-token")],
        &[("user-type", "toy-signin")],
        &[("user-type", "toy-configured")],
        &[("user-type", "copy-toy-issued-suffix")],
    ];
    for headers in REFLECTED {
        let options = Options {
            headers,
            ..Options::default()
        };
        assert!(
            call_evidence(&server, &op, &options, "one", "toy-token", &mut evidence)
                .await
                .is_err()
        );
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn underscore_path_rule_is_opt_in_and_accepts_only_database_id_characters() {
    let server = Server::start(vec![response("200 OK", br#"{"id":"one"}"#)]);
    let mut evidence = Evidence::new();
    let op = declaration();
    let mut options = Options::default();
    let failure = call_evidence(&server, &op, &options, "db_1-x", "toy-token", &mut evidence)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    options.path_ids = PathIds::WithUnderscore;
    for bad in ["", "a.b", "a/b", "a b", "a%2fb", "a?b", "caf\u{e9}"] {
        let failure = call_evidence(&server, &op, &options, bad, "toy-token", &mut evidence)
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    }
    assert!(
        call_evidence(&server, &op, &options, "db_1-x", "toy-token", &mut evidence)
            .await
            .is_ok()
    );
    assert_eq!(server.requests(), 1);
    assert!(server.heads()[0].starts_with(b"GET /items/db_1-x "));
}

#[tokio::test]
async fn failed_signin_withholds_partial_even_after_later_success() {
    let server = Server::start(vec![
        response("200 OK", br#"{"id":"one","untracked":"toy-otp"}"#),
        response("200 OK", br#"{"id":"two"}"#),
    ]);
    let write = Operation {
        method: Method::POST,
        class: redirect::Classification::Write,
        ..declaration()
    };
    let read = declaration();
    let options = Options::default();
    let mut evidence = Evidence::new();
    let (_, output) = call_evidence(&server, &write, &options, "one", "toy-first", &mut evidence)
        .await
        .unwrap();
    let failure = call_evidence_with(
        &server,
        &read,
        &options,
        "two",
        &[],
        EvidenceSource::FailedSignin,
        &mut evidence,
    )
    .await
    .unwrap_err();
    assert!(!evidence.can_retain_partial());
    assert!(
        call_evidence(&server, &read, &options, "two", "toy-second", &mut evidence)
            .await
            .is_ok()
    );
    assert!(!evidence.can_retain_partial());
    let failure = evidence.with_partial(failure, output.0, WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.error().class(), ErrorClass::Auth);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn route_inputs_cannot_reflect_prior_bearers_or_service_secrets() {
    let server = Server::start(vec![
        response("200 OK", br#"{"id":"one"}"#),
        response("200 OK", br#"{"id":"two"}"#),
        response("200 OK", br#"{"id":"three"}"#),
        response("200 OK", br#"{"id":"four"}"#),
    ]);
    let op = declaration();
    let options = Options::default();
    let mut evidence = Evidence::new();
    assert!(
        call_evidence(&server, &op, &options, "one", "toy-first", &mut evidence)
            .await
            .is_ok()
    );
    evidence.add_secret(&Secret::new("toy issued"));
    let prior_bearer = call_evidence(
        &server,
        &op,
        &options,
        "toy-first",
        "toy-second",
        &mut evidence,
    )
    .await;
    let query_secret = call_evidence_with(
        &server,
        &op,
        &options,
        "two",
        &[("name", "copy toy issued")],
        EvidenceSource::Token("toy-second"),
        &mut evidence,
    )
    .await;
    assert!(prior_bearer.is_err());
    assert!(query_secret.is_err());
    assert_eq!(server.requests(), 1);
    evidence.add_secret(&Secret::new("items"));
    let public = call_evidence_with(
        &server,
        &op,
        &options,
        "two",
        &[("items", "public")],
        EvidenceSource::Token("toy-second"),
        &mut evidence,
    )
    .await;
    assert!(public.is_ok());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn key_routes_send_only_the_cdn_key_and_record_reflection() {
    let server = Server::start(vec![response("401 Unauthorized", b"{}")]);
    let key = Secret::new("toy-cdn-key");
    let mut evidence = Evidence::new();
    let reply = raw_route(&server, RouteAuth::CdnKey(Some(&key)), &mut evidence)
        .await
        .unwrap();
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(evidence.reflected_in(&json!("toy-cdn-key")));
    let heads = server.heads();
    let req = String::from_utf8_lossy(&heads[0]);
    assert!(
        req.to_ascii_lowercase()
            .contains("authorization: bearer toy-cdn-key")
    );
    assert!(!req.contains("toy-configured"));
}

#[tokio::test]
async fn public_routes_send_no_credentials() {
    let server = Server::start(vec![response("200 OK", b"<html></html>")]);
    let reply = raw_route(&server, RouteAuth::PublicHtml, &mut Evidence::new())
        .await
        .unwrap();
    assert_eq!(reply.body.as_slice(), b"<html></html>");
    let heads = server.heads();
    let req = String::from_utf8_lossy(&heads[0]).to_ascii_lowercase();
    assert!(req.contains("accept: text/html"));
    assert!(!req.contains("authorization:"));
    assert!(!req.contains("toy-configured"));
}

#[tokio::test]
async fn missing_cdn_key_refuses_before_send() {
    let server = Server::start(vec![]);
    let result = raw_route(&server, RouteAuth::CdnKey(None), &mut Evidence::new()).await;
    assert_eq!(
        result.err().unwrap().error().class(),
        ErrorClass::NoCredentials
    );
}

#[allow(clippy::result_large_err)]
async fn raw_route(
    server: &Server,
    auth: RouteAuth<'_>,
    evidence: &mut Evidence,
) -> Result<sender::Response, Failure<()>> {
    let clients = sender::Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
    };
    let (ctx, _) = crate::RequestContext::new();
    let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let base = crate::endpoint::Base::parse(&server.origin).unwrap();
    let configured = sender::SentCredentials::for_test(vec![Secret::new("toy-configured")]);
    let mut op = declaration();
    op.template = "route";
    execute_raw(
        sender::Runtime {
            configured_credentials: Some(&configured),
            client: &clients,
            ctx: &ctx,
            clock: &clock,
            deadline: clock.monotonic_now() + Duration::from_secs(5),
            sleeper: &Immediate,
            jitter: &mut || 0,
        },
        &base,
        &op,
        &Options::DEFAULT,
        &[],
        &[],
        auth,
        bytes::Bytes::new(),
        evidence,
    )
    .await
}
