use crate::testing::{Response, Server};
use crate::{RequestContext, WriteDispatch};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use crate::testing::{Product, with_clock, with_product_loopback};
use crate::{Clock, Config, ConfigOptions, EndpointOverrides, ErrorClass, Secret};
use http::{Method, StatusCode};

use super::wire::{Request, Write};

use super::super::tests::{client, reply};
use super::*;

#[tokio::test]
async fn vpc_delete_refuses_lone_aliased_guard_fields() {
    let server = Server::start(vec![reply("200 OK", r#"{"id":"vpc-1","ServerCount":1}"#)]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .delete_vpc(
            &ctx,
            DeleteVpcInput {
                vpc_id: "vpc-1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn vpc_rename_does_not_follow_a_same_origin_redirect() {
    let redirect = Response::Http {
        status: "307 Temporary Redirect".into(),
        headers: vec![
            ("Location".into(), "/redirected".into()),
            ("Content-Length".into(), "0".into()),
        ],
        body: Vec::new(),
    };
    let server = Server::start(vec![redirect]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .update_vpc(
            &ctx,
            UpdateVpcInput {
                vpc_id: "vpc-1".into(),
                name: "new-name".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().api_error().unwrap().status(), None);
    assert_eq!(server.requests(), 1);
}

struct RetryClock {
    now: tokio::time::Instant,
    delays: AtomicUsize,
}

impl RetryClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            now: tokio::time::Instant::now(),
            delays: AtomicUsize::new(0),
        })
    }
}

impl Clock for RetryClock {
    fn utc_now(&self) -> time::OffsetDateTime {
        time::OffsetDateTime::UNIX_EPOCH
    }
    fn monotonic_now(&self) -> tokio::time::Instant {
        self.now
    }
    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        if deadline <= self.now + Duration::from_secs(5) {
            Box::pin(async move {
                self.delays.fetch_add(1, Ordering::SeqCst);
            })
        } else {
            Box::pin(std::future::pending())
        }
    }
}

fn options() -> ConfigOptions {
    ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_static_token(Secret::new("toy-credential"))
}

fn with_retry_clock(server: &Server, clock: Arc<RetryClock>) -> crate::network::Client {
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        with_clock(options(), clock),
        server,
    ))
    .unwrap();
    crate::network::Client::new(&config)
}

