use greennode_sdk::globalloadbalancer::{
    Client, CreateListenerInput, CreatePoolInput, DeleteListenerInput, DeletePoolInput,
};
use greennode_sdk::{Config, ConfigOptions, ErrorClass, RequestContext, Secret, WriteDispatch};

#[tokio::test]
async fn all_child_mutations_refuse_static_auth() {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_static_token(Secret::new("toy-key")),
    )
    .unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let (pool, _, pool_dispatch) = client
        .create_pool(
            &ctx,
            CreatePoolInput::new(
                "glb-1",
                "pool",
                "backend",
                "project-1",
                "server-1",
                "vpc-1",
                "sub-1",
                8080,
            ),
        )
        .await
        .unwrap_err()
        .into_parts();
    let (listener, _, listener_dispatch) = client
        .create_listener(
            &ctx,
            CreateListenerInput::new(
                "glb-1",
                "listener",
                "pool-1",
                443,
                vec!["192.0.2.1/32".into()],
                "project-1",
                "server-1",
            ),
        )
        .await
        .unwrap_err()
        .into_parts();
    let (delete_pool, _, pool_delete_dispatch) = client
        .delete_pool(&ctx, DeletePoolInput::new("glb-1", "pool-1"))
        .await
        .unwrap_err()
        .into_parts();
    let (delete_listener, _, listener_delete_dispatch) = client
        .delete_listener(&ctx, DeleteListenerInput::new("glb-1", "lis-1"))
        .await
        .unwrap_err()
        .into_parts();
    for (error, dispatch) in [
        (pool, pool_dispatch),
        (listener, listener_dispatch),
        (delete_pool, pool_delete_dispatch),
        (delete_listener, listener_delete_dispatch),
    ] {
        assert!(error.has_identity(ErrorClass::InvalidConfig));
        assert_eq!(dispatch, WriteDispatch::NotSent);
    }
}
