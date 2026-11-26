use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::IsTerminal;
use std::process::ExitCode;

use greennode_sdk::{ConfigSource, RequestContext, SystemClock};

struct Environment(HashMap<OsString, OsString>);

impl ConfigSource for Environment {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        self.0.get(name).cloned()
    }
}

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::FAILURE,
    };
    runtime.block_on(async {
        let (context, cancel) = RequestContext::new();
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut interrupt), Ok(mut terminate)) = (
                signal(SignalKind::interrupt()),
                signal(SignalKind::terminate()),
            ) else {
                return ExitCode::FAILURE;
            };
            let interrupt_cancel = cancel.clone();
            tokio::spawn(async move {
                interrupt.recv().await;
                interrupt_cancel.cancel();
            });
            tokio::spawn(async move {
                terminate.recv().await;
                cancel.cancel();
            });
        }
        #[cfg(not(unix))]
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancel.cancel();
            }
        });
        let source = Environment(std::env::vars_os().collect());
        let mut stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        let stdin_terminal = stdin.is_terminal();
        let stdout_terminal = stdout.is_terminal();
        let stderr_terminal = stderr.is_terminal();
        greennode::cli::run_invocation(greennode::cli::Invocation {
            args: std::env::args_os().collect(),
            source: &source,
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal,
            stdout_terminal,
            stderr_terminal,
            clock: &SystemClock,
            context: &context,
        })
        .await
    })
}
