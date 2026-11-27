use super::evidence::{Evidence, SentCredentials};
use crate::{Error, ErrorClass, Failure, WriteDispatch};
use serde::Serialize;
use std::{cell::RefCell, future::Future};

#[derive(Clone, Default)]
struct Observed {
    public: SentCredentials,
    transport: SentCredentials,
    write: Option<WriteDispatch>,
}

tokio::task_local! {
    static OBSERVED: RefCell<Observed>;
}

pub(crate) async fn scope<T>(future: impl Future<Output = T>) -> T {
    if OBSERVED.try_with(|_| ()).is_ok() {
        future.await
    } else {
        OBSERVED.scope(RefCell::default(), future).await
    }
}

pub(super) fn record(public: &SentCredentials, transport: &SentCredentials, write: WriteDispatch) {
    let _ = OBSERVED.try_with(|state| {
        let mut state = state.borrow_mut();
        state.public.extend(public);
        state.transport.extend(transport);
        match (state.write, write) {
            (Some(WriteDispatch::MayHaveBeenSent), _) | (_, WriteDispatch::NotAWrite) => {}
            (_, observed) => state.write = Some(observed),
        }
    });
}

pub(crate) fn observe_signin(credentials: &SentCredentials) {
    record(credentials, credentials, WriteDispatch::NotAWrite);
}

pub(crate) fn authenticate_error(error: Error, configured: &SentCredentials) -> Error {
    let mut evidence = Evidence::from_credentials(configured);
    let _ = OBSERVED.try_with(|state| evidence.absorb(&state.borrow().public));
    evidence.screen_error(error)
}

pub(super) fn record_secret(secret: &crate::Secret) {
    let _ = OBSERVED.try_with(|state| state.borrow_mut().public.record(secret));
}

pub(crate) fn bytes_reflect(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && OBSERVED
            .try_with(|state| {
                super::response::BodyMatcher::for_output(&state.borrow().public, bytes.len())
                    .reflects(bytes, false)
            })
            .unwrap_or(false)
}

pub(crate) fn screen_text(text: &str) -> &str {
    if OBSERVED
        .try_with(|state| state.borrow().public.text_reflects(text))
        .unwrap_or(false)
    {
        ""
    } else {
        text
    }
}

pub(crate) fn private_safe(secret: &crate::Secret) -> bool {
    OBSERVED
        .try_with(|state| {
            !state
                .borrow()
                .transport
                .text_reflects(secret.expose_secret())
        })
        .unwrap_or(true)
}

macro_rules! finish {
    ($future:expr) => {
        $crate::wire::product::exit::verify($future)
            .await
            .map_err(|failure| *failure)
    };
}

pub(crate) use finish;

pub(crate) async fn verify<T: Serialize>(
    future: impl Future<Output = Result<T, Failure<T>>>,
) -> Result<T, Box<Failure<T>>> {
    scope(async {
        let result = future.await;
        let state = OBSERVED.with(|state| state.borrow().clone());
        if state.public.values().is_empty() {
            return result.map_err(Box::new);
        }
        let evidence = Evidence::from_credentials(&state.public);
        let reflected = |output: &T| evidence.serialized_output_reflects(output);
        match result {
            Ok(output) if reflected(&output) => Err(Box::new(Failure::new(
                evidence.screen_error(Error::new(ErrorClass::Api)),
                None,
                state.write.unwrap_or(WriteDispatch::NotAWrite),
            ))),
            Ok(output) => Ok(output),
            Err(failure) => Err(Box::new(failure.screen_exit(
                |error| evidence.screen_error(error),
                reflected,
                |secret| state.transport.text_reflects(secret.expose_secret()),
            ))),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_product_methods_have_a_screened_exit() {
        fn check(path: &std::path::Path) {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let file = entry.path();
                if file.is_dir() {
                    if !matches!(
                        file.file_name().and_then(std::ffi::OsStr::to_str),
                        Some("config" | "wire")
                    ) {
                        check(&file);
                    }
                    continue;
                }
                if file.extension().and_then(std::ffi::OsStr::to_str) != Some("rs")
                    || file.file_name().and_then(std::ffi::OsStr::to_str) == Some("context.rs")
                {
                    continue;
                }
                let source = std::fs::read_to_string(&file).unwrap();
                for (offset, _) in source.match_indices("pub async fn ") {
                    let rest = &source[offset + "pub async fn ".len()..];
                    let (signature, body) = rest.split_once('{').unwrap();
                    if signature.starts_with("discard_after_delivery_failure(") {
                        continue;
                    }
                    assert!(
                        body.trim_start()
                            .starts_with("crate::wire::product::exit::finish!(async {"),
                        "{}: {signature}",
                        file.display()
                    );
                }
            }
        }
        check(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    }

    #[tokio::test]
    async fn output_screening_refuses_when_its_work_budget_is_exhausted() {
        let result = finish!(async {
            let mut credentials = SentCredentials::default();
            credentials.record(&crate::Secret::new("z".repeat(4096)));
            record(&credentials, &credentials, WriteDispatch::NotAWrite);
            Ok(vec!["ok"; 5000])
        });
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn joined_futures_contribute_evidence_before_finishing() {
        let result = finish!(async {
            let (value, ()) = tokio::join!(async { "toy-final" }, async {
                let mut credentials = SentCredentials::default();
                credentials.record(&crate::Secret::new("toy-final"));
                record(&credentials, &credentials, WriteDispatch::NotAWrite);
            });
            Ok(value)
        });
        assert!(result.is_err());
    }
}
