use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
#[cfg(not(windows))]
use std::io::Read;
use std::io::{self, IsTerminal};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use greennode_sdk::{ConfigSource, RequestContext, SystemClock};
#[cfg(unix)]
use rustix::io::retry_on_intr;
#[cfg(unix)]
use rustix::termios::{
    LocalModes, OptionalActions, QueueSelector, Termios, tcflush, tcgetattr, tcsetattr,
};

struct Environment(HashMap<OsString, OsString>);

impl ConfigSource for Environment {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        self.0.get(name).cloned()
    }
}

#[cfg(not(windows))]
struct Input<'a> {
    context: &'a RequestContext,
    stdin: &'a io::Stdin,
    #[cfg(unix)]
    cancellation: UnixStream,
}

#[cfg(not(windows))]
impl Input<'_> {
    fn check(&self) -> io::Result<()> {
        if self.context.check(&SystemClock).is_some() {
            return Err(io::Error::other("stdin read cancelled"));
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Read for Input<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        use rustix::event::{PollFd, PollFlags, poll};
        let mut fds = [
            PollFd::new(self.stdin, PollFlags::IN),
            PollFd::new(&self.cancellation, PollFlags::IN),
        ];
        loop {
            self.check()?;
            match poll(&mut fds, None) {
                Ok(_) => {
                    self.check()?;
                    return Ok(rustix::io::read(self.stdin, bytes)?);
                }
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[cfg(unix)]
struct StdinTerminal<'a> {
    stdin: &'a io::Stdin,
    original: Option<Termios>,
}

#[cfg(unix)]
impl greennode::cli::Terminal for StdinTerminal<'_> {
    fn disable_echo(&mut self) -> io::Result<()> {
        let original = retry_on_intr(|| tcgetattr(self.stdin))?;
        let mut mode = original.clone();
        mode.local_modes
            .remove(LocalModes::ECHO | LocalModes::ECHONL);
        self.original = Some(original);
        retry_on_intr(|| tcsetattr(self.stdin, OptionalActions::Now, &mode))?;
        Ok(())
    }

    fn restore_echo(&mut self, discard_input: bool) -> io::Result<()> {
        if let Some(original) = &self.original {
            let discard = if discard_input {
                retry_on_intr(|| tcflush(self.stdin, QueueSelector::IFlush))
            } else {
                Ok(())
            };
            retry_on_intr(|| tcsetattr(self.stdin, OptionalActions::Now, original))?;
            self.original = None;
            discard?;
        }
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
impl Read for Input<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.stdin.read(bytes)
    }
}

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
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
        {
            use std::future::{Future, poll_fn};
            use std::task::Poll;
            let mut interrupt = Box::pin(tokio::signal::ctrl_c());
            let initial = poll_fn(|cx| match interrupt.as_mut().poll(cx) {
                Poll::Ready(result) => Poll::Ready(Some(result)),
                Poll::Pending => Poll::Ready(None),
            })
            .await;
            match initial {
                Some(Ok(())) => cancel.cancel(),
                Some(Err(_)) => return ExitCode::FAILURE,
                None => {
                    tokio::spawn(async move {
                        if interrupt.await.is_ok() {
                            cancel.cancel();
                        }
                    });
                }
            }
        }
        tokio::task::yield_now().await;
        let source = Environment(std::env::vars_os().collect());
        let stdin = io::stdin();
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        let stdin_terminal = stdin.is_terminal();
        #[cfg(not(windows))]
        let mut input = Input {
            context: &context,
            stdin: &stdin,
            #[cfg(unix)]
            cancellation: {
                let Ok((reader, writer)) = UnixStream::pair() else {
                    return ExitCode::FAILURE;
                };
                let context = context.clone();
                tokio::spawn(async move {
                    context.wait(&SystemClock).await;
                    drop(writer);
                });
                reader
            },
        };
        #[cfg(windows)]
        let mut input = stdin;
        let stdout_terminal = stdout.is_terminal();
        let stderr_terminal = stderr.is_terminal();
        let invocation = greennode::cli::Invocation {
            args: std::env::args_os().collect(),
            source: &source,
            config_options: greennode_sdk::ConfigOptions::default(),
            stdin: &mut input,
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal,
            stdout_terminal,
            stderr_terminal,
            clock: &SystemClock,
            context: &context,
        };
        #[cfg(unix)]
        {
            let mut terminal = StdinTerminal {
                stdin: &stdin,
                original: None,
            };
            greennode::cli::run_invocation_with_terminal(invocation, &mut terminal).await
        }
        #[cfg(not(unix))]
        greennode::cli::run_invocation(invocation).await
    })
}
