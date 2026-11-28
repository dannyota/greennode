mod guarded_create;

use super::*;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Provider<const N: usize> {
    calls: AtomicUsize,
    tokens: [&'static str; N],
}
impl<const N: usize> crate::CredentialsProvider for Provider<N> {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
        let i = self.calls.fetch_add(1, Ordering::Relaxed);
        let token = self.tokens.get(i).unwrap_or(&"toy-later");
        Box::pin(async move { Ok(crate::Token::new(crate::Secret::new(*token), None)) })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a crate::Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
fn provider<const N: usize>(tokens: [&'static str; N]) -> Arc<Provider<N>> {
    Arc::new(Provider {
        calls: AtomicUsize::new(0),
        tokens,
    })
}

#[tokio::test]
async fn create_error_and_advice_withhold_a_reflected_operation() {
    assert_safe_create_failure("CreateVolume", false).await;
}

#[tokio::test]
async fn create_keeps_the_action_when_the_recovery_message_is_withheld() {
    assert_safe_create_failure("before", true).await;
}

async fn assert_safe_create_failure(bearer: &str, withheld: bool) {
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply(
            "500 Internal Server Error",
            r#"{"message":"provider refused"}"#,
        ),
    ]);
    let client = volume_with_options(
        &server,
        crate::ConfigOptions::default().with_static_token(crate::Secret::new(bearer)),
        true,
    );
    let (ctx, _) = RequestContext::new();
    let failure = client.create_volume(&ctx, create()).await.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 3);
    assert_eq!(
        failure.recovery_action(),
        Some(crate::RecoveryAction::CheckExistingVolume)
    );
    assert_eq!(failure.error().class(), ErrorClass::Api);
    let advice = failure.error().recovery_diagnostic().unwrap();
    assert!(!advice.contains(bearer));
    assert!(!format!("{failure:?} {}", failure.error()).contains(bearer));
    let api = failure.error().api_error().unwrap();
    assert!(
        [api.operation(), api.code(), api.message(), &api.to_string()]
            .into_iter()
            .all(|field| !field.contains(bearer))
    );
    assert_eq!(api.status(), Some(500));
    if withheld {
        assert!(advice.is_empty());
    } else {
        assert!(api.operation().is_empty());
        assert!(advice.contains("existing volume"));
    }
}

