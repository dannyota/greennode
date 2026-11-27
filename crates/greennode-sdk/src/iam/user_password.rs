use std::fmt;

use zeroize::Zeroizing;

use super::user_writes::{CreateUserInput, ResetUserPasswordInput};
use super::wire::REDACTED;
use super::writes::not_sent;
use super::{Client, invalid_input};
use crate::wire::product::evidence::Evidence;
use crate::{ContextEnd, Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, Secret};

const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@";

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
pub(super) fn validate_password<O>(
    op: &str,
    supplied: Option<&Secret>,
    length: i64,
) -> Result<usize, Failure<O>> {
    let invalid = |why| super::writes::guard_failed(invalid_input(format!("{op}: {why}")));
    if let Some(password) = supplied {
        if length != 0 {
            return Err(invalid("PasswordLength requires generation"));
        }
        let value = password.expose_secret();
        if value.is_empty() || value.len() > 4096 {
            return Err(invalid(
                "password must be nonempty valid UTF-8 and at most 4096 bytes",
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(invalid("password must not contain controls"));
        }
        return Ok(0);
    }
    match length {
        0 => Ok(32),
        8..=50 => usize::try_from(length).map_err(|_| invalid("invalid password length")),
        _ => Err(invalid("password length must be between 8 and 50")),
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    pub(super) fn prepare_password<O>(
        &self,
        ctx: &RequestContext,
        supplied: Option<Secret>,
        length: usize,
    ) -> Result<(Secret, bool), Failure<O>> {
        if let Some(password) = supplied {
            return Ok((password, false));
        }
        let entropy_failed = |message| {
            not_sent(Error::new(ErrorClass::Api).with_detail(RecoveryAdvice::new(message)))
        };
        let mut candidate = Zeroizing::new(String::with_capacity(length));
        let mut entropy = Zeroizing::new([0_u8; 64]);
        let mut attempts = 0;
        for _ in 0..1024 {
            if let Some(end) = ctx.check(self.session.clock()) {
                return Err(not_sent(Error::new(match end {
                    ContextEnd::Cancelled => ErrorClass::Cancelled,
                    ContextEnd::DeadlineExceeded => ErrorClass::DeadlineExceeded,
                })));
            }
            getrandom::fill(entropy.as_mut())
                .map_err(|_| entropy_failed("iam: password entropy unavailable"))?;
            for byte in entropy.iter().copied().filter(|byte| *byte < 252) {
                let Some(character) = ALPHABET.get(usize::from(byte) % ALPHABET.len()).copied()
                else {
                    return Err(entropy_failed("iam: password entropy unavailable"));
                };
                candidate.push(char::from(character));
                if candidate.len() == length {
                    if candidate.bytes().any(|b| b.is_ascii_uppercase())
                        && candidate.bytes().any(|b| b.is_ascii_lowercase())
                        && candidate.bytes().any(|b| b.is_ascii_digit())
                        && candidate.contains('@')
                    {
                        return Ok((Secret::new(std::mem::take(&mut *candidate)), true));
                    }
                    zeroize::Zeroize::zeroize(&mut *candidate);
                    attempts += 1;
                    if attempts == 256 {
                        return Err(entropy_failed("iam: password candidate budget exhausted"));
                    }
                }
            }
        }
        Err(entropy_failed("iam: password entropy budget exhausted"))
    }
}

crate::error::debug_record!(CreateUserInput => |self, formatter| {
    debug_password_input(
        formatter,
        "CreateUserInput",
        ("username", &self.username),
        self.password.as_ref(),
        self.password_length,
    )
});

crate::error::debug_record!(ResetUserPasswordInput => |self, formatter| {
    debug_password_input(
        formatter,
        "ResetUserPasswordInput",
        ("user_id", &self.user_id),
        self.password.as_ref(),
        self.password_length,
    )
});

fn debug_password_input(
    formatter: &mut fmt::Formatter<'_>,
    name: &str,
    (label, operand): (&str, &str),
    password: Option<&Secret>,
    length: i64,
) -> fmt::Result {
    let mut evidence = Evidence::new();
    if let Some(password) = password {
        evidence.add_redaction(password);
    }
    let operand = if evidence.id_reflects(operand) {
        REDACTED
    } else {
        operand
    };
    let length: &dyn fmt::Debug = if evidence.id_reflects(&length.to_string()) {
        &REDACTED
    } else {
        &length
    };
    crate::error::write_debug(formatter, |formatter| {
        formatter
            .debug_struct(name)
            .field(label, &operand)
            .field("password", &password)
            .field("password_length", length)
            .finish()
    })
}
