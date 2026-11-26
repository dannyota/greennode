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
    use std::process::ExitCode;

    let mut handler = configure::Handler(terminal);
    if handler.accepts(&invocation.args) {
        return handler.run(&mut invocation).await;
    }

    let registry = match crate::services::registered() {
        Ok(registry) => registry,
        Err(error) => {
            let _ = writeln!(invocation.stderr, "invalid command registry: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    let matches = match root::parse(
        std::mem::take(&mut invocation.args),
        invocation.stdout,
        invocation.stderr,
        &registry,
    ) {
        Ok(matches) => matches,
        Err(exit) => return exit,
    };
    if let Some(exit) = root::run_local(&matches, invocation.stdout, invocation.stderr, &registry) {
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
