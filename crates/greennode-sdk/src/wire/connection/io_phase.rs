use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use std::{
    fmt, io,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wire) enum IoPhase {
    Read,
    Write,
}

impl fmt::Display for IoPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
        })
    }
}

impl std::error::Error for IoPhase {}

impl IoPhase {
    fn mark(self, error: io::Error) -> io::Error {
        io::Error::new(error.kind(), self)
    }
}

#[derive(Debug)]
pub(crate) struct PhaseIo<T>(pub(super) T);

impl<T: Connection> Connection for PhaseIo<T> {
    fn connected(&self) -> Connected {
        self.0.connected()
    }
}

impl<T: Read + Unpin> Read for PhaseIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0)
            .poll_read(cx, buf)
            .map_err(|error| IoPhase::Read.mark(error))
    }
}

impl<T: Write + Unpin> Write for PhaseIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0)
            .poll_write(cx, buf)
            .map_err(|error| IoPhase::Write.mark(error))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0)
            .poll_flush(cx)
            .map_err(|error| IoPhase::Write.mark(error))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0)
            .poll_shutdown(cx)
            .map_err(|error| IoPhase::Write.mark(error))
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0)
            .poll_write_vectored(cx, bufs)
            .map_err(|error| IoPhase::Write.mark(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper_util::{client::legacy::Client, rt::TokioExecutor};
    use std::future::{Ready, ready};
    use tower_service::Service;

    struct WriteFault(usize, io::ErrorKind);

    impl Connection for WriteFault {
        fn connected(&self) -> Connected {
            Connected::new()
        }
    }

    impl Read for WriteFault {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: ReadBufCursor<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl Write for WriteFault {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.0 == 0 {
                return Poll::Ready(Err(self.1.into()));
            }
            let n = self.0.min(buf.len());
            self.0 -= n;
            Poll::Ready(Ok(n))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[derive(Clone)]
    struct FaultConnector(io::ErrorKind);

    impl Service<http::Uri> for FaultConnector {
        type Response = PhaseIo<WriteFault>;
        type Error = io::Error;
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: http::Uri) -> Self::Future {
            ready(Ok(PhaseIo(WriteFault(1024, self.0))))
        }
    }

    #[tokio::test]
    async fn request_body_write_errors_keep_the_io_phase() {
        use crate::wire::{
            product::retry::SendFailure,
            send::{self, Error},
        };
        for (kind, diagnostic) in [
            (io::ErrorKind::ConnectionReset, "write: connection reset"),
            (io::ErrorKind::BrokenPipe, "write: network request failed"),
        ] {
            let client = Client::builder(TokioExecutor::new()).build(FaultConnector(kind));
            let request = http::Request::post("https://example.test/")
                .body(Full::new(Bytes::from(vec![0; 8192])))
                .unwrap();
            let error = client.request(request).await.unwrap_err();
            let safe = send::transport_failure(&error);
            assert!(
                matches!(safe, Error::Transport(SendFailure::PossiblySent, cause) if cause.message() == diagnostic),
                "{kind:?}: {safe:?}"
            );
        }
    }
}