fn volume_with_options(server: &Server, options: crate::ConfigOptions, project: bool) -> Client {
    let options = if project {
        options.with_project_id("project-1".into())
    } else {
        options
    };
    let config = crate::Config::new(options.with_region("han-1".into()).with_endpoint_overrides(
        crate::EndpointOverrides {
            vserver: Some(format!("{}/server/", server.origin)),
            portal: Some(format!("{}/portal/", server.origin)),
            ..Default::default()
        },
    ))
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn api_display_withholds_a_credential_completed_by_the_status_suffix() {
    let bearer = "500)";
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply(
            "500 Internal Server Error",
            r#"{"message":"provider refused"}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = volume_with_options(
        &server,
        crate::ConfigOptions::default().with_static_token(crate::Secret::new(bearer)),
        true,
    );
    let failure = client.create_volume(&ctx, create()).await.unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(500));
    assert_eq!(api.message(), "provider refused");
    assert!(!api.to_string().contains(bearer));
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn screened_oversized_client_refusal_does_not_get_ambiguous_write_advice() {
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        Server::oversized("404 Not Found"),
    ]);
    let client = volume_with_options(
        &server,
        crate::ConfigOptions::default().with_credentials_provider(provider([
            "toy-first",
            "toy-second",
            "404",
        ])),
        true,
    );
    let (ctx, _) = RequestContext::new();
    let failure = client.create_volume(&ctx, create()).await.unwrap_err();
    assert_eq!(server.requests(), 3, "class: {:?}", failure.error().class());
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_class(ErrorClass::NotFound));
    assert_eq!(failure.error().api_error().unwrap().status(), None);
    assert!(failure.recovery_action().is_none());
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn paid_order_is_sent_when_recovery_text_is_withheld() {
    let mut mismatches = Vec::new();
    for project in [false, true] {
        let mut responses = Vec::new();
        if !project {
            responses.push(response(
                r#"{"projects":[{"projectId":"project-1","region":"han-1"}]}"#,
            ));
        }
        responses.extend([
            reply("401 Unauthorized", "{}"),
            response(EMPTY),
            response(QUOTE),
            reply("500 Internal Server Error", "{}"),
        ]);
        let server = Server::start(responses);
        let options = crate::ConfigOptions::default()
            .with_cdn_api_key(crate::Secret::new("inventory"))
            .with_credentials_provider(provider(["before", "Check", "Reconcile", "Audit"]));
        let client = volume_with_options(&server, options, project);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_volume(&ctx, create()).await.unwrap_err();
        let order_was_sent = server
            .heads()
            .iter()
            .any(|head| head.starts_with(b"POST /server/v2/project-1/volumes "));
        if failure.write_dispatch() != WriteDispatch::MayHaveBeenSent
            || failure.error().class() != ErrorClass::Api
            || server.requests() != if project { 4 } else { 5 }
            || !order_was_sent
            || failure.recovery_action() != Some(crate::RecoveryAction::CheckExistingVolume)
            || failure.error().recovery_diagnostic() != Some("")
        {
            mismatches.push((
                project,
                failure.write_dispatch(),
                failure.error().class(),
                server.requests(),
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "unexpected paid order outcomes: {mismatches:?}"
    );
}

#[tokio::test]
async fn accepted_create_keeps_the_recovery_action_when_text_is_withheld() {
    for has_id in [false, true] {
        let body = if has_id {
            r#"{"data":{"uuid":"volume-1"}}"#
        } else {
            r#"{"data":{}}"#
        };
        let server = Server::start(vec![
            response(EMPTY),
            response(QUOTE),
            reply("202 Accepted", body),
            reply("403 Forbidden", "{}"),
        ]);
        let client = volume_with_options(
            &server,
            crate::ConfigOptions::default().with_static_token(crate::Secret::new("before")),
            true,
        );
        let (ctx, _) = RequestContext::new();
        let failure = client.create_volume(&ctx, create()).await.unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), if has_id { 4 } else { 3 });
        let error = failure.error();
        assert_eq!(
            failure.recovery_action(),
            Some(crate::RecoveryAction::CheckExistingVolume)
        );
        assert_eq!(error.recovery_diagnostic(), Some(""));
        assert!(
            error
                .api_error()
                .is_none_or(|api| !api.to_string().contains("before"))
        );
    }
}

#[tokio::test]
async fn create_finish_keeps_the_recovery_action_for_uncertain_settlement() {
    let mut missing = Vec::new();
    for withheld_output in [false, true] {
        let server = Server::start(vec![
            response(EMPTY),
            response(QUOTE),
            reply("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
            if withheld_output {
                response(AVAILABLE)
            } else {
                reply("403 Forbidden", "{}")
            },
        ]);
        let final_bearer = if withheld_output { "3200" } else { "toy-final" };
        let client = volume_with_options(
            &server,
            crate::ConfigOptions::default().with_credentials_provider(provider([
                "the",
                "toy-quote",
                "toy-write",
                final_bearer,
            ])),
            true,
        );
        let (ctx, _) = RequestContext::new();
        let failure = client.create_volume(&ctx, create()).await.unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), 4);
        let expected_action =
            (!withheld_output).then_some(crate::RecoveryAction::CheckExistingVolume);
        if failure.recovery_action() != expected_action
            || failure.error().has_identity(ErrorClass::NotSettled) == withheld_output
            || failure.error().recovery_diagnostic() != (!withheld_output).then_some("")
        {
            missing.push(withheld_output);
        }
    }
    assert!(
        missing.is_empty(),
        "missing screened warning for withheld output cases: {missing:?}"
    );
}

#[tokio::test]
async fn create_keeps_actionable_recovery_after_confirmation_rotates_credentials() {
    for confirmation in ["toy-confirm", "Check"] {
        let server = Server::start(vec![
            response(EMPTY),
            response(QUOTE),
            reply("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
            reply("403 Forbidden", "{}"),
        ]);
        let tokens = provider(["toy-inventory", "toy-quote", "Audit", confirmation]);
        let options = crate::ConfigOptions::default().with_credentials_provider(tokens.clone());
        let client = volume_with_options(&server, options, true);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_volume(&ctx, create()).await.unwrap_err();
        assert_eq!(server.requests(), 4);
        assert_eq!(tokens.calls.load(Ordering::Relaxed), 4);
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.error().has_class(ErrorClass::NotSettled));
        let action = Some(crate::RecoveryAction::CheckExistingVolume);
        assert_eq!(failure.recovery_action(), action);
        assert_eq!(failure.error().recovery_action(), action);
        let diagnostic = failure.error().recovery_diagnostic().unwrap();
        assert_eq!(diagnostic.is_empty(), confirmation == "Check");
    }
}

