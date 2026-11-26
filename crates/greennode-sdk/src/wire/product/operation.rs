use super::{redirect, retry, sender};
use crate::wire::send;
use crate::wire::strict_json::Field;
use http::{Method, StatusCode};
use serde::de::DeserializeOwned;

#[allow(dead_code)]
pub(crate) struct Operation {
    pub(crate) id: &'static str,
    pub(crate) template: &'static str,
    pub(crate) method: Method,
    pub(crate) class: redirect::Classification,
    pub(crate) retry: redirect::Retry,
    pub(crate) success: &'static [StatusCode],
    pub(crate) cap: usize,
    pub(crate) allow_recovery: bool,
    pub(crate) allow_empty: bool,
    pub(crate) no_output: bool,
    pub(crate) keep_status: bool,
    pub(crate) overflow_overrides_status: bool,
    pub(crate) sensitive: bool,
    pub(crate) no_redirect: bool,
    pub(crate) retry_config: retry::Config,
}

#[allow(dead_code, clippy::result_large_err)]
pub(crate) async fn execute<T: Output>(
    runtime: sender::Runtime<'_>,
    base: &crate::endpoint::Base,
    operation: &Operation,
    path: &[&str],
    query: &[(&str, &str)],
    source: &dyn sender::BearerSource,
    body: bytes::Bytes,
) -> Result<(StatusCode, T), crate::Failure<T>> {
    let dispatch = if matches!(operation.class, redirect::Classification::Write) {
        crate::WriteDispatch::NotSent
    } else {
        crate::WriteDispatch::NotAWrite
    };
    let invalid = || {
        crate::Failure::new(
            crate::Error::new(crate::ErrorClass::InvalidInput),
            None,
            dispatch,
        )
    };
    if path
        .iter()
        .any(|id| id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    {
        return Err(invalid());
    }
    let mut parts = operation.template.split("{}");
    let mut rendered = parts.next().ok_or_else(invalid)?.to_owned();
    for id in path {
        let suffix = parts.next().ok_or_else(invalid)?;
        rendered.push_str(id);
        rendered.push_str(suffix);
    }
    if parts.next().is_some() {
        return Err(invalid());
    }
    let route = base.route(&rendered, &[]).map_err(|_| invalid())?;
    let mut url = url::Url::parse(&route).map_err(|_| invalid())?;
    let mut pairs: Vec<_> = query.to_vec();
    pairs.sort_by(|left, right| left.0.cmp(right.0));
    if !pairs.is_empty() {
        let mut encoded = String::new();
        for (name, value) in pairs {
            if !encoded.is_empty() {
                encoded.push('&');
            }
            encode_query(name, &mut encoded);
            encoded.push('=');
            encode_query(value, &mut encoded);
        }
        url.set_query(Some(&encoded));
    }
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::ACCEPT,
        http::HeaderValue::from_static("application/json"),
    );
    if !body.is_empty() {
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
    }
    let request = sender::Request {
        url,
        headers,
        credentials: Vec::new(),
        body,
    };
    let policy = sender::Operation {
        method: operation.method.clone(),
        class: operation.class,
        retry: operation.retry,
        no_redirect: operation.no_redirect,
        allow_recovery: operation.allow_recovery,
        cap: operation.cap,
        encoding: send::Encoding::Gzip,
        retry_config: retry::Config {
            budget: operation.retry_config.budget,
            interval: operation.retry_config.interval,
        },
    };
    let response = match sender::run(runtime, &policy, sender::Auth::Bearer(source), &request).await
    {
        Ok(response) => response,
        Err(failure) => {
            let status = failure.status;
            let overflow = matches!(&failure.error, sender::Error::Send(error) if matches!(error.kind(), send::Error::BodyTooLarge));
            if overflow
                && !operation.overflow_overrides_status
                && status.is_some_and(|s| s.is_client_error() || s.is_server_error())
            {
                let status = status.ok_or_else(invalid)?;
                let error = super::response::classify(
                    operation.id,
                    status,
                    bytes::Bytes::new(),
                    &[],
                    (true, operation.sensitive, operation.retry),
                    &failure.credentials,
                )
                .err()
                .map(|error| *error)
                .unwrap_or_else(|| api_error(operation.id, Some(status), crate::ErrorClass::Api));
                return Err(crate::Failure::new(error, None, failure.dispatch));
            }
            if overflow {
                let api = crate::ApiError::without_fallback(
                    operation.id,
                    None,
                    crate::error::SafeText::new("", &[]),
                    crate::error::SafeText::new("", &[]),
                    false,
                );
                let error = crate::Error::new(crate::ErrorClass::Api)
                    .with_cause(crate::Error::from_api(api))
                    .with_body_too_large();
                let error = match status {
                    Some(status) => error.with_outer_detail(status),
                    None => error,
                };
                return Err(crate::Failure::new(error, None, failure.dispatch));
            }
            let class = match &failure.error {
                sender::Error::Context(crate::ContextEnd::Cancelled) => {
                    crate::ErrorClass::Cancelled
                }
                sender::Error::Context(crate::ContextEnd::DeadlineExceeded) => {
                    crate::ErrorClass::DeadlineExceeded
                }
                _ => crate::ErrorClass::Api,
            };
            let retryable = send_retry_hint(&failure.error, operation.retry);
            let error = match failure.error {
                sender::Error::Provider(error) => *error,
                _ if retryable => retryable_error(operation.id),
                _ => api_error(
                    operation.id,
                    if operation.keep_status { status } else { None },
                    class,
                ),
            };
            let error = match status {
                Some(status) => error.with_outer_detail(status),
                None => error,
            };
            return Err(crate::Failure::new(error, None, failure.dispatch));
        }
    };
    let status = response.status;
    let dispatch = if matches!(operation.class, redirect::Classification::Write) {
        crate::WriteDispatch::MayHaveBeenSent
    } else {
        crate::WriteDispatch::NotAWrite
    };
    let body = super::response::classify(
        operation.id,
        status,
        bytes::Bytes::copy_from_slice(&response.body),
        operation.success,
        (true, operation.sensitive, operation.retry),
        &response.credentials,
    )
    .map_err(|error| crate::Failure::new(*error, None, dispatch))?;
    if operation.no_output {
        return Ok((status, T::default()));
    }
    let output = decode(&body, operation.allow_empty, operation.cap).map_err(|_| {
        crate::Failure::new(
            api_error(operation.id, None, crate::ErrorClass::Api).with_outer_detail(status),
            None,
            dispatch,
        )
    })?;
    Ok((status, output))
}

