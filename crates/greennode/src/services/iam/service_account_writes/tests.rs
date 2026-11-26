use std::path::PathBuf;

use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, EndpointOverrides};

use super::*;

struct PrivateDir(PathBuf);

impl PrivateDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("greennode-iam-{}", getrandom::u64().unwrap()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn reset_file_failure_withholds_account_id_reflecting_issued_secret() {
    for (id, reflected) in [("acctYT9iY2Qsuffix", true), ("sa-1", false)] {
        let server = Server::start(
            [
                r#"{"userId":"user-1","userType":"iam-user"}"#,
                r#"[{"action":"CreatePolicy","label":"Write"}]"#,
                r#"{"data":[],"totalItems":0}"#,
                r#"{"clientSecret":"a?bcd"}"#,
            ]
            .map(|body| Response::Http {
                status: "200 OK".into(),
                headers: vec![("Content-Length".into(), body.len().to_string())],
                body: body.as_bytes().to_vec(),
            })
            .into(),
        );
        let options = ConfigOptions::default()
            .with_region("han-1".into())
            .with_static_token(Secret::new("toy/bearer"))
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some(format!("{}/dashboard/", server.origin)),
                iam: Some(format!("{}/iam/", server.origin)),
                ..Default::default()
            });
        let config = Config::new(with_product_loopback(Product::Iam, options, &server)).unwrap();
        let directory = PrivateDir::new();
        let file = directory.0.join("rotated");
        let input = Input {
            id: id.into(),
            path: file.to_str().unwrap().into(),
            ..Input::default()
        };
        check_destination(&input, &ArgMatches::default()).unwrap();
        std::fs::write(&file, b"existing").unwrap();
        let (ctx, _) = RequestContext::new();
        let Err(ExecutionFailure::Local(error)) = reset(&ctx, &config, &input).await else {
            panic!("reset should report the private file failure");
        };
        assert_eq!(error.code, "SecretFileFailed");
        assert!(
            error
                .message
                .contains("run reset-service-account-secret again")
        );
        assert_eq!(error.message.contains(id), !reflected);
        assert_eq!(error.message.contains("<redacted>"), reflected);
        assert_eq!(std::fs::read(file).unwrap(), b"existing");
        let heads = server.heads();
        assert_eq!(heads.len(), 4);
        assert!(heads[1].starts_with(b"GET /iam/policies-api/v1/actions?"));
        assert!(heads[3].starts_with(b"POST /dashboard/accounts-api/v1/service-accounts/"));
    }
}
