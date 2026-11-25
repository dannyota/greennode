use crate::{LoginError, Secret, Token};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::{collections::HashSet, fmt};

enum Node {
    Object(Vec<(String, Node)>),
    String(Secret),
    Integer(i64),
    Null,
    Other,
}

struct NodeVisitor;
impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Node, E> {
        Ok(Node::Other)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node, E> {
        Ok(Node::Integer(v))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node, E> {
        Ok(i64::try_from(v).map_or(Node::Other, Node::Integer))
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Node, E> {
        Ok(Node::Other)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node, E> {
        Ok(Node::String(Secret::new(v)))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Node, E> {
        Ok(Node::String(Secret::new(v)))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        while seq.next_element::<Node>()?.is_some() {}
        Ok(Node::Other)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom("duplicate key"));
            }
            entries.push((key, map.next_value::<Node>()?));
        }
        Ok(Node::Object(entries))
    }
}
impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NodeVisitor)
    }
}

fn decode(
    bytes: &[u8],
    started: time::OffsetDateTime,
    now: time::OffsetDateTime,
) -> Result<Token, LoginError> {
    let err = || LoginError::new("token exchange", None, false);
    let Node::Object(fields) = serde_json::from_slice(bytes).map_err(|_| err())? else {
        return Err(err());
    };
    let mut access = None;
    let mut expires = None;
    let mut refresh_seen = false;
    for (name, value) in fields {
        if name.eq_ignore_ascii_case("accessToken") {
            if access.is_some() {
                return Err(err());
            }
            access = Some(match value {
                Node::String(value) => value,
                Node::Null => Secret::new(""),
                _ => return Err(err()),
            });
        } else if name.eq_ignore_ascii_case("expiresIn") {
            if expires.is_some() {
                return Err(err());
            }
            expires = Some(match value {
                Node::Integer(value) => value,
                Node::Null => 0,
                _ => return Err(err()),
            });
        } else if name.eq_ignore_ascii_case("refreshToken") {
            if refresh_seen || !matches!(value, Node::String(_) | Node::Null) {
                return Err(err());
            }
            refresh_seen = true;
        }
    }
    let access = access.ok_or_else(err)?;
    let expires = expires.ok_or_else(err)?;
    if access.expose_secret().is_empty() || expires <= 30 || expires > i64::MAX / 1_000_000_000 {
        return Err(err());
    }
    let end = started
        .checked_add(time::Duration::seconds(expires))
        .ok_or_else(err)?;
    if end <= now.saturating_add(time::Duration::seconds(30)) {
        return Err(err());
    }
    Ok(Token::new(access, Some(end)))
}

pub(super) struct Exchange<'a> {
    pub(super) client: &'a crate::wire::connection::OneSendClient,
    pub(super) token_url: &'a url::Url,
    pub(super) dashboard: &'a url::Url,
    pub(super) code: &'a Secret,
    pub(super) verifier: &'a Secret,
    pub(super) clock: &'a dyn crate::Clock,
    pub(super) deadline: tokio::time::Instant,
}

