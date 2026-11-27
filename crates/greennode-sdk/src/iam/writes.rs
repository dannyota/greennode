use std::sync::Arc;
use std::task::Poll;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde_json::Value;
use tokio::sync::{Mutex, MutexGuard};

use super::wire::{Body, ObservedResponse, Wire};
use super::{Api, Client, DEFAULT_CAP, OBJECT_CAP, read_operation, require_identity, screen_error};
use crate::error::RecoveryAdvice;
use crate::wire::product::{
    evidence::{Evidence, SentCredentials},
    operation::{self, Operation},
    options::{AttemptCheck, Options, PasswordSlot},
    redirect::{Classification, Retry},
};
use crate::{Error, ErrorClass, Failure, RequestContext, Secret, WriteDispatch};

/// IAM-specific write outcome, available through [`Error::detail`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WriteError {
    /// The target could be the current caller.
    SelfChange,
    /// The caller or target cannot be proved safe for a guarded write.
    PrivilegedChange,
    /// The create returned but could not be confirmed.
    CreateUnconfirmed,
    /// The service acknowledged a write without returning its one-time credential.
    NoSecret,
    /// The requested user could not be verified as an IAM user.
    OriginNotIam,
    /// A user creation may have reached the service without proving ownership.
    UserCreateUncertain,
    /// A user administration write may have reached the service.
    UserWriteUncertain,
    /// The policy is managed by GreenNode.
    ManagedPolicy,
    /// A resource still has an attachment that prevents deletion.
    ResourceInUse,
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    pub(super) fn write_evidence<O>(
        &self,
        ctx: &RequestContext,
        op: &str,
        ids: &[(&str, &str)],
        fields: &[&str],
    ) -> Result<Evidence, Failure<O>> {
        super::validate_ids::<O>(op, ids).map_err(guard_failed)?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|e| not_sent(*e))?;
        let mut evidence = Evidence::new();
        if let Some(configured) = runtime.configured_credentials {
            evidence.absorb(configured);
        }
        if ids.iter().any(|(_, id)| evidence.id_reflects(id))
            || fields.iter().any(|field| evidence.text_reflects(field))
        {
            return Err(withheld_operand());
        }
        Ok(evidence)
    }

    pub(super) async fn write<T: Wire, O>(
        &self,
        ctx: &RequestContext,
        api: Api,
        operation: &Operation,
        path: &[&str],
        body: Option<Value>,
        evidence: &mut Evidence,
    ) -> Result<T, Failure<O>> {
        let body = match body {
            Some(body) => {
                if evidence.output_reflects(&body) {
                    return Err(withheld_operand());
                }
                serde_json::to_vec(&body)
                    .map(Bytes::from)
                    .map_err(|_| not_sent(Error::new(ErrorClass::InvalidInput)))?
            }
            None => Bytes::new(),
        };
        self.send_write(
            ctx,
            api,
            operation,
            path,
            (body, Options::DEFAULT),
            evidence,
        )
        .await
    }

    pub(super) async fn write_user_password<T: Wire, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        (username, password): (Option<&str>, &Secret),
        evidence: &mut Evidence,
    ) -> Result<T, Failure<O>> {
        #[derive(serde::Serialize)]
        struct UserPassword<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            username: Option<&'a str>,
            password: &'a str,
        }

        let body = serde_json::to_vec(&UserPassword {
            username,
            password: password.expose_secret(),
        })
        .map(Bytes::from)
        .map_err(|_| not_sent(Error::new(ErrorClass::InvalidInput)))?;
        evidence.add_operation_password(password);
        let options = Options {
            password_slot: Some(PasswordSlot::IamUser(password)),
            ..Options::DEFAULT
        };
        self.send_write(
            ctx,
            Api::Accounts,
            operation,
            path,
            (body, options),
            evidence,
        )
        .await
    }

    async fn send_write<T: Wire, O>(
        &self,
        ctx: &RequestContext,
        api: Api,
        operation: &Operation,
        path: &[&str],
        (body, options): (Bytes, Options<'_>),
        evidence: &mut Evidence,
    ) -> Result<T, Failure<O>> {
        let session = &self.session;
        let endpoints = session.endpoints();
        let base = match api {
            Api::Accounts => endpoints.dashboard.as_ref(),
            Api::Policies => endpoints.iam.as_ref(),
        }
        .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|e| not_sent(*e))?;
        let checks = evidence.cached_checks().to_vec();
        let attempt =
            move |credentials: &SentCredentials| checks.iter().find_map(|check| check(credentials));
        let options = Options {
            attempt_check: Some(AttemptCheck(&attempt)),
            ..options
        };
        operation::execute_with_evidence::<Body<T>>(
            runtime,
            base,
            operation,
            &options,
            path,
            &[],
            session.bearer_source(),
            body,
            evidence,
        )
        .await
        .map(|(_, Body(value))| value)
        .map_err(|failed| {
            let (error, _, dispatch) = failed.into_parts();
            let observed = ObservedResponse::status(&error).map(ObservedResponse::from_status);
            let error = super::wire::private_status_error(operation, error, evidence);
            let error = screen_error(error, evidence);
            let error = match observed {
                Some(outcome) => error.with_outer_detail(outcome),
                None => error,
            };
            Failure::new(error, None, dispatch)
        })
    }

    pub(super) async fn require_classified_caller<O>(
        &self,
        ctx: &RequestContext,
        op: &str,
        evidence: &mut Evidence,
    ) -> Result<super::GetCallerIdentityOutput, Failure<O>> {
        let mut slot = self.lock_bounded(&self.guard.caller, ctx).await?;
        if let Some(caller) = slot.as_ref() {
            check_cached_caller(caller, evidence)?;
            keep_caller_check(caller, evidence);
            return self.classified(caller.clone(), op);
        }
        let fetched: super::GetCallerIdentityOutput = self
            .fetch::<_, O>(
                ctx,
                Api::Accounts,
                &Operation {
                    sensitive: matches!(
                        op,
                        "iam.UpdateServiceAccount"
                            | "iam.DeleteServiceAccount"
                            | "iam.ResetServiceAccountSecret"
                            | "iam.DeleteUser"
                            | "iam.ResetUserPassword"
                            | "iam.EnableUserMFA"
                            | "iam.DisableUserMFA"
                    ),
                    ..read_operation(
                        "iam.GetCallerIdentity",
                        "accounts-api/v1/auth/userinfo",
                        OBJECT_CAP,
                        false,
                    )
                },
                &[],
                &[],
                evidence,
            )
            .await
            .map_err(guard_failed)?;
        if caller_reflected(&fetched, evidence) {
            return Err(guard_refusal(
                "iam: iam.GetCallerIdentity: unsafe caller identity",
                None,
            ));
        }
        let fetched = self.classified(fetched, op)?;
        keep_caller_check(&fetched, evidence);
        *slot = Some(fetched.clone());
        drop(slot);
        Ok(fetched)
    }

    pub(super) async fn lock_bounded<'a, T, O>(
        &self,
        mutex: &'a Mutex<T>,
        ctx: &RequestContext,
    ) -> Result<MutexGuard<'a, T>, Failure<O>> {
        let clock = self.session.clock();
        if let Some(end) = ctx.check(clock) {
            return Err(not_sent(end.into()));
        }
        let mut end = Box::pin(ctx.wait(clock));
        let mut lock = Box::pin(mutex.lock());
        std::future::poll_fn(|cx| {
            if let Poll::Ready(end) = end.as_mut().poll(cx) {
                return Poll::Ready(Err(not_sent(end.into())));
            }
            lock.as_mut().poll(cx).map(Ok)
        })
        .await
    }

    fn classified<O>(
        &self,
        caller: super::GetCallerIdentityOutput,
        op: &str,
    ) -> Result<super::GetCallerIdentityOutput, Failure<O>> {
        if !matches!(
            caller.user_type.as_str(),
            "iam-user" | "user-sa" | "service-sa"
        ) {
            return Err(guard_refusal(
                &format!(
                    "iam: refused: target is protected: {op}: the caller's user type is not recognized"
                ),
                Some(WriteError::PrivilegedChange),
            ));
        }
        Ok(caller)
    }
}

