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

pub(crate) fn observe_sent(credentials: &SentCredentials) {
    record(credentials, credentials, WriteDispatch::NotAWrite);
}

#[cfg(test)]
pub(crate) fn recorded_exactly(value: &str) -> bool {
    OBSERVED.with(|state| {
        state
            .borrow()
            .public
            .values()
            .iter()
            .any(|secret| secret.expose_secret() == value)
    })
}

pub(crate) fn authenticate_error(error: Error, configured: &SentCredentials) -> Error {
    let mut evidence = Evidence::from_credentials(configured);
    let _ = OBSERVED.try_with(|state| evidence.absorb(&state.borrow().public));
    evidence.screen_public_error(error)
}

pub(super) fn observe_operation_password(slot: super::options::PasswordSlot<'_>) {
    let (super::options::PasswordSlot::DatabaseUser(password)
    | super::options::PasswordSlot::IamUser(password)) = slot;
    record_secret(password);
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
        let result = match result {
            Ok(output) if !reflected(&output) => return Ok(output),
            Ok(_) => Failure::new(
                Error::new(ErrorClass::Api),
                None,
                state.write.unwrap_or(WriteDispatch::NotAWrite),
            ),
            Err(failure) => failure,
        };
        Err(Box::new(result.screen_exit(
            |error| evidence.screen_public_error(error),
            reflected,
            |secret| state.transport.text_reflects(secret.expose_secret()),
            |text| evidence.text_reflects(text),
        )))
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
        let credentials = super::super::response::budget_test_credentials();
        let field = format!("!{}", "a".repeat(255));
        let clean = finish!(async {
            record(&credentials, &credentials, WriteDispatch::NotAWrite);
            Ok(vec![field.clone()])
        });
        assert!(clean.is_ok());
        let exhausted = finish!(async {
            record(&credentials, &credentials, WriteDispatch::NotAWrite);
            Ok(vec![field; 1024])
        });
        assert!(exhausted.is_err());
    }

    #[tokio::test]
    async fn error_exit_checks_display_and_final_diagnostic_values() {
        use crate::error::SafeText;
        let credentials = SentCredentials::for_test(
            [
                "greennode: Cancelled",
                "403)",
                "billing.CreateBudget",
                "context canceled",
            ]
            .into_iter()
            .map(crate::Secret::new)
            .collect(),
        );
        let failure = finish!(async {
            record(&credentials, &credentials, WriteDispatch::NotSent);
            let cause = Error::from_api(crate::ApiError::new(
                "billing.CreateBudget",
                Some(403),
                SafeText::new("Forbidden", &[]),
                SafeText::new("permission denied (403)", &[]),
                false,
            ));
            let error = Error::from(crate::ContextEnd::Cancelled)
                .with_detail(crate::RecoveryAdvice::new(
                    "lookup failed: context canceled",
                ))
                .with_cause(cause);
            Err::<(), _>(Failure::new(error, None, WriteDispatch::NotSent))
        })
        .unwrap_err();
        let error = failure.error();
        assert!(error.to_string() == "request failed");
        assert!(error.has_identity(ErrorClass::Cancelled));
        assert!(error.has_class(ErrorClass::PermissionDenied));
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        let api = error.api_error().unwrap();
        assert!(api.operation().is_empty());
        assert!(api.message().is_empty());
        assert_eq!(api.status(), Some(403));
        assert!(
            error
                .detail::<crate::RecoveryAdvice>()
                .unwrap()
                .message()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn error_exit_withholds_a_reflecting_display_fallback() {
        let credentials = SentCredentials::for_test(
            ["greennode: Api", "request failed"]
                .into_iter()
                .map(crate::Secret::new)
                .collect(),
        );
        let failure = finish!(async {
            record(&credentials, &credentials, WriteDispatch::NotAWrite);
            Err::<(), _>(Failure::new(
                Error::new(ErrorClass::Api),
                None,
                WriteDispatch::NotAWrite,
            ))
        })
        .unwrap_err();
        assert!(failure.error().to_string().is_empty());
    }

    #[tokio::test]
    async fn failure_debug_and_typed_arithmetic_detail_do_not_reflect() {
        let credentials = SentCredentials::for_test(
            [
                "class: EstimateOverflow",
                "write_dispatch: NotSent",
                "multiplication",
                "toy-item",
            ]
            .into_iter()
            .map(crate::Secret::new)
            .collect(),
        );
        let failure = finish!(async {
            record(&credentials, &credentials, WriteDispatch::NotSent);
            Err::<(), _>(Failure::new(
                Error::new(ErrorClass::EstimateOverflow).with_detail(
                    crate::pricing::EstimateArithmeticError {
                        stage: "multiplication",
                        item_key: "toy-item".into(),
                    },
                ),
                None,
                WriteDispatch::NotSent,
            ))
        })
        .unwrap_err();
        let error = failure.error();
        assert!(error.has_identity(ErrorClass::EstimateOverflow));
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        for rendered in [
            format!("{error:?}"),
            format!("{error:#?}"),
            format!("{failure:?}"),
            format!("{failure:#?}"),
        ] {
            assert!(!credentials.text_reflects(&rendered));
        }
        if let Some(detail) = error.detail::<crate::pricing::EstimateArithmeticError>() {
            assert!(!credentials.text_reflects(detail.stage));
            assert!(!credentials.text_reflects(&detail.item_key));
        }
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
