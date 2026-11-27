use super::refusals::nat_detail_config;
use super::*;

const ZONE_BODY: &str = r#"{"success":true,"data":[{"uuid":"zone-1","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#;

#[tokio::test]
async fn endpoint_reads_look_up_the_zone_once_per_client_and_its_clones() {
    let server = Server::start(vec![
        response("200 OK", ZONE_BODY),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/get_endpoint.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/get_endpoint.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_endpoints.json"),
        ),
    ]);
    let config = nat_detail_config(&server, true);
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    client
        .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
        .await
        .unwrap();
    client
        .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
        .await
        .unwrap();
    client
        .clone()
        .list_endpoints(&ctx, ListEndpointsInput::default())
        .await
        .unwrap();
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    assert!(heads[0].starts_with(b"GET /vnetwork/v1/regions "));
    assert!(
        heads[1..]
            .iter()
            .all(|head| !head.starts_with(b"GET /vnetwork/v1/regions "))
    );
}

#[tokio::test]
async fn nat_reads_look_up_the_zone_each_time() {
    let nat = include_str!("../../../tests/fixtures/network/get_nat_instance.json");
    let server = Server::start(vec![
        response("200 OK", ZONE_BODY),
        response("200 OK", nat),
        response("200 OK", ZONE_BODY),
        response("200 OK", nat),
    ]);
    let config = nat_detail_config(&server, true);
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    for _ in 0..2 {
        client
            .get_nat_instance(&ctx, GetNatInstanceInput::new("nat-1"))
            .await
            .unwrap();
    }
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    for index in [0, 2] {
        assert!(heads[index].starts_with(b"GET /vnetwork/v1/regions "));
    }
}

struct RotatingToken(std::sync::atomic::AtomicUsize);

impl crate::CredentialsProvider for RotatingToken {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let token = if n < 2 { "first-token" } else { "zone-1" };
            Ok(crate::Token::new(Secret::new(token), None))
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn cached_endpoint_zone_is_refused_when_a_later_token_reflects_it() {
    let server = Server::start(vec![
        response("200 OK", ZONE_BODY),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/get_endpoint.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/get_endpoint.json"),
        ),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_credentials_provider(std::sync::Arc::new(RotatingToken(
            std::sync::atomic::AtomicUsize::new(0),
        )));
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    client
        .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
        .await
        .unwrap();
    assert_eq!(server.heads().len(), 2);
    let failure = client
        .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn endpoint_tag_reflections_are_invalid_responses() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":61}]}"#,
        ),
        response(
            "200 OK",
            r#"{"data":[{"uuid":"abceHl6def","resourceUuid":"endpoint-1"}]}"#,
        ),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_static_token(Secret::new("xyz"));
    let options = with_product_loopback(Product::Vserver, options, &server);
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .list_endpoint_tags(&ctx, ListEndpointTagsInput::new("endpoint-1"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn explicit_list_zone_keeps_the_senders_error() {
    let server = Server::start(vec![response("200 OK", r#"{"data":[]}"#)]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_credentials_provider(std::sync::Arc::new(RotatingToken(
            std::sync::atomic::AtomicUsize::new(2),
        )));
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let input = ListEndpointsInput {
        zone_id: "zone-1".into(),
        ..Default::default()
    };
    let failure = Client::new(&config)
        .list_endpoints(&ctx, input)
        .await
        .unwrap_err();
    assert_senders_refusal(failure.error());
    assert!(server.heads().is_empty());
}

struct FixedToken(&'static str);

impl crate::CredentialsProvider for FixedToken {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>>
    {
        Box::pin(async move { Ok(crate::Token::new(Secret::new(self.0), None)) })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn region_name_fallback_zone_keeps_the_senders_error() {
    let server = Server::start(vec![response("200 OK", r#"{"success":true,"data":[]}"#)]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_credentials_provider(std::sync::Arc::new(FixedToken("hcm-3")));
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
        .await
        .unwrap_err();
    assert_senders_refusal(failure.error());
    assert_eq!(server.heads().len(), 1);
}

fn assert_senders_refusal(error: &Error) {
    let api = error.api_error().unwrap();
    assert_eq!(
        (error.class(), api.code(), api.message(), api.status()),
        (ErrorClass::Api, "", "", None)
    );
}