fn encode_query(input: &str, output: &mut String) {
    fn digit(value: u8) -> char {
        char::from(if value < 10 {
            b'0' + value
        } else {
            b'A' + value - 10
        })
    }
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else if byte == b' ' {
            output.push('+');
        } else {
            output.push('%');
            output.push(digit(byte >> 4));
            output.push(digit(byte & 15));
        }
    }
}

fn send_retry_hint(error: &sender::Error, policy: redirect::Retry) -> bool {
    matches!(
        error,
        sender::Error::Send(send::Error::Transport(retry::SendFailure::FailedDial))
    ) || (matches!(policy, redirect::Retry::Repeatable)
        && matches!(
            error,
            sender::Error::Send(send::Error::Transport(retry::SendFailure::PossiblySent))
        ))
}

fn retryable_error(operation: &'static str) -> crate::Error {
    crate::Error::from_api(crate::ApiError::without_fallback(
        operation,
        None,
        crate::error::SafeText::new("", &[]),
        crate::error::SafeText::new("", &[]),
        true,
    ))
}

fn api_error(
    operation: &'static str,
    status: Option<StatusCode>,
    class: crate::ErrorClass,
) -> crate::Error {
    let api = crate::ApiError::without_fallback(
        operation,
        status.map(|s| s.as_u16()),
        crate::error::SafeText::new("", &[]),
        crate::error::SafeText::new("", &[]),
        false,
    );
    if class == crate::ErrorClass::Api {
        crate::Error::from_api(api)
    } else {
        crate::Error::new(class).with_cause(crate::Error::from_api(api))
    }
}

#[allow(dead_code)]
pub(crate) trait Output: DeserializeOwned + Default {
    const FIELDS: &'static [Field<'static>];
    const SHAPE: crate::wire::strict_json::Shape<'static> =
        crate::wire::strict_json::Shape::Object(&[]);
}

