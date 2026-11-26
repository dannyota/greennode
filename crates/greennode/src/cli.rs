mod configure;
pub mod dispatch;
pub mod errors;
mod gendocs;
pub mod input;
pub mod op;
mod query;
pub mod render;
mod root;

pub use root::command_with_registry;

pub struct Invocation<'a> {
    pub args: Vec<std::ffi::OsString>,
    pub source: &'a dyn greennode_sdk::ConfigSource,
    pub config_options: greennode_sdk::ConfigOptions,
    pub stdin: &'a mut dyn std::io::Read,
    pub stdout: &'a mut dyn std::io::Write,
    pub stderr: &'a mut dyn std::io::Write,
    pub stdin_terminal: bool,
    pub stdout_terminal: bool,
    pub stderr_terminal: bool,
    pub clock: &'a dyn greennode_sdk::Clock,
    pub context: &'a greennode_sdk::RequestContext,
}

pub trait Terminal {
    fn disable_echo(&mut self) -> std::io::Result<()>;
    fn restore_echo(&mut self, discard_input: bool) -> std::io::Result<()>;
}

trait InvocationHandler {
    fn accepts(&self, args: &[std::ffi::OsString]) -> bool;
    fn run<'a>(
        &'a mut self,
        invocation: &'a mut Invocation<'_>,
    ) -> dispatch::FutureResult<'a, std::process::ExitCode>;
}

pub async fn run_invocation(invocation: Invocation<'_>) -> std::process::ExitCode {
    run_with_terminal(invocation, None).await
}

#[cfg(unix)]
pub async fn run_invocation_with_terminal(
    invocation: Invocation<'_>,
    terminal: &mut dyn Terminal,
) -> std::process::ExitCode {
    run_with_terminal(invocation, Some(terminal)).await
}

async fn run_with_terminal(
    mut invocation: Invocation<'_>,
    terminal: Option<&mut dyn Terminal>,
) -> std::process::ExitCode {
    let mut handler = configure::Handler(terminal);
    if handler.accepts(&invocation.args) {
        return handler.run(&mut invocation).await;
    }

    let registry = match crate::services::registered() {
        Ok(registry) => registry,
        Err(error) => {
            let _ = writeln!(invocation.stderr, "invalid command registry: {error:?}");
            return std::process::ExitCode::FAILURE;
        }
    };
    run_registered(invocation, &registry).await
}

