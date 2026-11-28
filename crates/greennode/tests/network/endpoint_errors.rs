use super::*;
use greennode_sdk::testing::{ResponseBodyClock, with_clock};
use std::{sync::Arc, time::Duration};

async fn interrupt_project_discovery(bearer: &str, status: &str) -> Vec<u8> {
    let (head, _) = tokio::sync::oneshot::channel();
    let (eof, _) = tokio::sync::oneshot::channel();
    let vserver = Server::start(vec![Response::ObserveStall {
        head: format!("HTTP/1.1 {status}\r\nContent-Length: 10\r\n\r\n").into_bytes(),
        sent: head,
        eof,
    }]);
    let vnetwork = Server::start(Vec::new());
    let clock = Arc::new(ResponseBodyClock::new());
    let options = with_clock(
        ConfigOptions::default().with_static_token(Secret::new(bearer)),
        clock.clone(),
    );
    let options = with_product_loopback(Product::Vserver, options, &vserver);
    let options = with_product_loopback(Product::Vnetwork, options, &vnetwork);
    let (context, cancel) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let work = run_invocation(Invocation {
        args: [
            "greennode",
            "--region",
            "hcm-3",
            "--read-only",
            "network",
            "quote-create-endpoint",
            "--name",
            "edge",
            "--vpc-id",
            "net-00000000-0000-0000-0000-000000000002",
            "--subnet-id",
            "sub-00000000-0000-0000-0000-000000000003",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
        source: &Source,
        config_options: options,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: clock.as_ref(),
        context: &context,
    });
    let interrupt = async {
        clock.started().await;
        cancel.cancel();
    };
    let (exit, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(work, interrupt)
    })
    .await
    .unwrap();
    assert_eq!(exit, ExitCode::FAILURE);
    assert!(stdout.is_empty());
    assert_eq!(vserver.requests(), 1);
    assert_eq!(vnetwork.requests(), 0);
    stderr
}

#[tokio::test]
async fn endpoint_quote_cli_keeps_the_outer_joined_interruption_diagnostic() {
    let stderr = interrupt_project_discovery(TOKEN, "403 Forbidden").await;
    assert_eq!(stderr, concat!(
        r#"{"error":{"code":"Forbidden","message":"network.QuoteCreateEndpoint: endpoint response withheld (status 403)\nnetwork.QuoteCreateEndpoint: context canceled","status":403,"operation":"network.QuoteCreateEndpoint"}}"#,
        "\n",
    ).as_bytes());
}

#[tokio::test]
async fn endpoint_quote_cli_screens_the_rebuilt_context_reason() {
    let bearer = "canceled";
    let stderr = interrupt_project_discovery(bearer, "403 Forbidden").await;
    assert!(!String::from_utf8_lossy(&stderr).contains(bearer));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["status"], 403);
    assert_eq!(error["error"]["code"], "Forbidden");
}

#[tokio::test]
async fn endpoint_quote_cli_screens_context_only_after_an_interrupted_200() {
    let bearer = "canceled";
    let stderr = interrupt_project_discovery(bearer, "200 OK").await;
    assert!(!String::from_utf8_lossy(&stderr).contains(bearer));
    let value: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(value["error"]["code"], "RequestFailed");
    assert!(value["error"].get("status").is_none());
}
