use super::*;
use greennode_sdk::testing::{ResponseBodyClock, with_clock};
use std::{sync::Arc, time::Duration};

async fn interrupt_project_discovery(
    bearer: &str,
    status: &str,
    command: &str,
    flags: &[&str],
) -> Vec<u8> {
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
    let fixture = if command == "quote-create-endpoint" {
        with_product_loopback
    } else {
        greennode_sdk::testing::with_write_loopback
    };
    let options = fixture(Product::Vserver, options, &vserver);
    let options = fixture(Product::Vnetwork, options, &vnetwork);
    let (context, cancel) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let work = run_invocation(Invocation {
        args: [
            "greennode",
            "--region",
            "hcm-3",
            "network",
            command,
            "--vpc-id",
            "net-00000000-0000-0000-0000-000000000002",
        ]
        .into_iter()
        .chain(flags.iter().copied())
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
    let (exit, ()) = tokio::time::timeout(Duration::from_secs(60), async {
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

const QUOTE_FLAGS: &[&str] = &[
    "--read-only",
    "--name",
    "edge",
    "--subnet-id",
    "sub-00000000-0000-0000-0000-000000000003",
];

#[tokio::test]
async fn endpoint_quote_cli_keeps_the_outer_joined_interruption_diagnostic() {
    let stderr =
        interrupt_project_discovery(TOKEN, "403 Forbidden", "quote-create-endpoint", QUOTE_FLAGS)
            .await;
    assert_eq!(stderr, concat!(
        r#"{"error":{"code":"Forbidden","message":"network.QuoteCreateEndpoint: endpoint response withheld (status 403)\nnetwork.QuoteCreateEndpoint: context canceled","status":403,"operation":"network.QuoteCreateEndpoint"}}"#,
        "\n",
    ).as_bytes());
}

#[tokio::test]
async fn endpoint_quote_cli_screens_the_rebuilt_context_reason() {
    let bearer = "canceled";
    let stderr = interrupt_project_discovery(
        bearer,
        "403 Forbidden",
        "quote-create-endpoint",
        QUOTE_FLAGS,
    )
    .await;
    assert!(!String::from_utf8_lossy(&stderr).contains(bearer));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["status"], 403);
    assert_eq!(error["error"]["code"], "Forbidden");
}

#[tokio::test]
async fn endpoint_quote_cli_screens_context_only_after_an_interrupted_200() {
    let bearer = "canceled";
    let stderr =
        interrupt_project_discovery(bearer, "200 OK", "quote-create-endpoint", QUOTE_FLAGS).await;
    assert!(!String::from_utf8_lossy(&stderr).contains(bearer));
    let value: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(value["error"]["code"], "RequestFailed");
    assert!(value["error"].get("status").is_none());
}

#[tokio::test]
async fn endpoint_write_cli_names_the_write_in_both_preflight_interruption_causes() {
    for (command, flags, operation) in [
        (
            "create-endpoint",
            &[
                "--yes",
                "--name",
                "edge",
                "--max-price",
                "1",
                "--subnet-id",
                "sub-00000000-0000-0000-0000-000000000003",
            ][..],
            "network.CreateEndpoint",
        ),
        (
            "delete-endpoint",
            &[
                "--yes",
                "--endpoint-id",
                "enp-00000000-0000-0000-0000-0000000000c1",
            ][..],
            "network.DeleteEndpoint",
        ),
    ] {
        let stderr = interrupt_project_discovery(TOKEN, "403 Forbidden", command, flags).await;
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["operation"], operation);
        assert_eq!(
            error["error"]["message"],
            format!(
                "{operation}: endpoint response withheld (status 403)\n{operation}: context canceled"
            )
        );
    }
}
