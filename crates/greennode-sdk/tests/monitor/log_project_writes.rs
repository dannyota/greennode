use super::{channel_writes::client,response,target};
use greennode_sdk::monitor::{CreateLogProjectInput,DeleteLogProjectInput};
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass,RequestContext,WriteDispatch};

const EMPTY:&str=r#"{"content":[],"currentPage":0,"pageSize":100,"totalPages":0,"totalElements":0}"#;
const CLASSES:&str=include_str!("../fixtures/monitor/remaining_log_classes.json");
const QUOTE:&str=include_str!("../fixtures/monitor/remaining_log_quote.json");

#[tokio::test]
async fn log_order_uses_the_exact_body_that_was_priced() {
    let server=Server::start(vec![response("200 OK",EMPTY),response("200 OK",CLASSES),response("200 OK",QUOTE),response("201 Created",r#"{"orderId":4001,"paymentUrl":"withheld"}"#)]);
    let (ctx,_)=RequestContext::new();
    let mut input=CreateLogProjectInput::new("Logs");input.max_price=5.0;input.no_wait=true;
    let output=client(&server).create_log_project(&ctx,input).await.unwrap();
    assert_eq!(output.order_id,"4001");
    assert!(output.log_project.id.is_empty());
    let bodies=server.bodies();
    assert!(bodies[2]==bodies[3]);
    assert_eq!(target(&server.heads()[2]).path(),"/billing-api/v2/log/prices/created-price");
    assert_eq!(target(&server.heads()[3]).path(),"/billing-api/v2/log/quotas");
}

#[tokio::test]
async fn log_order_above_price_sends_nothing_and_ambiguous_order_never_retries() {
    let (ctx,_)=RequestContext::new();
    let mut input=CreateLogProjectInput::new("Logs");input.no_wait=true;
    let server=Server::start(vec![response("200 OK",EMPTY),response("200 OK",CLASSES),response("200 OK",QUOTE)]);
    let failure=client(&server).create_log_project(&ctx,input.clone()).await.unwrap_err();
    assert_eq!(failure.error().class(),ErrorClass::PriceAboveMax);
    assert_eq!(failure.write_dispatch(),WriteDispatch::NotSent);
    assert_eq!(server.requests(),3);
    input.max_price=5.0;
    let server=Server::start(vec![response("200 OK",EMPTY),response("200 OK",CLASSES),response("200 OK",QUOTE),response("503 Service Unavailable","{}"),response("201 Created","{}")]);
    let failure=client(&server).create_log_project(&ctx,input).await.unwrap_err();
    assert_eq!(failure.write_dispatch(),WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(),4);
}

#[tokio::test]
async fn log_project_purge_after_a_missing_live_resource_uses_the_trash_route() {
    let server=Server::start(vec![response("404 Not Found","{}"),response("204 No Content","")]);
    let (ctx,_)=RequestContext::new();
    let mut input=DeleteLogProjectInput::new("project-1");input.purge=true;input.no_wait=true;
    client(&server).delete_log_project(&ctx,input).await.unwrap();
    let heads=server.heads();
    assert!(heads[0].starts_with(b"DELETE /billing-api/v1/log/quotas/project-1 "));
    assert!(heads[1].starts_with(b"DELETE /billing-api/v1/trash/log/quotas/project-1 "));
}
