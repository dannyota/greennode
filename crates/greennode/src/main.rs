use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use greennode_sdk::{ConfigSource, RequestContext, SystemClock};

struct Environment(HashMap<OsString, OsString>);

impl ConfigSource for Environment {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        self.0.get(name).cloned()
    }
}

struct Input<'a> {
    context: &'a RequestContext,
    #[cfg(not(windows))]
    stdin: io::Stdin,
    #[cfg(windows)]
    read_complete: bool,
}

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
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let mut fds = [PollFd::new(&self.stdin, PollFlags::IN)];
        let wait = Timespec {
            tv_sec: 0,
            tv_nsec: 25_000_000,
        };
        loop {
            self.check()?;
            if poll(&mut fds, Some(&wait))? != 0 {
                self.check()?;
                return Ok(rustix::io::read(&self.stdin, bytes)?);
            }
        }
    }
}

#[cfg(windows)]
impl Read for Input<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        use std::sync::mpsc::{RecvTimeoutError, sync_channel};
        use std::time::Duration;
        use zeroize::Zeroizing;

        self.check()?;
        if self.read_complete || bytes.is_empty() {
            return Ok(0);
        }
        self.read_complete = true;
        let limit = bytes.len();
        let (send, receive) = sync_channel(1);
        std::thread::Builder::new().spawn(move || {
            let read = || -> io::Result<_> {
                let mut stdin = same_file::Handle::stdin()?;
                let mut data = Zeroizing::new(vec![0; limit]);
                let mut used = 0;
                while let Some(rest) = data.get_mut(used..).filter(|rest| !rest.is_empty()) {
                    match stdin.as_file_mut().read(rest) {
                        Ok(0) => break,
                        Ok(n) => used += n,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok((data, used))
            };
            let _ = send.send(read());
        })?;
        loop {
            self.check()?;
            match receive.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => {
                    let (data, used) = result?;
                    bytes[..used].copy_from_slice(&data[..used]);
                    return Ok(used);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("stdin reader stopped"));
                }
            }
        }
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
        let mut stdin = Input {
            context: &context,
            #[cfg(not(windows))]
            stdin,
            #[cfg(windows)]
            read_complete: false,
        };
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
