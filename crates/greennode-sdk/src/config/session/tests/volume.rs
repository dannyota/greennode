use super::*;
use crate::volume::{Client, CreateVolumeInput};
use crate::wire::test_server::Server;
use crate::{RecoveryAdvice, TestClock, WriteDispatch};

struct ExpireAfterAcknowledgement {
    clock: TestClock,
    bodies: AtomicUsize,
}

impl Clock for ExpireAfterAcknowledgement {
    fn utc_now(&self) -> time::OffsetDateTime {
        self.clock.utc_now()
    }

    fn monotonic_now(&self) -> tokio::time::Instant {
        self.clock.monotonic_now()
    }

    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.clock.sleep_until(deadline)
    }

    fn response_body_started(&self) {
        if self.bodies.fetch_add(1, Ordering::Relaxed) == 3 {
            self.clock.advance(Duration::from_secs(2));
        }
    }
}

#[tokio::test]
async fn volume_confirmation_preserves_the_screened_service_account_login_reason() {
    for (bearer, expected) in [
        (
            "toy-volume-token",
            "token endpoint rejected request (status 403)",
        ),
        ("403)", "request failed"),
    ] {
        let token = serde_json::json!({"access_token":bearer,"expires_in":31}).to_string();
        let login = Server::start(vec![
            response("200 OK", None, &token),
            response("403 Forbidden", None, "{}"),
        ]);
        let product = Server::start(vec![
            response(
                "200 OK",
                None,
                r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            ),
            response("200 OK", None, r#"{"optimumPrice":3}"#),
            response("202 Accepted", None, r#"{"data":{"uuid":"volume-1"}}"#),
        ]);
        let clock = Arc::new(ExpireAfterAcknowledgement {
            clock: TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
            bodies: AtomicUsize::new(0),
        });
        let mut config = Config::new(
            options()
                .with_service_account(service())
                .with_project_id("project-1".into())
                .with_clock(clock)
                .with_endpoint_overrides(EndpointOverrides {
                    portal: Some(product.origin.clone()),
                    vserver: Some(product.origin.clone()),
                    ..Default::default()
                }),
        )
        .unwrap();
        let session = Arc::get_mut(&mut config.session).unwrap();
        let Auth::ServiceAccount(source) = &mut session.auth else {
            panic!("wrong auth kind")
        };
        source.set_test_endpoint(login.origin.clone(), login.root_der.clone());
        session.set_test_product_root(product.root_der.clone());
        let (ctx, _) = RequestContext::new();
        let failure = Client::new(&config)
            .create_volume(
                &ctx,
                CreateVolumeInput {
                    name: "toy-volume".into(),
                    size: 1,
                    volume_type_id: "type-1".into(),
                    zone_id: "zone-1".into(),
                    max_price: 3.0,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!((product.requests(), login.requests()), (3, 2));
        assert!(failure.partial().is_none());
        assert!(failure.error().has_identity(ErrorClass::NotSettled));
        assert!(failure.error().service_account_login_error().is_some());
        let reason = failure
            .error()
            .detail::<RecoveryAdvice>()
            .unwrap()
            .message();
        assert!(!reason.contains(bearer));
        assert!(
            reason
                == format!(
                    "vngcloud: write outcome not confirmed: volume.CreateVolume: volume volume-1: {expected}"
                ),
            "incorrect confirmation diagnostic"
        );
    }
}
