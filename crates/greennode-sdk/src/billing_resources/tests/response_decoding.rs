use super::*;

#[test]
fn put_account_redaction_screens_sign_in_statuses() {
    let mut redaction = Evidence::new();
    redaction.add_redaction(&Secret::new("418"));
    let error = Error::new(ErrorClass::Auth)
        .with_login(crate::LoginError::new(
            "IAM sign-in failed",
            Some(418),
            false,
        ))
        .with_cause(Error::new(ErrorClass::Auth).with_service_login(
            crate::ServiceAccountLoginError::new("service-account exchange failed", Some(418)),
        ));
    let result = screen_account_error(error, &redaction);
    assert!(result.login_error().unwrap().status().is_none());
    assert!(
        result
            .service_account_login_error()
            .unwrap()
            .status()
            .is_none()
    );
}

#[tokio::test]
async fn whitespace_only_response_is_distinct_from_zero_bytes() {
    let server = Server::start(vec![reply("200 OK", " \t\r\n"), reply("200 OK", "")]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    for message in [WITHHELD, INVALID] {
        let failure = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
        invalid_at(&failure, 200);
        assert_eq!(failure.api_error().unwrap().message(), message);
    }
}

#[tokio::test]
async fn billing_resources_preserve_raw_surrogate_escapes_and_normalize_typed_strings() {
    let unpaired = format!("{}uD800", char::from(92));
    let status = format!(r#"{{"value":"{unpaired}"}}"#);
    let metadata = format!(r#"["{unpaired}"]"#);
    let body = format!(
        r#"{{"code":200,"data":{{"data":[{{"artifactName":"typed{unpaired}","status":{status},"billingElements":[{{"metaKey":{metadata}}}]}}]}}}}"#
    );
    let server = Server::start(vec![reply("200 OK", &body), reply("200 OK", &body)]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::for_write();
    let list = list(config.session(), &ctx, &mut evidence).await.unwrap();
    let (row, _) = list.rows(&evidence).unwrap().next().unwrap();
    assert_eq!(row.artifact_name, "typed�");
    assert_eq!(row.status.as_deref().unwrap().get(), status);
    assert_eq!(
        row.billing_elements[0].meta_key.as_deref().unwrap().get(),
        metadata
    );
    let public = crate::billing::Client::new(&config)
        .list_resources(&ctx, crate::billing::ListResourcesInput::default())
        .await
        .unwrap();
    assert_eq!(public.items[0].status.as_deref().unwrap().get(), status);
    assert!(serde_json::to_string(&public).unwrap().contains(&status));
}

#[tokio::test]
async fn retained_raw_escape_spelling_is_screened_against_current_evidence() {
    let escape = format!("{}uD800", char::from(92));
    let body = format!(r#"{{"code":200,"data":{{"data":[{{"status":{{"value":"{escape}"}}}}]}}}}"#);
    let server = Server::start(vec![reply("200 OK", body)]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::new();
    let output = list(config.session(), &ctx, &mut evidence).await.unwrap();
    evidence.add_secret(&Secret::new("uD800"));
    let error = refusal(output.rows(&evidence));
    invalid_at(&error, 200);
}

#[tokio::test]
async fn nested_raw_identifiers_screen_embedded_short_encoded_credentials() {
    for key in ["ids", "uuids", "project_uuid"] {
        for value in [json!("13031329"), json!(13031329)] {
            let body = json!({"code":200,"data":{"data":[{"status":{(key):[value]}}]}}).to_string();
            let server = Server::start(vec![reply("200 OK", body)]);
            let config = static_config(&server);
            let (ctx, _) = RequestContext::new();
            let mut evidence = Evidence::new();
            let resources = list(config.session(), &ctx, &mut evidence).await.unwrap();
            evidence.add_secret(&Secret::new("012"));
            let error = refusal(resources.rows(&evidence));
            invalid_at(&error, 200);
        }
    }
}

#[tokio::test]
async fn ignored_wire_numbers_are_screened_only_as_received() {
    let accepted = r#"{"code":200,"data":{"data":[{"ignored":1e6}]}}"#;
    let reflected = r#"{"code":200,"data":{"data":[{"ignored":1e+06}]}}"#;
    let server = Server::start(vec![reply("200 OK", accepted), reply("200 OK", reflected)]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("e+06"));
    assert!(list(config.session(), &ctx, &mut evidence).await.is_ok());
    let error = refusal(list(config.session(), &ctx, &mut evidence).await);
    invalid_at(&error, 200);
}
