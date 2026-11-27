use super::*;

#[tokio::test]
async fn quote_redacts_dotted_i_client_secret_marker() {
    let server = Server::start(vec![response(
        OK,
        r#"{"optimumPrice":10,"propertiesPrice":[{"name":"CLİENT_SECRET","description":"plain"}]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SERVER))
        .await
        .unwrap();
    assert_eq!(output.properties[0].name, "[redacted]");
    assert_eq!(output.properties[0].description, "plain");
}

#[tokio::test]
async fn quote_redacts_superscript_client_secret_marker() {
    let server = Server::start(vec![response(
        OK,
        r#"{"optimumPrice":10,"propertiesPrice":[{"name":"CLIE²NT_SECRET","description":"plain"}]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SERVER))
        .await
        .unwrap();
    assert_eq!(output.properties[0].name, "[redacted]");
}

#[tokio::test]
async fn quote_redacts_client_secret_marker_in_property_text() {
    let server = Server::start(vec![response(
        OK,
        r#"{"optimumPrice":10,"propertiesPrice":[{"name":"CLIENT_SECRET value","description":"prefix client_secret suffix"},{"name":"disk","description":"normal"}]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SERVER))
        .await
        .unwrap();
    assert_eq!(output.properties[0].name, "[redacted]");
    assert_eq!(output.properties[0].description, "[redacted]");
    assert_eq!(output.properties[1].name, "disk");
    assert_eq!(output.properties[1].description, "normal");
}
