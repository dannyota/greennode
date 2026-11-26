use greennode_sdk::globalloadbalancer::{Client, CreateLoadBalancerInput, DeleteLoadBalancerInput};
use greennode_sdk::{
    Config, ConfigOptions, EndpointOverrides, ErrorClass, RequestContext, Secret, WriteDispatch,
};

#[cfg(all(feature = "test-support", not(debug_assertions)))]
#[tokio::test]
async fn release_write_fixture_does_not_unlock_writes() {
    use greennode_sdk::testing::{Product, Server, with_write_loopback};
    let server = Server::start(vec![]);
    let options = with_write_loopback(
        Product::GlobalLoadBalancer,
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("toy-token"))
            .with_endpoint_overrides(EndpointOverrides {
                global_load_balancer: Some(format!("{}/glb-controller/", server.origin)),
                ..Default::default()
            }),
        &server,
    );
    let client = Client::new(&Config::new(options).unwrap());
    let (ctx, _) = RequestContext::new();
    let failure = client
        .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::InvalidConfig));
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn public_endpoint_override_and_static_token_never_unlock_writes() {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("toy-glb-token"))
            .with_endpoint_overrides(EndpointOverrides {
                global_load_balancer: Some("https://127.0.0.1:1/glb-controller/".into()),
                ..Default::default()
            }),
    )
    .unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let create = client
        .create_load_balancer(&ctx, CreateLoadBalancerInput::new("demo", "pkg-1"))
        .await
        .unwrap_err();
    let delete = client
        .delete_load_balancer(&ctx, DeleteLoadBalancerInput::new("glb-1"))
        .await
        .unwrap_err();
    assert!(create.error().has_identity(ErrorClass::InvalidConfig));
    assert!(delete.error().has_identity(ErrorClass::InvalidConfig));
    assert_eq!(create.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(delete.write_dispatch(), WriteDispatch::NotSent);
}