pub(super) fn write_operation(
    id: &'static str,
    method: Method,
    template: &'static str,
    retry: Retry,
    success: &'static [StatusCode],
) -> Operation {
    Operation {
        method,
        class: Classification::Write,
        retry,
        success,
        allow_recovery: !matches!(retry, Retry::Once),
        no_output: success == [StatusCode::NO_CONTENT],
        no_redirect: true,
        ..read_operation(id, template, DEFAULT_CAP, true)
    }
}

pub(super) fn guard_failed<O>(failed: Failure<O>) -> Failure<O> {
    not_sent(failed.into_parts().0)
}

pub(super) fn not_sent<O>(error: Error) -> Failure<O> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) fn withheld_operand<O>() -> Failure<O> {
    not_sent(
        super::invalid_input::<O>("IAM request operand withheld".into())
            .into_parts()
            .0,
    )
}

pub(super) fn guard_refusal<O>(message: &str, detail: Option<WriteError>) -> Failure<O> {
    let error = Error::new(ErrorClass::Api).with_outer_detail(RecoveryAdvice::new(message));
    not_sent(match detail {
        Some(detail) => error.with_outer_detail(detail),
        None => error,
    })
}

pub(super) fn settle<O>(
    failed: Failure<O>,
    partial: Option<O>,
    message: &str,
    evidence: &Evidence,
) -> Failure<O> {
    let cause = failed.into_parts().0;
    unsettled(cause, partial, evidence, |cause| {
        format!(
            "{message}: vngcloud: write outcome not confirmed: {}",
            error_reason(cause)
        )
    })
}

