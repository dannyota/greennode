use super::*;
use greennode_sdk::WriteDispatch;
use serde_json::{Value, json};

const ACTIVE: &str = include_str!("../fixtures/cdn/webaccelerator-detail.json");
const DISABLED: &str = include_str!("../fixtures/cdn/webaccelerator-detail-disabled.json");
const DEPLOYING: &str = include_str!("../fixtures/cdn/webaccelerator-detail-deploying.json");
const TOGGLED: &str = include_str!("../fixtures/cdn/write-toggle.json");
const NOT_FOUND: &str = include_str!("../fixtures/cdn/webaccelerator-not-found.json");

fn reply(body: &str) -> Response {
    response(body, "application/json")
}

#[tokio::test]
async fn update_merges_the_fresh_object_and_returns_the_follow_up_read() {
    let server = Server::start(vec![
        reply(ACTIVE),
        reply(include_str!("../fixtures/cdn/write-update.json")),
        reply(DEPLOYING),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateWebAcceleratorInput::new("cdn-1");
    input.set_rule_actions = vec![RuleActionInput {
        name: "alwaysHttps".into(),
        value: "false".into(),
    }];
    input.c_names = Some(vec![]);
    input.no_wait = true;
    let output = client(&server, Some(CDN_KEY))
        .update_web_accelerator(&ctx, input)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(output).unwrap()["WebAccelerator"]["Status"],
        3
    );
    let original: Value = serde_json::from_str(ACTIVE).unwrap();
    let mut expected = original["data"].clone();
    let actions = expected["defaultRuleAction"].as_array_mut().unwrap();
    actions
        .iter_mut()
        .filter(|action| action["actionName"] == "alwaysHttps")
        .for_each(|action| action["value"] = json!("false"));
    expected["cName"] = json!([]);
    let body: Value = serde_json::from_slice(&server.bodies()[1]).unwrap();
    assert!(body == expected);
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /vcdn-api/v1/cdn/detail/cdn-1 "));
    assert!(heads[1].starts_with(b"PUT /vcdn-api/v1/cdn/update "));
    assert!(heads[2].starts_with(b"GET /vcdn-api/v1/cdn/detail/cdn-1 "));
}

#[tokio::test]
async fn delete_guards_preflight_and_rechecks_a_write_401_once() {
    let (ctx, _) = RequestContext::new();
    for (preflight, detail) in [
        (DEPLOYING, Some(ErrorDetail::ResourceBusy)),
        (
            r#"{"success":true,"data":{"cdnId":"cdn-2","status":1}}"#,
            None,
        ),
    ] {
        let server = Server::start(vec![reply(preflight)]);
        let failure = client(&server, Some(CDN_KEY))
            .delete_web_accelerator(&ctx, DeleteWebAcceleratorInput::new("cdn-1"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().detail::<ErrorDetail>().copied(), detail);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 1);
    }
    for (follow_up, class) in [
        (NOT_FOUND, ErrorClass::NotFound),
        (ACTIVE, ErrorClass::Auth),
    ] {
        let server = Server::start(vec![
            reply(DISABLED),
            with_status("401 Unauthorized", "", "application/json"),
            reply(follow_up),
        ]);
        let failure = client(&server, Some(CDN_KEY))
            .delete_web_accelerator(&ctx, DeleteWebAcceleratorInput::new("cdn-1"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), class);
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), 3);
        assert!(server.heads()[1].starts_with(b"DELETE /vcdn-api/v1/cdn/delete/cdn-1 "));
    }
}