async fn run_registered(
    mut invocation: Invocation<'_>,
    registry: &crate::services::Registry,
) -> std::process::ExitCode {
    use std::process::ExitCode;

    let matches = match root::parse(
        std::mem::take(&mut invocation.args),
        invocation.stdout,
        invocation.stderr,
        registry,
    ) {
        Ok(matches) => matches,
        Err(exit) => return exit,
    };
    if let Some(exit) = root::run_local(&matches, invocation.stdout, invocation.stderr, registry) {
        return exit;
    }
    let Some((operation, handler, matches)) = registry.select(&matches) else {
        let exit = dispatch::local(
            invocation.stderr,
            dispatch::LocalError {
                code: "RequestFailed",
                message: "command handler unavailable".into(),
                exit: 1,
            },
        );
        return ExitCode::from(exit);
    };
    ExitCode::from((handler.execute)(operation, matches, &mut invocation).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dispatch::{ExecutionResult, FutureResult, OperationFailure};
    use errors::{Rule, RuleExit, RuleName, RuleOperation};
    use greennode_sdk::{
        Config, ConfigOptions, ConfigSource, ErrorClass, RequestContext, Secret, SystemClock,
        WriteDispatch, testing,
    };
    use input::InputDescriptor;
    use op::{Handler, Kind, Operation, Service};
    use std::{
        ffi::{OsStr, OsString},
        path::PathBuf,
    };

    struct Source;

    impl ConfigSource for Source {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }

        fn home_dir(&self) -> Option<PathBuf> {
            None
        }
    }

    fn fail<'a>(
        _: &'a RequestContext,
        _: &'a Config,
        _: &'a (),
    ) -> FutureResult<'a, ExecutionResult> {
        Box::pin(async {
            Err(OperationFailure {
                error: testing::cause(
                    testing::api("stub.Inspect", Some(404), "", "safe message"),
                    testing::identity(ErrorClass::PermissionDenied),
                ),
                partial: None,
                dispatch: WriteDispatch::NotAWrite,
                query_partial: false,
            })
        })
    }

    const INPUT: InputDescriptor<()> = InputDescriptor {
        defaults: || (),
        fields: &[],
        allow_json: false,
        allow_query: false,
        project_binding: None,
        pre_guard: None,
        guard: None,
    };

    const OPERATION: Operation = Operation {
        name: "inspect",
        sdk_operation: "stub.Inspect",
        short_help: "Inspect",
        example: None,
        presence_required_fields: &[],
        inherited_bindings: &[],
        kind: Kind::Read,
        flags: &[],
        json_fields: &[],
        extra_flags: &[],
        notes: &[],
        handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation, operation, matches, &INPUT, fail,
            ))
        })),
    };

    const NAME: RuleName = RuleName {
        rank: 18,
        code: "StubPermission",
        operation: RuleOperation::Api,
        include_api_status: true,
    };

    const RULE: Rule = Rule {
        message: None,
        identity: "stub-permission",
        matches: |error| error.has_identity(ErrorClass::PermissionDenied),
        name: Some(NAME),
        exit: None,
    };

    const EXIT_ONLY: Rule = Rule {
        name: None,
        exit: Some(RuleExit::Failure),
        ..RULE
    };

    const SERVICE: Service = Service {
        name: "stub",
        operations: &[OPERATION],
        error_rules: &[
            Rule {
                message: None,
                identity: "stub-api",
                matches: |error| error.api_error().is_some(),
                name: Some(RuleName {
                    rank: 19,
                    code: "StubApi",
                    operation: RuleOperation::Fixed("stub.Fixed"),
                    ..NAME
                }),
                exit: Some(RuleExit::Failure),
            },
            RULE,
        ],
    };

    #[tokio::test]
    async fn registered_service_rules_are_ranked_and_local_to_dispatch() {
        let registry = crate::services::Registry::new(&[
            SERVICE,
            Service {
                name: "exit-only",
                error_rules: &[EXIT_ONLY],
                ..SERVICE
            },
            Service {
                name: "plain",
                error_rules: &[],
                ..SERVICE
            },
        ])
        .unwrap();
        for (service, exit, stderr) in [
            (
                "stub",
                1,
                "{\"error\":{\"code\":\"StubPermission\",\"message\":\"safe message\",\"status\":404,\"operation\":\"stub.Inspect\"}}\n",
            ),
            (
                "plain",
                4,
                "{\"error\":{\"code\":\"NotFound\",\"message\":\"safe message\",\"status\":404,\"operation\":\"stub.Inspect\"}}\n",
            ),
            (
                "exit-only",
                1,
                "{\"error\":{\"code\":\"NotFound\",\"message\":\"safe message\",\"status\":404,\"operation\":\"stub.Inspect\"}}\n",
            ),
        ] {
            let (context, _) = RequestContext::new();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let invocation = Invocation {
                args: ["greennode", service, "inspect"].map(OsString::from).into(),
                source: &Source,
                config_options: ConfigOptions::default()
                    .with_region("hcm-3".into())
                    .with_static_token(Secret::new("toy-token")),
                stdin: &mut std::io::empty(),
                stdout: &mut out,
                stderr: &mut err,
                stdin_terminal: false,
                stdout_terminal: false,
                stderr_terminal: false,
                clock: &SystemClock,
                context: &context,
            };
            assert_eq!(
                run_registered(invocation, &registry).await,
                std::process::ExitCode::from(exit)
            );
            assert!(out.is_empty());
            assert_eq!(err, stderr.as_bytes());
        }
    }

    #[test]
    fn registry_rejects_duplicate_service_identities_and_name_ranks() {
        use crate::services::{Registry, RegistryError};

        for services in [
            &[Service {
                error_rules: &[RULE, Rule { name: None, ..RULE }],
                ..SERVICE
            }][..],
            &[Service {
                error_rules: &[
                    RULE,
                    Rule {
                        identity: "duplicate-rank",
                        ..RULE
                    },
                ],
                ..SERVICE
            }],
        ] {
            assert!(matches!(
                Registry::new(services),
                Err(RegistryError::InvalidErrorRules("stub"))
            ));
        }
    }
}