#[allow(dead_code)]
fn decode<T: Output>(body: &[u8], allow_empty: bool, cap: usize) -> Result<T, ()> {
    if allow_empty && body.is_empty() {
        return Ok(T::default());
    }
    crate::wire::strict_json::validate(body, T::FIELDS, cap).map_err(|_| ())?;
    crate::wire::strict_json::validate_shape(body, &T::SHAPE).map_err(|_| ())?;
    serde_json::from_slice(body).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        connection::{HeadLimit, OneSendClient, PooledClient, Trust},
        test_server::{Response, Server},
    };
    use crate::{Clock, TestClock};
    use std::{future::Future, pin::Pin, time::Duration};

    struct Source(bool);
    impl sender::BearerSource for Source {
        fn token<'a>(
            &'a self,
            _: &'a crate::RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<sender::BearerToken, crate::Error>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(sender::BearerToken {
                    token: crate::Token::new(crate::Secret::new("toy-token"), None),
                    obtained_at: None,
                })
            })
        }
        fn rejected<'a>(
            &'a self,
            _: &'a crate::RequestContext,
            _: &'a crate::Secret,
            _: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async move {
                if self.0 {
                    Err(crate::Error::provider_failed())
                } else {
                    Ok(())
                }
            })
        }
    }
    struct Immediate;
    impl sender::Sleeper for Immediate {
        fn sleep(&self, _: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }
    }
    fn declaration() -> Operation {
        Operation {
            id: "test.get",
            template: "items/{}",
            method: Method::GET,
            class: redirect::Classification::Read,
            retry: redirect::Retry::Once,
            success: &[StatusCode::OK],
            cap: 100,
            allow_recovery: false,
            allow_empty: false,
            no_output: false,
            keep_status: false,
            overflow_overrides_status: false,
            sensitive: false,
            no_redirect: false,
            retry_config: retry::Config::default(),
        }
    }
    fn response(status: &str, body: &[u8]) -> Response {
        Response::Http {
            status: status.into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body: body.to_vec(),
        }
    }
    #[allow(clippy::result_large_err)]
    async fn call(
        server: &Server,
        op: &Operation,
        path: &[&str],
    ) -> Result<(StatusCode, Example), crate::Failure<Example>> {
        call_with_query(server, op, path, &[]).await
    }
    #[allow(clippy::result_large_err)]
    async fn call_with_query(
        server: &Server,
        op: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<(StatusCode, Example), crate::Failure<Example>> {
        call_with_body(server, op, path, query, bytes::Bytes::new()).await
    }
    #[allow(clippy::result_large_err)]
    async fn call_with_body(
        server: &Server,
        op: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
        body: bytes::Bytes,
    ) -> Result<(StatusCode, Example), crate::Failure<Example>> {
        call_with_source(server, op, path, query, body, &Source(false)).await
    }
    #[allow(clippy::result_large_err)]
    async fn call_with_source(
        server: &Server,
        op: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
        body: bytes::Bytes,
        source: &dyn sender::BearerSource,
    ) -> Result<(StatusCode, Example), crate::Failure<Example>> {
        let clients = sender::Clients {
            pooled: PooledClient::new(HeadLimit::Api, Trust::test(server.root_der.clone()))
                .unwrap(),
            one_send: OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone()))
                .unwrap(),
        };
        let (ctx, _) = crate::RequestContext::new();
        let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let deadline = clock.monotonic_now() + Duration::from_secs(5);
        let base = crate::endpoint::Base::parse(&server.origin).unwrap();
        execute(
            sender::Runtime {
                client: &clients,
                ctx: &ctx,
                clock: &clock,
                deadline,
                sleeper: &Immediate,
                jitter: &mut || 0,
            },
            &base,
            op,
            path,
            query,
            source,
            body,
        )
        .await
    }

    #[tokio::test]
    async fn json_headers_match_body_presence() {
        let server = Server::start(vec![
            response("200 OK", br#"{"id":"one"}"#),
            response("200 OK", br#"{"id":"one"}"#),
        ]);
        assert!(call(&server, &declaration(), &["one"]).await.is_ok());
        assert!(
            call_with_body(
                &server,
                &declaration(),
                &["one"],
                &[],
                bytes::Bytes::from_static(b"{}")
            )
            .await
            .is_ok()
        );
        let heads = server.heads();
        let first = String::from_utf8_lossy(&heads[0]).to_ascii_lowercase();
        let second = String::from_utf8_lossy(&heads[1]).to_ascii_lowercase();
        assert!(first.contains("accept: application/json"));
        assert!(!first.contains("content-type:"));
        assert!(second.contains("accept: application/json"));
        assert!(second.contains("content-type: application/json"));
    }

    #[tokio::test]
    async fn get_decodes_typed_response() {
        let server = Server::start(vec![response("200 OK", br#"{"id":"one"}"#)]);
        let (status, output) = call(&server, &declaration(), &["one"]).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(output, Example { id: "one".into() });
        assert_eq!(server.requests(), 1);
    }

    #[derive(Debug, Default, serde::Deserialize, PartialEq)]
    struct Example {
        id: String,
    }

    impl Output for Example {
        const FIELDS: &'static [Field<'static>] = &[Field {
            name: "id",
            children: &[],
        }];
    }

    #[tokio::test]
    async fn paged_get_encodes_query() {
        let server = Server::start(vec![response("200 OK", br#"{"id":"one"}"#)]);
        assert!(
            call_with_query(
                &server,
                &declaration(),
                &["one"],
                &[
                    ("size", "20"),
                    ("page", "2"),
                    ("ignored", ""),
                    ("filter", "a b~*")
                ]
            )
            .await
            .is_ok()
        );
        assert!(
            server.heads()[0]
                .starts_with(b"GET /items/one?filter=a+b~%2A&ignored=&page=2&size=20 ")
        );
    }

    #[tokio::test]
    async fn path_identifiers_fill_template() {
        let server = Server::start(vec![response("200 OK", br#"{"id":"one"}"#)]);
        let mut op = declaration();
        op.template = "projects/{}/items/{}";
        assert!(call(&server, &op, &["project-1", "item-1"]).await.is_ok());
        let heads = server.heads();
        assert!(heads[0].starts_with(b"GET /projects/project-1/items/item-1 "));
    }

    #[tokio::test]
    async fn declared_empty_body_decodes() {
        let server = Server::start(vec![response("200 OK", b"")]);
        let mut op = declaration();
        op.allow_empty = true;
        assert_eq!(
            call(&server, &op, &["one"]).await.unwrap().1,
            Example::default()
        );
    }

    #[tokio::test]
    async fn no_output_discards_bounded_success_body() {
        let server = Server::start(vec![response("200 OK", b"not json")]);
        let mut op = declaration();
        op.no_output = true;
        assert_eq!(
            call(&server, &op, &["one"]).await.unwrap().1,
            Example::default()
        );
    }

    #[tokio::test]
    async fn malformed_success_keeps_observed_status() {
        let server = Server::start(vec![response("200 OK", b"{")]);
        let error = call(&server, &declaration(), &["one"]).await.unwrap_err();
        assert_eq!(error.error().detail::<StatusCode>(), Some(&StatusCode::OK));
        assert_eq!(error.error().class(), crate::ErrorClass::Api);
    }

    #[tokio::test]
    async fn failed_invalidation_keeps_unauthorized_status() {
        let server = Server::start(vec![response("401 Unauthorized", b"{}")]);
        let error = call_with_source(
            &server,
            &declaration(),
            &["one"],
            &[],
            bytes::Bytes::new(),
            &Source(true),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.error().detail::<StatusCode>(),
            Some(&StatusCode::UNAUTHORIZED)
        );
        assert_eq!(error.error().class(), crate::ErrorClass::Auth);
    }

    #[tokio::test]
    async fn oversized_success_retains_identity_and_status() {
        let server = Server::start(vec![response("200 OK", b"0123456789")]);
        let mut op = declaration();
        op.cap = 4;
        let error = call(&server, &op, &["one"]).await.unwrap_err();
        assert!(error.error().is_body_too_large());
        assert_eq!(error.error().class(), crate::ErrorClass::Api);
        assert_eq!(error.error().detail::<StatusCode>(), Some(&StatusCode::OK));
    }

    #[tokio::test]
    async fn overflow_override_retains_identity_and_status() {
        let server = Server::start(vec![response("404 Not Found", b"0123456789")]);
        let mut op = declaration();
        op.cap = 4;
        op.overflow_overrides_status = true;
        let error = call(&server, &op, &["one"]).await.unwrap_err();
        assert!(error.error().is_body_too_large());
        assert_eq!(error.error().class(), crate::ErrorClass::Api);
        assert_eq!(
            error.error().detail::<StatusCode>(),
            Some(&StatusCode::NOT_FOUND)
        );
        assert!(!error.error().has_class(crate::ErrorClass::NotFound));
    }

    #[tokio::test]
    async fn oversized_error_respects_status_precedence() {
        let server = Server::start(vec![response("404 Not Found", b"0123456789")]);
        let mut op = declaration();
        op.cap = 4;
        let error = call(&server, &op, &["one"]).await.unwrap_err();
        assert_eq!(error.error().class(), crate::ErrorClass::NotFound);
        assert_eq!(error.error().api_error().unwrap().status(), Some(404));
    }

    #[tokio::test]
    async fn safe_error_omits_reflected_bearer() {
        let server = Server::start(vec![response(
            "403 Forbidden",
            br#"{"code":"Denied","message":"toy-token"}"#,
        )]);
        let error = call(&server, &declaration(), &["one"]).await.unwrap_err();
        assert_eq!(error.error().class(), crate::ErrorClass::PermissionDenied);
        let api = error.error().api_error().unwrap();
        assert_eq!(api.code(), "Denied");
        assert!(api.message() == "Forbidden");
    }

    #[tokio::test]
    async fn invalid_path_is_rejected_before_send() {
        let server = Server::start(vec![response("200 OK", br#"{"id":"one"}"#)]);
        let error = call(&server, &declaration(), &["bad_id"])
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), crate::ErrorClass::InvalidInput);
        assert_eq!(server.requests(), 0);
        assert!(call(&server, &declaration(), &["one"]).await.is_ok());
    }

    #[tokio::test]
    async fn once_does_not_retry_503() {
        let server = Server::start(vec![
            response("503 Service Unavailable", b"{}"),
            response("200 OK", br#"{"id":"one"}"#),
        ]);
        assert!(call(&server, &declaration(), &["one"]).await.is_err());
        assert_eq!(server.requests(), 1);
    }

    #[tokio::test]
    async fn write_redirect_is_refused() {
        let server = Server::start(vec![
            Response::Http {
                status: "302 Found".into(),
                headers: vec![
                    ("Location".into(), "/other".into()),
                    ("Content-Length".into(), "0".into()),
                ],
                body: Vec::new(),
            },
            response("200 OK", br#"{"id":"one"}"#),
        ]);
        let mut op = declaration();
        op.method = Method::POST;
        op.class = redirect::Classification::Write;
        op.retry = redirect::Retry::Repeatable;
        let error = call(&server, &op, &["one"]).await.unwrap_err();
        assert_eq!(
            error.write_dispatch(),
            crate::WriteDispatch::MayHaveBeenSent
        );
        assert_eq!(server.requests(), 1);
    }

    #[test]
    fn failed_dial_keeps_retry_hint() {
        let failed = sender::Error::Send(send::Error::Transport(retry::SendFailure::FailedDial));
        let body = sender::Error::Send(send::Error::Body);
        assert!(send_retry_hint(&failed, redirect::Retry::Once));
        assert!(retryable_error("test").api_error().unwrap().retryable());
        assert!(!send_retry_hint(&body, redirect::Retry::Once));
    }

    #[test]
    fn struct_output_rejects_sequence_form() {
        assert!(decode::<Example>(br#"["one"]"#, false, 64).is_err());
        assert!(decode::<Example>(br#"{"id":"one"}"#, false, 64).is_ok());
    }

    #[test]
    fn rejects_duplicate_keys() {
        assert!(decode::<Example>(br#"{"id":"one","Id":"two"}"#, false, 64).is_err());
        assert!(decode::<Example>(br#"{"id":"one"}"#, false, 64).is_ok());
    }

    #[test]
    fn decodes_typed_response() {
        assert_eq!(
            decode::<Example>(br#"{"id":"one"}"#, false, 64),
            Ok(Example { id: "one".into() })
        );
    }
}