#[tokio::test]
async fn toggles_infer_change_only_from_matching_confirmation() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        reply(DISABLED),
        with_status("502 Bad Gateway", "", "application/json"),
        reply(DEPLOYING),
    ]);
    let mut input = EnableWebAcceleratorInput::new("cdn-1");
    input.no_wait = true;
    let output = client(&server, Some(CDN_KEY))
        .enable_web_accelerator(&ctx, input)
        .await
        .unwrap();
    assert!(output.changed);
    assert_eq!(server.requests(), 3);
    assert!(server.heads()[1].starts_with(b"PUT /vcdn-api/v1/cdn/status/change/cdn-1 "));
    let server = Server::start(vec![reply(DISABLED)]);
    let output = client(&server, Some(CDN_KEY))
        .disable_web_accelerator(&ctx, DisableWebAcceleratorInput::new("cdn-1"))
        .await
        .unwrap();
    assert!(!output.changed);
    assert_eq!(server.requests(), 1);
    let server = Server::start(vec![reply(ACTIVE), reply(TOGGLED), reply(DISABLED)]);
    let output = client(&server, Some(CDN_KEY))
        .disable_web_accelerator(&ctx, DisableWebAcceleratorInput::new("cdn-1"))
        .await
        .unwrap();
    assert!(output.changed);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn update_partial_redacts_the_cdn_key_and_user_identifier() {
    let initial = r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"domainName":"toy-uuid","userUuid":"toy-uuid","childrenRule":{"data":"toy-uuid"}}}"#;
    let server = Server::start(vec![reply(initial), reply(TOGGLED), reply("{}")]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateWebAcceleratorInput::new("cdn-1");
    input.origin_host_header = Some("new.example.test".into());
    input.no_wait = true;
    let failure = client(&server, Some(CDN_KEY))
        .update_web_accelerator(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let partial = serde_json::to_string(failure.partial().unwrap()).unwrap();
    assert!(!partial.contains(CDN_KEY) && !partial.contains("toy-uuid"));
    assert!(partial.contains("<redacted>"));
}

#[tokio::test]
async fn cdn_classifies_only_bounded_sanitized_message_text() {
    let long = format!("{} is not allow to update or delete", "x".repeat(520));
    for message in [
        long.as_str(),
        "internal@example.test is not allow to update or delete",
    ] {
        let body = json!({"success":false,"code":400,"message":message});
        let server = Server::start(vec![reply(&body.to_string())]);
        let failure = client(&server, Some(CDN_KEY))
            .purge_paths(
                &RequestContext::new().0,
                PurgePathsInput::new("cdn.example.test", vec!["/img/a.webp".into()]),
            )
            .await
            .unwrap_err();
        assert_ne!(
            failure.error().detail::<ErrorDetail>(),
            Some(&ErrorDetail::ResourceBusy)
        );
    }
}

#[tokio::test]
async fn reflective_cdn_envelope_cannot_choose_special_error_identity() {
    for (code, message, detail) in [
        (
            400,
            "ordinary response is not allow to update or delete",
            Some(ErrorDetail::ResourceBusy),
        ),
        (400, "Not found cdn ordinary response", None),
        (
            202,
            "Last CDN flush cache time ordinary response",
            Some(ErrorDetail::PurgeCooldown),
        ),
    ] {
        let body = json!({"success":false,"code":code,"message":format!("{message} {CDN_KEY}")});
        let server = Server::start(vec![reply(&body.to_string())]);
        let failure = client(&server, Some(CDN_KEY))
            .purge_paths(
                &RequestContext::new().0,
                PurgePathsInput::new("cdn.example.test", vec!["/img/a.webp".into()]),
            )
            .await
            .unwrap_err();
        assert!(!failure.error().has_identity(ErrorClass::NotFound));
        if let Some(detail) = detail {
            assert_ne!(failure.error().detail::<ErrorDetail>(), Some(&detail));
        }
    }
}

#[tokio::test]
async fn purge_paths_sends_exact_paths_once_and_classifies_cooldown() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![reply(include_str!(
        "../fixtures/cdn/purge-success.json"
    ))]);
    let cdn = client(&server, Some(CDN_KEY));
    let input = |path: &str| PurgePathsInput::new("cdn.example.test", vec![path.into()]);
    let failure = cdn.purge_paths(&ctx, input("/*")).await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
    cdn.purge_paths(&ctx, input("/img/a.webp")).await.unwrap();
    assert!(server.heads()[0].starts_with(b"POST /vcdn-api/v1/cdn/flush-cache "));
    let body: Value = serde_json::from_slice(&server.bodies()[0]).unwrap();
    assert!(
        body == json!({"cdnDomain":"cdn.example.test","type":"URI","patterns":["/img/a.webp"]})
    );
    let server = Server::start(vec![reply(
        r#"{"success":false,"code":202,"message":"Last CDN flush cache time less than 30 seconds"}"#,
    )]);
    let failure = client(&server, Some(CDN_KEY))
        .purge_paths(&ctx, input("/img/a.webp"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::PurgeCooldown)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn toggle_after_a_cancelled_send_is_unconfirmed_and_keeps_dispatch_evidence() {
    let (received, request) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![reply(DISABLED), Response::StallBeforeHead(received)]);
    let (ctx, cancel) = RequestContext::new();
    let client = client(&server, Some(CDN_KEY));
    let (result, ()) = tokio::join!(
        client.enable_web_accelerator(&ctx, EnableWebAcceleratorInput::new("cdn-1")),
        async {
            request.await.unwrap();
            cancel.cancel();
        }
    );
    let failure = result.unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::StatusUnconfirmed)
    );
    assert!(failure.error().has_class(ErrorClass::Cancelled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn settlement_refuses_a_reflected_reply_and_retains_the_last_safe_read() {
    let unexpected =
        r#"{"success":true,"data":{"cdnId":"cdn-1","status":42,"domainName":"TOY-CDN-KEY"}}"#;
    let server = Server::start(vec![reply(ACTIVE), reply(TOGGLED), reply(unexpected)]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateWebAcceleratorInput::new("cdn-1");
    input.origin_host_header = Some("new.example.test".into());
    let failure = client(&server, Some(CDN_KEY))
        .update_web_accelerator(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.error().detail::<ErrorDetail>(), None);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let partial = serde_json::to_value(failure.partial().unwrap()).unwrap();
    assert_eq!(partial["WebAccelerator"]["Status"], 1);
    assert!(!partial.to_string().to_ascii_lowercase().contains(CDN_KEY));
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn write_input_refuses_encoded_configured_credentials_before_preflight() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, Some(CDN_KEY));
    let mut update = UpdateWebAcceleratorInput::new("cdn-1");
    update.origin_host_header = Some("toy%2dmanagement%2dtoken".into());
    let failure = client
        .update_web_accelerator(&ctx, update)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let failure = client
        .purge_paths(
            &ctx,
            PurgePathsInput::new("cdn.example.test", vec!["/toy%252dcdn%252dkey".into()]),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn definite_toggle_refusal_skips_confirmation() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        reply(DISABLED),
        reply(include_str!("../fixtures/cdn/write-busy.json")),
    ]);
    let failure = client(&server, Some(CDN_KEY))
        .enable_web_accelerator(&ctx, EnableWebAcceleratorInput::new("cdn-1"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::ResourceBusy)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn recorded_update_and_delete_refusals_are_not_replayed() {
    let (ctx, _) = RequestContext::new();
    for (body, identity, code) in [
        (
            include_str!("../fixtures/cdn/write-busy-update.json"),
            Some(ErrorDetail::ResourceBusy),
            "500",
        ),
        (
            include_str!("../fixtures/cdn/write-package-refused.json"),
            None,
            "500",
        ),
    ] {
        let server = Server::start(vec![reply(ACTIVE), reply(body)]);
        let mut input = UpdateWebAcceleratorInput::new("cdn-1");
        input.lb_type = Some("least_conn".into());
        let failure = client(&server, Some(CDN_KEY))
            .update_web_accelerator(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(failure.error().detail::<ErrorDetail>().copied(), identity);
        assert_eq!(failure.error().api_error().unwrap().code(), code);
        assert_eq!(server.requests(), 2);
    }
    for (body, class, identity) in [
        (
            include_str!("../fixtures/cdn/write-busy-delete.json"),
            ErrorClass::Api,
            Some(ErrorDetail::ResourceBusy),
        ),
        (
            include_str!("../fixtures/cdn/write-not-found.json"),
            ErrorClass::NotFound,
            None,
        ),
    ] {
        let server = Server::start(vec![reply(ACTIVE), reply(body)]);
        let failure = client(&server, Some(CDN_KEY))
            .delete_web_accelerator(&ctx, DeleteWebAcceleratorInput::new("cdn-1"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), class);
        assert_eq!(failure.error().detail::<ErrorDetail>().copied(), identity);
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn purge_retries_429_but_does_not_retry_server_errors_or_follow_up_on_401() {
    let (ctx, _) = RequestContext::new();
    let input = || PurgePathsInput::new("cdn.example.test", vec!["/index.html".into()]);
    let server = Server::start(vec![
        with_status("429 Too Many Requests", "", "application/json"),
        reply(include_str!("../fixtures/cdn/purge-success.json")),
    ]);
    client(&server, Some(CDN_KEY))
        .purge_paths(&ctx, input())
        .await
        .unwrap();
    assert_eq!(server.requests(), 2);
    for (status, class) in [
        ("502 Bad Gateway", ErrorClass::Api),
        ("401 Unauthorized", ErrorClass::Auth),
    ] {
        let server = Server::start(vec![
            with_status(status, "", "application/json"),
            reply(TOGGLED),
        ]);
        let failure = client(&server, Some(CDN_KEY))
            .purge_paths(&ctx, input())
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), class);
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn update_preserves_unmodeled_json_objects_and_number_spellings() {
    let active = r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"unknown":{"$serde_json::private::Number":"1e+6"},"large":123456789123456789123456789}}"#;
    let server = Server::start(vec![reply(active), reply(TOGGLED), reply(ACTIVE)]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateWebAcceleratorInput::new("cdn-1");
    input.lb_type = Some("least_conn".into());
    input.no_wait = true;
    client(&server, Some(CDN_KEY))
        .update_web_accelerator(&ctx, input)
        .await
        .unwrap();
    let bodies = server.bodies();
    let body = String::from_utf8_lossy(&bodies[1]);
    assert!(body.contains(r#""unknown":{"$serde_json::private::Number":"1e+6"}"#));
    assert!(body.contains(r#""large":123456789123456789123456789"#));
}

#[tokio::test]
async fn write_preflight_rejects_object_identifiers() {
    let (ctx, _) = RequestContext::new();
    let object_id =
        r#"{"success":true,"data":{"cdnId":{"$serde_json::private::Number":"12"},"status":1}}"#;
    let server = Server::start(vec![reply(object_id), reply(TOGGLED)]);
    let failure = client(&server, Some(CDN_KEY))
        .delete_web_accelerator(&ctx, DeleteWebAcceleratorInput::new("12"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().code(), "EmptyResponse");
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);

    let numeric_id = r#"{"success":true,"data":{"cdnId":12,"status":1}}"#;
    let server = Server::start(vec![reply(numeric_id), reply(TOGGLED)]);
    client(&server, Some(CDN_KEY))
        .delete_web_accelerator(&ctx, DeleteWebAcceleratorInput::new("12"))
        .await
        .unwrap();
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn key_errors_keep_only_safe_service_codes() {
    let (ctx, _) = RequestContext::new();
    for (status, supplied, expected, message) in [
        (
            "401 Unauthorized",
            "KeyExpired",
            "KeyExpired",
            "vCDN API key rejected: check the key and its expiry",
        ),
        (
            "403 Forbidden",
            "KeyScope",
            "KeyScope",
            "vCDN API key not allowed to call this API",
        ),
        (
            "403 Forbidden",
            "toy%2dcdn%2dkey",
            "Forbidden",
            "vCDN API key not allowed to call this API",
        ),
    ] {
        let body = json!({"code": supplied, "message": CDN_KEY}).to_string();
        let server = Server::start(vec![with_status(status, &body, "application/json")]);
        let failure = client(&server, Some(CDN_KEY))
            .purge_paths(
                &ctx,
                PurgePathsInput::new("cdn.example.test", vec!["/index.html".into()]),
            )
            .await
            .unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert!(
            api.code() == expected,
            "expected the safe error code for {status}"
        );
        assert!(
            api.message() == message,
            "expected the fixed key error for {status}"
        );
        assert_eq!(server.requests(), 1);
    }
    let server = Server::start(vec![with_status(
        "403 Forbidden",
        r#"{"code":"KeyScope"}"#,
        "application/json",
    )]);
    let failure = client(&server, Some(CDN_KEY))
        .list_api_keys(&ctx, ListApiKeysInput)
        .await
        .unwrap_err();
    assert!(failure.error().api_error().unwrap().code() == "KeyScope");
}

#[tokio::test]
async fn purge_rejects_triply_percent_encoded_management_bearer() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![reply(include_str!(
        "../fixtures/cdn/purge-success.json"
    ))]);
    let result = client(&server, Some(CDN_KEY))
        .purge_paths(
            &ctx,
            PurgePathsInput::new(
                "cdn.example.test",
                vec!["/toy%25252dmanagement%25252dtoken".into()],
            ),
        )
        .await;
    assert_eq!(server.requests(), 0);
    let failure = result.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
}

#[tokio::test]
async fn cdn_settlement_exit_refuses_a_reflected_redaction_marker() {
    let latest = r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"domainName":"redacted"}}"#;
    let server = Server::start(vec![reply(DISABLED), reply(TOGGLED), reply(latest)]);
    let result = client(&server, Some("redacted"))
        .enable_web_accelerator(
            &RequestContext::new().0,
            EnableWebAcceleratorInput::new("cdn-1"),
        )
        .await;
    assert_eq!(server.requests(), 3);
    let failure = result.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
}

#[tokio::test]
async fn cdn_settlement_exit_refuses_reflection_created_by_reserializing_rules() {
    let latest = r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"childrenRule":{"left" : "safe","other":"left%22%3A%22"}}}"#;
    let server = Server::start(vec![reply(DISABLED), reply(TOGGLED), reply(latest)]);
    let result = client(&server, Some(r#"left":""#))
        .enable_web_accelerator(
            &RequestContext::new().0,
            EnableWebAcceleratorInput::new("cdn-1"),
        )
        .await;
    assert_eq!(server.requests(), 3);
    let failure = result.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
}

#[tokio::test]
async fn purge_accepts_only_clean_success_envelopes() {
    for (note, allowed) in [
        ("ordinary-note", true),
        (CDN_KEY, false),
        ("%252541", false),
    ] {
        let envelope = json!({"success":true,"note":note}).to_string();
        let server = Server::start(vec![reply(&envelope)]);
        let result = client(&server, Some(CDN_KEY))
            .purge_paths(
                &RequestContext::new().0,
                PurgePathsInput::new("cdn.example.test", vec!["/page".into()]),
            )
            .await;
        assert_eq!(result.is_ok(), allowed);
        if let Err(failure) = result {
            assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        }
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn toggle_confirmation_never_retries_a_guard_refusal_with_an_observed_status() {
    let server = Server::start(vec![
        reply(DISABLED),
        reply(TOGGLED),
        reply("{"),
        reply(ACTIVE),
    ]);
    let mut input = EnableWebAcceleratorInput::new("cdn-1");
    input.no_wait = true;
    let result = client(&server, Some(CDN_KEY))
        .enable_web_accelerator(&RequestContext::new().0, input)
        .await;
    let Err(failure) = result else {
        panic!("confirmation retried the guard refusal");
    };
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::StatusUnconfirmed)
    );
    assert_eq!(server.requests(), 3);
}
