pub mod dispatch;
pub mod errors;
mod gendocs;
pub mod input;
pub mod op;
mod query;
pub mod render;
mod root;

pub use root::{command_with_registry, run};

pub struct Invocation<'a> {
    pub args: Vec<std::ffi::OsString>,
    pub source: &'a dyn greennode_sdk::ConfigSource,
    pub stdin: &'a mut dyn std::io::Read,
    pub stdout: &'a mut dyn std::io::Write,
    pub stderr: &'a mut dyn std::io::Write,
    pub stdin_terminal: bool,
    pub stdout_terminal: bool,
    pub stderr_terminal: bool,
    pub clock: &'a dyn greennode_sdk::Clock,
    pub context: &'a greennode_sdk::RequestContext,
}

pub trait InvocationHandler {
    fn accepts(&self, args: &[std::ffi::OsString]) -> bool;
    fn run<'a>(
        &'a mut self,
        invocation: &'a mut Invocation<'_>,
    ) -> dispatch::FutureResult<'a, std::process::ExitCode>;
}

pub async fn run_registered(
    mut invocation: Invocation<'_>,
    handlers: &mut [&mut dyn InvocationHandler],
) -> std::process::ExitCode {
    for handler in handlers {
        if handler.accepts(&invocation.args) {
            return handler.run(&mut invocation).await;
        }
    }
    root::run(invocation.args, invocation.stdout, invocation.stderr)
}

pub async fn run_invocation(invocation: Invocation<'_>) -> std::process::ExitCode {
    run_registered(invocation, &mut []).await
}