pub(super) fn unsettled<O>(
    cause: Error,
    partial: Option<O>,
    evidence: &Evidence,
    advice: impl FnOnce(&Error) -> String,
) -> Failure<O> {
    const UNCONFIRMED: &str = "vngcloud: write outcome not confirmed";
    let partial = partial.filter(|_| evidence.can_retain_partial());
    let advice = partial
        .is_some()
        .then(|| advice(&cause))
        .filter(|message| !evidence.text_reflects(message));
    let error = Error::new(ErrorClass::NotSettled)
        .with_cause(cause)
        .with_outer_detail(RecoveryAdvice::new(
            advice.as_deref().unwrap_or(UNCONFIRMED),
        ));
    Failure::new(error, partial, WriteDispatch::MayHaveBeenSent)
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
pub(super) fn confirmed_identity<O>(
    actual: &str,
    expected: &str,
    message: &'static str,
    evidence: &Evidence,
) -> Result<(), Failure<O>> {
    if evidence.id_reflects(actual) || evidence.id_reflects(expected) {
        return Err(guard_refusal(message, None));
    }
    require_identity(actual, expected, message)
}

pub(super) fn error_reason(error: &Error) -> String {
    if let Some(advice) = error.detail::<RecoveryAdvice>() {
        return advice.message().to_owned();
    }
    let Some(api) = error.api_error() else {
        return if error.has_identity(ErrorClass::Cancelled) {
            "context canceled"
        } else if error.has_identity(ErrorClass::DeadlineExceeded) {
            "context deadline exceeded"
        } else {
            "request failed"
        }
        .to_owned();
    };
    let mut reason = String::new();
    let operation = api.operation();
    let has_operation = api
        .message()
        .strip_prefix(operation)
        .is_some_and(|rest| rest.starts_with(": "));
    if !operation.is_empty() && !has_operation {
        reason.push_str(operation);
        reason.push_str(": ");
    }
    reason.push_str(if api.message().is_empty() {
        "request failed"
    } else {
        api.message()
    });
    if let Some(status) = api.status() {
        use std::fmt::Write;
        let _ = write!(reason, " (status {status})");
    }
    reason
}

fn caller_reflected(caller: &super::GetCallerIdentityOutput, evidence: &Evidence) -> bool {
    evidence.id_reflects(&caller.user_id)
        || evidence.id_reflects(&caller.account_id.to_string())
        || [&caller.user_type, &caller.username]
            .into_iter()
            .any(|v| evidence.text_reflects(v))
        || evidence.output_reflects(&Value::from(caller.account_id))
}

fn unsafe_cached_error(op: &str) -> Error {
    Error::new(ErrorClass::Api).with_outer_detail(RecoveryAdvice::new(&format!(
        "iam: {op}: unsafe cached guard response"
    )))
}

fn unsafe_cached<O>(op: &str) -> Failure<O> {
    not_sent(unsafe_cached_error(op))
}

fn keep_caller_check(caller: &super::GetCallerIdentityOutput, evidence: &mut Evidence) {
    let caller = caller.clone();
    evidence.keep_cached_check(Arc::new(move |credentials| {
        let mut probe = Evidence::new();
        probe.absorb(credentials);
        caller_reflected(&caller, &probe)
            .then(|| Box::new(unsafe_cached_error("iam.GetCallerIdentity")))
    }));
}

pub(super) fn keep_actions_check(names: &[String], evidence: &mut Evidence) {
    let names = names.to_vec();
    evidence.keep_cached_check(Arc::new(move |credentials| {
        let mut probe = Evidence::new();
        probe.absorb(credentials);
        serde_json::to_value(&names)
            .map_or(true, |value| probe.output_reflects(&value))
            .then(|| Box::new(unsafe_cached_error("iam.ListActions")))
    }));
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
pub(super) fn check_cached_caller<O>(
    caller: &super::GetCallerIdentityOutput,
    evidence: &Evidence,
) -> Result<(), Failure<O>> {
    if caller_reflected(caller, evidence) {
        return Err(unsafe_cached("iam.GetCallerIdentity"));
    }
    Ok(())
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
pub(super) fn check_cached_actions<O>(
    names: &[String],
    evidence: &Evidence,
) -> Result<(), Failure<O>> {
    if serde_json::to_value(names).is_ok_and(|value| !evidence.output_reflects(&value)) {
        Ok(())
    } else {
        Err(unsafe_cached("iam.ListActions"))
    }
}
