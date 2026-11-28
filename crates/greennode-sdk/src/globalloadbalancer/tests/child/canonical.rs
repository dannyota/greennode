use super::*;

#[tokio::test]
async fn canonical_guard_keys_cannot_hide_an_extra_region_or_interface() {
    for region_alias in [true, false] {
        let mut f = child_fixture();
        if region_alias {
            f["regions"].as_array_mut().unwrap().push(json!({
                "id": "other-1", "Name": "han-1", "status": "ACTIVE",
                "vserverEndpoint": "https://han-1.api.vngcloud.vn/vserver/vserver-gateway"
            }));
        } else {
            f["server"]["data"]["internalInterfaces"].as_array_mut().unwrap().push(json!({
                "ServerUuid": "server-1", "networkUuid": "vpc-1", "subnetUuid": "sub-1", "fixedIp": "10.1.2.4"
            }));
        }
        let mut replies = catalog(&f);
        replies.extend([
            empty(),
            empty(),
            accepted(&f["pool_accept"]),
            ok(&f["parent"]),
        ]);
        replies.extend(chain(&f));
        replies.push(empty());
        let (client, glb, regional) = servers(replies, placement(&f), true, TOKEN);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
        assert_eq!(
            failure.error().detail::<super::super::DecodeError>(),
            Some(&super::super::DecodeError::InvalidShape)
        );
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(glb.requests(), if region_alias { 2 } else { 3 });
        assert_eq!(regional.requests(), usize::from(!region_alias));
    }
}

#[tokio::test]
async fn canonical_acknowledgement_keys_cannot_hide_a_backend_id() {
    let mut f = child_fixture();
    let group = f["pool_accept"]["globalPoolMembers"][0]
        .as_object_mut()
        .unwrap();
    group.remove("members");
    group.insert("Members".into(), json!([{"id":"row-2"}]));
    let mut replies = catalog(&f);
    replies.extend([
        empty(),
        empty(),
        accepted(&f["pool_accept"]),
        ok(&f["parent"]),
    ]);
    replies.extend(chain(&f));
    replies.push(empty());
    let (client, glb, _regional) = servers(replies, placement(&f), true, TOKEN);
    let (ctx, _) = RequestContext::new();
    let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert_eq!(glb.requests(), 6);
}
