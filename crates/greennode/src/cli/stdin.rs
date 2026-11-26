use std::io::{self, Read};
use std::sync::mpsc;
use tokio::time::Instant;
use zeroize::Zeroizing;

const READ_LIMIT: usize = 4097;

pub trait Stdin: Read {
    fn read_before(&mut self, _: &mut [u8], _: Instant) -> io::Result<usize> {
        Err(io::ErrorKind::TimedOut.into())
    }
}

impl Stdin for io::Empty {
    fn read_before(&mut self, bytes: &mut [u8], _: Instant) -> io::Result<usize> {
        self.read(bytes)
    }
}

impl Stdin for &[u8] {
    fn read_before(&mut self, bytes: &mut [u8], _: Instant) -> io::Result<usize> {
        self.read(bytes)
    }
}

impl Stdin for &mut dyn Read {}

pub struct BlockingStdin<R> {
    reader: Option<R>,
    eof: bool,
}

impl<R> BlockingStdin<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: Some(reader),
            eof: false,
        }
    }
}

impl<R: Read> Read for BlockingStdin<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.reader
            .as_mut()
            .ok_or(io::ErrorKind::TimedOut)?
            .read(bytes)
    }
}

impl<R: Read + Send + 'static> Stdin for BlockingStdin<R> {
    fn read_before(&mut self, bytes: &mut [u8], deadline: Instant) -> io::Result<usize> {
        if bytes.is_empty() || self.eof {
            return Ok(0);
        }
        let deadline = deadline.into_std();
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut reader = self.reader.take().ok_or(io::ErrorKind::TimedOut)?;
        let (send, receive) = mpsc::sync_channel(1);
        let limit = bytes.len().min(READ_LIMIT);
        std::thread::Builder::new().spawn(move || {
            let mut data = Zeroizing::new([0; READ_LIMIT]);
            let result = read_until(&mut reader, &mut data[..limit], deadline);
            let _ = send.send((reader, data, result));
        })?;
        let (reader, data, result) = receive
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => io::ErrorKind::TimedOut,
                mpsc::RecvTimeoutError::Disconnected => io::ErrorKind::BrokenPipe,
            })?;
        self.reader = Some(reader);
        let count = result?;
        self.eof = count < limit;
        bytes[..count].copy_from_slice(&data[..count]);
        Ok(count)
    }
}

fn read_until(
    reader: &mut dyn Read,
    bytes: &mut [u8],
    deadline: std::time::Instant,
) -> io::Result<usize> {
    let mut count = 0;
    while let Some(remaining) = bytes.get_mut(count..).filter(|bytes| !bytes.is_empty()) {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match reader.read(remaining) {
            Ok(0) => break,
            Ok(size) if size <= remaining.len() => count += size,
            Ok(_) => return Err(io::ErrorKind::InvalidData.into()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(count)
}