#[tokio::test]
async fn write_retry_policies_on_503() {
    let (ctx, _) = RequestContext::new();
    let clock = RetryClock::new();
    let server = Server::start(vec![
        reply("503 Service Unavailable", ""),
        reply("200 OK", ""),
        reply("200 OK", r#"{"id":"vpc-1","displayName":"new-name"}"#),
    ]);
    let result = with_retry_clock(&server, clock.clone())
        .update_vpc(
            &ctx,
            UpdateVpcInput {
                vpc_id: "vpc-1".into(),
                name: "new-name".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.vpc.name, "new-name");
    assert_eq!(clock.delays.load(Ordering::SeqCst), 1);
    assert_eq!(server.requests(), 3);

    let server = Server::start(vec![reply("503 Service Unavailable", "")]);
    let failure = with_retry_clock(&server, RetryClock::new())
        .create_vpc(
            &ctx,
            CreateVpcInput {
                name: "new-name".into(),
                cidr: "10.0.0.0/16".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), Some(503));
    assert_eq!(server.requests(), 1);

    let server = Server::start(vec![
        reply("200 OK", r#"{"id":"vpc-1","dnsStatus":"DISABLED"}"#),
        reply("503 Service Unavailable", ""),
    ]);
    let failure = with_retry_clock(&server, RetryClock::new())
        .enable_vpc_private_dns(
            &ctx,
            EnableVpcPrivateDnsInput {
                vpc_id: "vpc-1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn write_retry_policies_on_failed_dial() {
    const REQUEST: Request = Request::new(
        "network.CreateVPC",
        "v2/{}/networks",
        &Method::POST,
        &[StatusCode::OK],
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    for (request, delays) in [(REQUEST.once(), 0), (REQUEST, 3), (REQUEST.repeatable(), 3)] {
        let clock = RetryClock::new();
        let config = Config::new(with_clock(
            options().with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("https://{address}")),
                ..EndpointOverrides::default()
            }),
            clock.clone(),
        ))
        .unwrap();
        let client = crate::network::Client::new(&config);
        let (ctx, _) = RequestContext::new();
        let error = Write::new::<()>(&client, &ctx)
            .unwrap()
            .send::<()>(request, &[], None)
            .await
            .unwrap_err();
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(clock.delays.load(Ordering::SeqCst), delays);
    }
}

#[tokio::test]
async fn vpc_create_uses_a_path_id_only_when_it_will_poll() {
    let (ctx, _) = RequestContext::new();
    for no_wait in [true, false] {
        let server = Server::start(vec![reply(
            "200 OK",
            r#"{"data":{"id":"vpc_1","status":"ACTIVE"}}"#,
        )]);
        let result = client(&server)
            .create_vpc(
                &ctx,
                CreateVpcInput {
                    name: "example".into(),
                    cidr: "10.0.0.0/16".into(),
                    no_wait,
                },
            )
            .await;
        if no_wait {
            assert_eq!(result.unwrap().vpc.uuid, "vpc_1");
        } else {
            let failure = result.unwrap_err();
            assert_eq!(
                failure.error().api_error().unwrap().code(),
                "InvalidResponse"
            );
            assert!(failure.partial().is_none());
        }
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn subnet_delete_checks_servers_before_other_guards() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"uuid":"subnet-1","networkUuid":"vpc-1","status":"ACTIVE"}"#,
        ),
        reply("200 OK", r#"[{"uuid":"server-1"}]"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .delete_subnet(
            &ctx,
            DeleteSubnetInput {
                vpc_id: "vpc-1".into(),
                subnet_id: "subnet-1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        error.error().detail::<crate::network::ResourceInUse>(),
        Some(&crate::network::ResourceInUse)
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn subnet_delete_refuses_an_acl_attachment_found_on_the_second_page() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"uuid":"subnet-1","networkUuid":"vpc-1","status":"ACTIVE"}"#,
        ),
        reply("200 OK", "[]"),
        reply(
            "200 OK",
            r#"{"listData":[],"page":1,"pageSize":1,"totalPage":0,"totalItem":0}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[],"page":1,"pageSize":1,"totalPage":0,"totalItem":0}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[{"uuid":"acl-1","networkId":"vpc-2"}],"page":1,"pageSize":1,"totalPage":2,"totalItem":2}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[{"uuid":"acl-2","networkId":"vpc-1"}],"page":2,"pageSize":1,"totalPage":2,"totalItem":2}"#,
        ),
        reply(
            "200 OK",
            r#"{"data":{"uuid":"acl-2","status":"ACTIVE","interfaceNetworkUuid":"vpc-1","defaultAcl":false,"subnetAssociationList":["subnet-1"],"aclPolicyRules":[]}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .delete_subnet(
            &ctx,
            DeleteSubnetInput {
                vpc_id: "vpc-1".into(),
                subnet_id: "subnet-1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        error.error().detail::<crate::network::ResourceInUse>(),
        Some(&crate::network::ResourceInUse)
    );
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn acl_delete_checks_complete_absence_after_500_but_not_after_truncation() {
    let (ctx, _) = RequestContext::new();
    for truncated in [false, true] {
        let mutation = if truncated {
            Response::RawClose(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 100\r\n\r\n{}".to_vec(),
            )
        } else {
            reply("500 Internal Server Error", "{}")
        };
        let server = Server::start(vec![
            reply(
                "200 OK",
                r#"{"data":{"uuid":"acl-1","status":"ACTIVE","interfaceNetworkUuid":"vpc-1","defaultAcl":false,"subnetAssociationList":[],"aclPolicyRules":[]}}"#,
            ),
            mutation,
            reply(
                "200 OK",
                r#"{"listData":[],"page":1,"pageSize":1,"totalPage":0,"totalItem":0}"#,
            ),
        ]);
        let result = client(&server)
            .delete_network_acl(
                &ctx,
                DeleteNetworkAclInput {
                    network_acl_id: "acl-1".into(),
                },
            )
            .await;
        if truncated {
            let failure = result.unwrap_err();
            assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
            assert_eq!(failure.error().api_error().unwrap().status(), None);
            assert_eq!(server.requests(), 2);
        } else {
            assert!(result.is_ok());
            assert_eq!(server.requests(), 3);
        }
    }
}

#[tokio::test]
async fn route_replacement_refuses_a_concurrent_edit() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"data":{"uuid":"rt-1","status":"ACTIVE","routes":[]}}"#,
        ),
        reply(
            "200 OK",
            r#"{"data":{"uuid":"rt-1","status":"ACTIVE","routes":[{"destinationCidrBlock":"10.0.0.0/24","target":"10.0.0.1"}]}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .add_route(
            &ctx,
            AddRouteInput {
                route_table_id: "rt-1".into(),
                destination_cidr: "10.0.1.0/24".into(),
                target: "10.0.0.1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        error.error().detail::<NetworkWriteError>(),
        Some(&NetworkWriteError::Busy)
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn ambiguous_route_write_keeps_the_observed_table_without_replaying() {
    let initial = r#"{"data":{"uuid":"rt-1","status":"ACTIVE","routes":[]}}"#;
    let server = Server::start(vec![
        reply("200 OK", initial),
        reply("200 OK", initial),
        reply("503 Service Unavailable", ""),
        reply(
            "200 OK",
            r#"{"data":{"uuid":"rt-1","status":"ACTIVE","routes":[{"destinationCidrBlock":"10.0.1.0/24","target":"10.0.0.1"}]}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .add_route(
            &ctx,
            AddRouteInput {
                route_table_id: "rt-1".into(),
                destination_cidr: "10.0.1.0/24".into(),
                target: "10.0.0.1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(error.error().has_class(ErrorClass::NotSettled));
    assert_eq!(error.partial().unwrap().route_table.routes.len(), 1);
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn write_refuses_a_body_with_an_encoded_credential() {
    let server = Server::start(Vec::new());
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .update_vpc(
            &ctx,
            UpdateVpcInput {
                vpc_id: "vpc-1".into(),
                name: "toy%2dcredential".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert!(error.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn vpc_create_redacts_a_reflecting_display_name() {
    let server = Server::start(vec![reply(
        "200 OK",
        r#"{"data":{"id":"vpc-1","displayName":"toy%2dcredential"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server)
        .create_vpc(
            &ctx,
            CreateVpcInput {
                name: "example".into(),
                cidr: "10.0.0.0/16".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.vpc.name, "<redacted>");
}

#[tokio::test]
async fn dns_enable_screens_its_changed_field_against_the_bearer() {
    let server = Server::start(vec![
        reply("200 OK", r#"{"id":"vpc-1","dnsStatus":"DISABLED"}"#),
        reply("200 OK", ""),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_static_token(Secret::new("true")),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let result = crate::network::Client::new(&config)
        .enable_vpc_private_dns(
            &ctx,
            EnableVpcPrivateDnsInput {
                vpc_id: "vpc-1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(result.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(result.partial().is_none());
    assert_eq!(server.requests(), 2);
}

mod review;