pub(super) async fn exchange(input: Exchange<'_>) -> Result<Token, LoginError> {
    use crate::wire::send::{self, Encoding};
    use bytes::Bytes;
    use data_encoding::BASE64;
    use http::{Request, header};
    use http_body_util::Full;

    let failure = || LoginError::new("token exchange", None, false);
    if input.token_url.scheme() != "https"
        || !input.token_url.username().is_empty()
        || input.token_url.password().is_some()
        || input.token_url.fragment().is_some()
    {
        return Err(failure());
    }
    #[derive(serde::Serialize)]
    struct Body<'a> {
        #[serde(rename = "grantType")]
        grant_type: &'static str,
        code: &'a str,
        #[serde(rename = "redirectUri")]
        redirect_uri: &'a str,
        scope: &'static str,
        #[serde(rename = "codeVerifier")]
        verifier: &'a str,
    }
    let body = Secret::new(
        serde_json::to_string(&Body {
            grant_type: "authorization_code",
            code: input.code.expose_secret(),
            redirect_uri: input.dashboard.as_str(),
            scope: "openid",
            verifier: input.verifier.expose_secret(),
        })
        .map_err(|_| failure())?,
    );
    let authorization = format!(
        "Basic {}",
        BASE64.encode(b"c9e78411-f2a2-41ba-a9e4-3c56263c181a:")
    );
    let request = Request::post(input.token_url.as_str())
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, authorization)
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(Full::new(Bytes::copy_from_slice(
            body.expose_secret().as_bytes(),
        )))
        .map_err(|_| failure())?;
    let started = input.clock.utc_now();
    let response = send::once(
        input.client.as_client(),
        request,
        1 << 20,
        Encoding::Gzip,
        input.clock,
        input.deadline,
    )
    .await
    .map_err(|error| {
        LoginError::new(
            "token exchange",
            error.observed().map(|head| head.status.as_u16()),
            false,
        )
    })?;
    let status = Some(response.status.as_u16());
    if response.status != http::StatusCode::OK {
        return Err(LoginError::new("token exchange", status, false));
    }
    decode(&response.body, started, input.clock.utc_now())
        .map_err(|_| LoginError::new("token exchange", status, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::wire::{
        connection::{HeadLimit, OneSendClient, Trust},
        test_server::{Response, Server},
    };
    use crate::{Clock, SystemClock};
    use std::time::Duration;

    fn response(status: &str, body: &str) -> Response {
        Response::Http {
            status: status.into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body: body.as_bytes().to_vec(),
        }
    }

    async fn run(server: &Server) -> Result<Token, LoginError> {
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let token_url = url::Url::parse(&format!("{}/token", server.origin)).unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/").unwrap();
        exchange(Exchange {
            client: &client,
            token_url: &token_url,
            dashboard: &dashboard,
            code: &Secret::new("auth-code-canary"),
            verifier: &Secret::new("verifier-canary"),
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
        })
        .await
    }

    #[tokio::test]
    async fn token_exchange_succeeds_once() {
        let server = Server::start(vec![response(
            "200 OK",
            r#"{"accessToken":"token-canary","expiresIn":3600}"#,
        )]);
        let result = run(&server).await;
        assert!(result.is_ok());
        assert!(!format!("{:?}", result.unwrap()).contains("token-canary"));
        assert_eq!(server.requests(), 1);
        let head = String::from_utf8_lossy(&server.heads()[0]).to_ascii_lowercase();
        assert!(head.starts_with("post /token "));
        assert!(head.contains("content-type: application/json"));
    }

    #[tokio::test]
    async fn token_failures_do_not_leak_secrets() {
        for response in [
            Response::Close,
            response(
                "302 Found",
                r#"{"accessToken":"auth-code-canary","expiresIn":3600,"extra":"verifier-canary"}"#,
            ),
            response(
                "500 Error",
                r#"{"accessToken":"auth-code-canary","expiresIn":3600,"extra":"verifier-canary"}"#,
            ),
            response("200 OK", "auth-code-canary verifier-canary"),
            response(
                "200 OK",
                &format!(
                    r#"{{"accessToken":"abc","expiresIn":3600,"extra":"{}"}}"#,
                    "x".repeat(1 << 20)
                ),
            ),
        ] {
            let server = Server::start(vec![response]);
            let result = run(&server).await;
            assert!(result.is_err());
            let error = result.err().unwrap();
            assert_eq!(error.reason(), "token exchange");
            assert!(!format!("{error:?}").contains("auth-code-canary"));
            assert!(!format!("{error:?}").contains("verifier-canary"));
        }
    }

    #[test]
    fn token_schema_and_lifetime() {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        assert!(decode(br#"{"accessToken":"abc","expiresIn":3600}"#, now, now).is_ok());
        for bad in [
            br#"{"accessToken":"","expiresIn":3600}"#.as_slice(),
            br#"{"accessToken":"abc","expiresIn":30}"#,
            br#"{"accessToken":"abc","expiresIn":9223372037}"#,
            br#"{"accessToken":"abc","expiresIn":"3600"}"#,
            br#"{"accessToken":"abc","expiresIn":3600,"accessToken":"other"}"#,
            br#"{"accessToken":"abc","ACCESSTOKEN":"other","expiresIn":3600}"#,
            br#"{"accessToken":"abc","expiresIn":3600,"unknown":{"x":1,"x":2}}"#,
            br#"{"accessToken":"abc","expiresIn":3600} {}"#,
        ] {
            assert!(decode(bad, now, now).is_err());
        }
        assert!(
            decode(
                br#"{"accessToken":"abc","expiresIn":30}"#,
                now,
                now - time::Duration::seconds(1)
            )
            .is_err()
        );
        assert!(
            decode(
                br#"{"accessToken":"abc","expiresIn":31}"#,
                now,
                now + time::Duration::seconds(1)
            )
            .is_err()
        );
    }
}
