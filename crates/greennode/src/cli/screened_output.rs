use greennode_sdk::RequestContext;
use std::{
    cell::Cell,
    io::{self, Write},
};

const OUTPUT_CAP: usize = 64 << 20;
const WITHHELD: &[u8] =
    b"{\"error\":{\"code\":\"OutputWithheld\",\"message\":\"Command output withheld. Check the operation outcome before retrying.\"}}\n";

pub(super) struct Budget {
    remaining: Cell<usize>,
    exhausted: Cell<bool>,
}

impl Budget {
    pub(super) const fn new() -> Self {
        Self {
            remaining: Cell::new(OUTPUT_CAP),
            exhausted: Cell::new(false),
        }
    }
}

pub(super) struct Stream<'a> {
    bytes: Vec<u8>,
    budget: &'a Budget,
}

impl<'a> Stream<'a> {
    pub(super) fn new(budget: &'a Budget) -> Self {
        Self {
            bytes: Vec::new(),
            budget,
        }
    }
}

impl Write for Stream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(remaining) = self
            .budget
            .remaining
            .get()
            .checked_sub(bytes.len())
            .filter(|_| !self.budget.exhausted.get())
        else {
            self.budget.exhausted.set(true);
            return Err(io::Error::other("command output exceeds limit"));
        };
        self.budget.remaining.set(remaining);
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn publish(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    output: Stream<'_>,
    diagnostics: Stream<'_>,
    status: u8,
) -> u8 {
    if output.budget.exhausted.get()
        || diagnostics.budget.exhausted.get()
        || RequestContext::response_bytes_reflect(&output.bytes)
        || RequestContext::response_bytes_reflect(&diagnostics.bytes)
    {
        if !RequestContext::response_bytes_reflect(WITHHELD) {
            let _ = stderr.write_all(WITHHELD);
        }
        return 1;
    }
    let output = stdout.write_all(&output.bytes);
    let diagnostics = stderr.write_all(&diagnostics.bytes);
    if output.is_err() || diagnostics.is_err() {
        return 1;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_output_limit_withholds_both_streams() {
        let budget = Budget {
            remaining: Cell::new(12),
            exhausted: Cell::new(false),
        };
        let (mut output, mut diagnostics) = (Stream::new(&budget), Stream::new(&budget));
        output.write_all(b"some output").unwrap();
        assert!(diagnostics.write_all(b"too much").is_err());
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        assert_eq!(publish(&mut stdout, &mut stderr, output, diagnostics, 0), 1);
        assert!(stdout.is_empty());
        assert_eq!(stderr, WITHHELD);
    }
}