#[tokio::test]
async fn delete_settles_when_confirmation_404_is_withheld() {
    let server = Server::start(vec![
        response(AVAILABLE),
        reply(ACCEPTED, ""),
        reply("404 Not Found", "{}"),
    ]);
    let options = crate::ConfigOptions::default().with_static_token(crate::Secret::new("404"));
    let client = volume_with_options(&server, options, true);
    let (ctx, _) = RequestContext::new();
    let result = client
        .delete_volume(
            &ctx,
            DeleteVolumeInput {
                volume_id: "volume-1".into(),
                no_wait: false,
            },
        )
        .await;
    assert_eq!(server.requests(), 3);
    assert!(result.is_ok());
}

#[tokio::test]
async fn available_confirmation_with_withheld_output_does_not_mark_the_write_uncertain() {
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
        response(AVAILABLE),
    ]);
    let options = crate::ConfigOptions::default().with_credentials_provider(provider([
        "toy-inventory",
        "toy-quote",
        "toy-write",
        "3200",
    ]));
    let client = volume_with_options(&server, options, true);
    let (ctx, _) = RequestContext::new();
    let failure = client.create_volume(&ctx, create()).await.unwrap_err();
    assert_eq!(server.requests(), 4);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert!(!failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(failure.recovery_action(), None);
}

#[tokio::test]
async fn provider_page_shape_message_stays_an_http_inventory_error() {
    let body = serde_json::json!({"message": crate::volume::wire::PAGE_SHAPE}).to_string();
    let server = Server::start(vec![reply("403 Forbidden", &body)]);
    let client = volume_with_options(
        &server,
        crate::ConfigOptions::default().with_static_token(crate::Secret::new("toy-token")),
        true,
    );
    let (ctx, _) = RequestContext::new();
    let failure = client.create_volume(&ctx, create()).await.unwrap_err();
    assert_eq!(server.requests(), 1);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.error().has_class(ErrorClass::PermissionDenied));
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.operation(), api.status(), api.code()),
        ("volume.ListVolumes", Some(403), "Forbidden")
    );
    assert_eq!(failure.recovery_action(), None);
}

#[tokio::test]
async fn create_settlement_bound_warns_that_the_volume_exists() {
    use crate::{Clock, testing::ResponseBodyClock};
    use std::time::Duration;
    for caller_bound in [Some(Duration::from_secs(5)), None] {
        let creating = AVAILABLE.replace("AVAILABLE", "CREATING");
        let server = Server::start(vec![
            response(EMPTY),
            response(QUOTE),
            reply("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
            response(&creating),
        ]);
        let clock = Arc::new(ResponseBodyClock::new());
        let client = volume_with_options(
            &server,
            crate::ConfigOptions::default()
                .with_static_token(crate::Secret::new("toy-bearer"))
                .with_clock(clock.clone()),
            true,
        );
        let (ctx, _) = RequestContext::new();
        let ctx = match caller_bound {
            Some(duration) => ctx.child_with_deadline(clock.monotonic_now() + duration),
            None => ctx,
        };
        let advance = async {
            clock.wait_for_sleep(Duration::from_secs(2)).await;
            clock.advance(caller_bound.unwrap_or(Duration::from_secs(300)));
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::join!(client.create_volume(&ctx, create()), advance)
        })
        .await
        .unwrap();
        let failure = result.unwrap_err();
        assert_eq!(server.requests(), 4);
        assert!(failure.error().has_identity(ErrorClass::NotSettled));
        assert!(failure.error().has_identity(ErrorClass::DeadlineExceeded));
        let reason = failure
            .error()
            .detail::<crate::RecoveryAdvice>()
            .unwrap()
            .message();
        if caller_bound.is_some() {
            assert_eq!(
                failure.recovery_action(),
                Some(crate::RecoveryAction::CheckExistingVolume)
            );
            assert_eq!(
                reason,
                "vngcloud: write outcome not confirmed: volume.CreateVolume: volume volume-1: context deadline exceeded"
            );
        } else {
            assert_eq!(
                failure.recovery_action(),
                Some(crate::RecoveryAction::DoNotRepeatVolumeCreate)
            );
            assert_eq!(
                reason,
                "vngcloud: write outcome not confirmed: volume.CreateVolume: volume volume-1 did not reach AVAILABLE within 5m0s\ncontext deadline exceeded"
            );
            assert_eq!(
                failure.error().recovery_diagnostic(),
                Some("The volume exists and this create must not be repeated.")
            );
        }
    }
}

#[tokio::test]
async fn create_keeps_the_confirmation_get_cause_when_its_body_is_canceled() {
    use crate::testing::ResponseBodyClock;
    use std::time::Duration;
    let (head, _) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
        Response::Stall(head, 10),
    ]);
    let clock = Arc::new(ResponseBodyClock::new());
    let client = volume_with_options(
        &server,
        crate::ConfigOptions::default()
            .with_static_token(crate::Secret::new("toy-bearer"))
            .with_clock(clock.clone()),
        true,
    );
    let (ctx, cancel) = RequestContext::new();
    let interrupt = async {
        clock.wait_for_body(4).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(client.create_volume(&ctx, create()), interrupt)
    })
    .await
    .unwrap();
    let failure = result.unwrap_err();
    assert_eq!(server.requests(), 4);
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert!(failure.error().has_identity(ErrorClass::Cancelled));
    assert_eq!(
        failure.error().api_error().map(crate::ApiError::operation),
        Some("volume.GetVolume")
    );
    assert_eq!(
        failure.recovery_action(),
        Some(crate::RecoveryAction::CheckExistingVolume)
    );
}

#[tokio::test]
async fn create_refuses_noncanonical_acknowledgement_fields_after_dispatch() {
    for body in [
        r#"{"data":{"UUID":"volume-1"}}"#,
        r#"{"Data":{"uuid":"volume-1"}}"#,
    ] {
        let server = Server::start(vec![
            response(EMPTY),
            response(QUOTE),
            reply("202 Accepted", body),
        ]);
        let client = volume_with_options(
            &server,
            crate::ConfigOptions::default().with_static_token(crate::Secret::new("toy-bearer")),
            true,
        );
        let (ctx, _) = RequestContext::new();
        let mut input = create();
        input.no_wait = true;
        let failure = client.create_volume(&ctx, input).await.unwrap_err();
        assert_eq!(server.requests(), 3);
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse"
        );
        assert_eq!(
            failure.recovery_action(),
            Some(crate::RecoveryAction::CheckExistingVolume)
        );
    }
}

#[test]
fn internal_settlement_timeout_with_nested_deadline_keeps_its_warning() {
    use crate::{volume::write, wire::product::evidence::Evidence};
    let evidence = Evidence::new();
    let cause = ErrorClass::DeadlineExceeded;
    let timed_out = crate::Error::new(ErrorClass::NotSettled)
        .with_detail(crate::RecoveryAdvice::new(
            "the volume exists and this create must not be repeated",
        ))
        .with_cause(crate::Error::new(cause));
    let error = write::settlement_error(
        timed_out,
        "volume.CreateVolume",
        "volume-1",
        None,
        Some(crate::RecoveryAction::CheckExistingVolume),
        &evidence,
    );
    assert!(error.has_identity(cause));
    assert_eq!(
        error.detail::<crate::RecoveryAdvice>().unwrap().message(),
        "the volume exists and this create must not be repeated"
    );
}

#[tokio::test]
async fn attach_confirmation_accepts_go_unicode_case_folding() {
    let attached = IN_USE.replace("IN-USE", "iN-uſE");
    let server = Server::start(vec![
        response(AVAILABLE),
        reply(ACCEPTED, ""),
        response(&attached),
        reply("403 Forbidden", "{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let options = crate::ConfigOptions::default()
        .with_static_token(crate::Secret::new("toy-token"))
        .with_clock(std::sync::Arc::new(
            crate::testing::clock_advancing_on_sleep(
                crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
                std::time::Duration::from_secs(2),
            ),
        ));
    let result = volume_with_options(&server, options, true)
        .attach_volume(
            &ctx,
            AttachVolumeInput {
                volume_id: "volume-1".into(),
                server_id: "server-1".into(),
                no_wait: false,
            },
        )
        .await;
    assert_eq!(server.requests(), 3);
    assert_eq!(result.unwrap().volume.status, "iN-uſE");
}

#[test]
fn confirmation_token_cache_failure_keeps_the_fixed_reason() {
    let cause = crate::Error::new(ErrorClass::TokenCache);
    let error = crate::volume::write::settlement_error(
        cause,
        "volume.CreateVolume",
        "volume-1",
        None,
        Some(crate::RecoveryAction::CheckExistingVolume),
        &crate::wire::product::evidence::Evidence::new(),
    );
    assert!(error.has_identity(ErrorClass::NotSettled));
    assert!(error.has_identity(ErrorClass::TokenCache));
    assert_eq!(
        error.detail::<crate::RecoveryAdvice>().unwrap().message(),
        "vngcloud: write outcome not confirmed: volume.CreateVolume: volume volume-1: vngcloud: token cache unavailable"
    );
}
