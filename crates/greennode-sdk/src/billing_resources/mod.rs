mod credentials;
mod decode;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::value::RawValue;

use crate::config::Session;
use crate::error::SafeText;
use crate::model::wire::{JsonSource, go_json};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{AttemptCheck, Options},
    redirect::{Classification, Retry},
    response, retry,
};
use crate::{
    ApiError, Error, ErrorClass, Failure, LoginError, RequestContext, Secret,
    ServiceAccountLoginError, WriteDispatch,
};

pub use decode::{ListResourcesOutput, Resource, ResourceBillingElement, ResourceThresholds};

const LIST: &str = "billing.ListResources";
const ACCOUNT: &str = "billingresources.GetAccount";
const WITHHELD: &str = "billing response withheld";
const INVALID: &str = "billing response had invalid structure";
const CAP: usize = 4 << 20;

/// Validated resource rows and their public billing projection.
#[derive(Debug)]
pub(crate) struct ResourceList {
    projection: ListResourcesOutput,
    renew_periods: Vec<RenewPeriod>,
}

/// The provider's field presence and value for `renewPeriod`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenewPeriod {
    Omitted,
    Null,
    Value(i64),
}

impl ResourceList {
    /// Consumes the validated list and returns the public billing projection.
    pub(crate) fn into_projection(self) -> ListResourcesOutput {
        self.projection
    }

    /// Rechecks retained rows with the current credential evidence and pairs them with renewal periods.
    #[allow(dead_code, reason = "Used by the NAT auto-renew port")]
    pub(crate) fn rows<'a>(
        &'a self,
        evidence: &'a Evidence,
    ) -> Result<impl Iterator<Item = (&'a Resource, RenewPeriod)> + 'a, Box<Error>> {
        if !evidence.can_retain_partial()
            || credentials::resources_reflect(&self.projection, evidence)
        {
            return Err(invalid(
                &operation(LIST, "gateway/api/v1/resources", Method::GET),
                evidence,
            ));
        }
        Ok(self
            .projection
            .items
            .iter()
            .zip(self.renew_periods.iter().copied()))
    }
}

/// Reads the unfiltered, account-wide billing resource inventory.
pub(crate) async fn list(
    session: &Session,
    ctx: &RequestContext,
    evidence: &mut Evidence,
) -> Result<ResourceList, Box<Error>> {
    let op = operation(LIST, "gateway/api/v1/resources", Method::GET);
    let body = exchange(session, ctx, &op, &Options::DEFAULT, Bytes::new(), evidence)
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
    let source = JsonSource::new(&body);
    let data = decode_data(&op, source.text(), evidence)?;
    let mut output = decode::list(data).map_err(|()| invalid(&op, evidence))?;
    if credentials::resources_reflect(&output.projection, evidence) {
        return Err(invalid(&op, evidence));
    }
    if source.has_replacements() {
        decode::restore_raw_fields(&source, data, &mut output.projection)
            .map_err(|()| invalid(&op, evidence))?;
    }
    Ok(output)
}

/// Validated billing account identity for the auto-renew header.
#[allow(dead_code, reason = "Used by the storage and NAT auto-renew ports")]
pub(crate) struct AccountId(Secret);

/// Resolves a positive integral account ID for the current operation.
#[allow(dead_code, reason = "Used by the storage and NAT auto-renew ports")]
pub(crate) async fn account(
    session: &Session,
    ctx: &RequestContext,
    evidence: &mut Evidence,
) -> Result<AccountId, Box<Error>> {
    let op = operation(ACCOUNT, "gateway/api/v1/home/user-info", Method::GET);
    let body = exchange(session, ctx, &op, &Options::DEFAULT, Bytes::new(), evidence)
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
    let text = go_json(&body);
    let data = decode_data(&op, &text, evidence)?;
    let id = decode::account(data).map_err(|()| invalid(&op, evidence))?;
    if evidence.id_reflects(&id) {
        return Err(invalid(&op, evidence));
    }
    Ok(AccountId(Secret::new(id)))
}

/// Identifies one resource and its requested renewal setting.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "Used by the storage and NAT auto-renew ports")]
pub(crate) struct Setting<'a> {
    pub(crate) product: &'a str,
    pub(crate) artifact_type: &'a str,
    pub(crate) artifact_id: &'a str,
    pub(crate) channel: i64,
    pub(crate) auto_renew_info: AutoRenewInfo,
}

/// The provider's enabled flag and renewal period in minutes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoRenewInfo {
    pub(crate) is_enable: bool,
    pub(crate) period: i64,
}

/// Sends one auto-renew setting without retry or redirect and retains dispatch evidence.
#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
#[allow(dead_code, reason = "Used by the storage and NAT auto-renew ports")]
pub(crate) async fn put(
    session: &Session,
    ctx: &RequestContext,
    operation_id: &'static str,
    account: &AccountId,
    setting: Setting<'_>,
    evidence: &mut Evidence,
) -> Result<(), Failure<()>> {
    let op = operation(
        operation_id,
        "gateway/api/v1/resources/autoRenew",
        Method::PUT,
    );
    let artifact_id = setting.artifact_id;
    let check = |credentials: &crate::wire::product::evidence::SentCredentials| {
        let mut current = Evidence::new();
        current.absorb(credentials);
        (current.id_reflects(account.0.expose_secret()) || current.id_reflects(artifact_id))
            .then(|| Box::new(Error::new(ErrorClass::InvalidInput)))
    };
    let body = serde_json::to_vec(&[setting])
        .map(Bytes::from)
        .map_err(|_| {
            Failure::new(
                Error::new(ErrorClass::InvalidInput),
                None,
                WriteDispatch::NotSent,
            )
        })?;
    let options = Options {
        headers: &[("portal-user-id", account.0.expose_secret())],
        attempt_check: Some(AttemptCheck(&check)),
        ..Options::DEFAULT
    };
    let mut account_redaction = Evidence::new();
    account_redaction.add_redaction(&account.0);
    let body = exchange(session, ctx, &op, &options, body, evidence)
        .await
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(
                screen_account_error(error, &account_redaction),
                None,
                dispatch,
            )
        })?;
    let text = go_json(&body);
    let rejected = |error: Error| {
        Failure::new(
            screen_account_error(error, &account_redaction),
            None,
            WriteDispatch::MayHaveBeenSent,
        )
    };
    let data = decode_data(&op, &text, evidence).map_err(|error| rejected(*error))?;
    if let Some(count) = decode::renewal(data).map_err(|()| rejected(*invalid(&op, evidence)))? {
        let message = format!("server refused auto-renew setting; error count {count}");
        return Err(rejected(api(
            &op,
            Some(StatusCode::OK),
            "AutoRenewRejected",
            &message,
            false,
            evidence,
        )));
    }
    Ok(())
}

fn screen_account_error(error: Error, account_redaction: &Evidence) -> Error {
    let reflects = |text: &str| account_redaction.text_reflects(text);
    error.screen_api(&reflects).screen_sign_in(&reflects)
}

fn decode_data<'a>(
    op: &Operation,
    text: &'a str,
    evidence: &Evidence,
) -> Result<&'a RawValue, Box<Error>> {
    if text.is_empty() {
        return Err(invalid(op, evidence));
    }
    let raw: &RawValue = serde_json::from_str(text).map_err(|_| {
        Box::new(api(
            op,
            Some(StatusCode::OK),
            "InvalidResponse",
            WITHHELD,
            false,
            evidence,
        ))
    })?;
    let data = decode::envelope(raw).map_err(|()| invalid(op, evidence))?;
    if credentials::reflected(raw, evidence) {
        return Err(invalid(op, evidence));
    }
    Ok(data)
}

fn operation(id: &'static str, template: &'static str, method: Method) -> Operation {
    let write = method == Method::PUT;
    Operation {
        id,
        template,
        method,
        class: if write {
            Classification::Write
        } else {
            Classification::Read
        },
        retry: if write {
            Retry::Once
        } else {
            Retry::Repeatable
        },
        success: &[StatusCode::OK],
        cap: CAP,
        allow_recovery: !write,
        allow_empty: true,
        no_output: false,
        keep_status: true,
        overflow_overrides_status: false,
        sensitive: true,
        reject_reflected_credentials: false,
        no_redirect: true,
        retry_config: retry::Config::default(),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
async fn exchange(
    session: &Session,
    ctx: &RequestContext,
    op: &Operation,
    options: &Options<'_>,
    body: Bytes,
    evidence: &mut Evidence,
) -> Result<Bytes, Failure<()>> {
    let dispatch = if op.method == Method::PUT {
        WriteDispatch::NotSent
    } else {
        WriteDispatch::NotAWrite
    };
    let base = session
        .endpoints()
        .billing
        .as_ref()
        .ok_or_else(|| Failure::new(Error::new(ErrorClass::InvalidConfig), None, dispatch))?;
    let mut jitter = || session.retry_jitter();
    let runtime = session
        .runtime(ctx, &mut jitter)
        .map_err(|error| Failure::new(*error, None, dispatch))?;
    let result = operation::execute_raw(
        runtime,
        base,
        op,
        options,
        &[],
        &[],
        RouteAuth::Bearer(session.bearer_source()),
        body,
        evidence,
    )
    .await;
    let reply = result.map_err(|failure| {
        let (error, _, dispatch) = failure.into_parts();
        Failure::new(withheld_error(op, error, evidence), None, dispatch)
    })?;
    if reply.status != StatusCode::OK {
        let dispatch = if op.method == Method::PUT {
            WriteDispatch::MayHaveBeenSent
        } else {
            WriteDispatch::NotAWrite
        };
        return Err(Failure::new(
            if reply.status.is_success() {
                api(
                    op,
                    Some(reply.status),
                    "InvalidResponse",
                    WITHHELD,
                    false,
                    evidence,
                )
            } else {
                status_error(op, reply.status, None, evidence)
            },
            None,
            dispatch,
        ));
    }
    Ok(Bytes::copy_from_slice(&reply.body))
}

fn invalid(op: &Operation, evidence: &Evidence) -> Box<Error> {
    Box::new(api(
        op,
        Some(StatusCode::OK),
        "InvalidResponse",
        INVALID,
        false,
        evidence,
    ))
}

fn api(
    op: &Operation,
    status: Option<StatusCode>,
    code: &str,
    message: &str,
    retryable: bool,
    evidence: &Evidence,
) -> Error {
    Error::from_api(ApiError::without_fallback(
        op.id,
        status.map(|status| status.as_u16()),
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
    .screen_api(&|text| evidence.text_reflects(text))
}

fn status_error(
    op: &Operation,
    status: StatusCode,
    retry_hint: Option<bool>,
    evidence: &Evidence,
) -> Error {
    let classified = response::classify(
        op.id,
        status,
        Bytes::new(),
        &[],
        (true, true, op.retry),
        evidence.credentials(),
    )
    .err();
    let detail = classified.as_deref().and_then(Error::api_error);
    api(
        op,
        Some(status),
        detail.map_or("RequestFailed", ApiError::code),
        WITHHELD,
        retry_hint.unwrap_or_else(|| detail.is_some_and(ApiError::retryable)),
        evidence,
    )
}

fn withheld_error(op: &Operation, error: Error, evidence: &Evidence) -> Error {
    let api_error = error.api_error();
    let sign_in = screened_sign_in(&error, evidence);
    let observed = error.detail::<StatusCode>().copied();
    let status = observed.or_else(|| StatusCode::from_u16(api_error?.status()?).ok());
    let overflow = error.is_body_too_large();
    let class = error.class();
    let mut safe = if status.is_some_and(|status| status.is_success()) || overflow {
        api(op, status, "InvalidResponse", WITHHELD, false, evidence)
    } else if let Some(status) = status {
        status_error(op, status, api_error.map(ApiError::retryable), evidence)
    } else if let Some(detail) = api_error {
        api(op, None, "", WITHHELD, detail.retryable(), evidence)
    } else {
        return sign_in.unwrap_or_else(|| error.screen_api(&|text| evidence.text_reflects(text)));
    };
    if overflow {
        safe = safe.with_body_too_large();
    }
    if let Some(status) = observed
        && !evidence.text_reflects(status.as_str())
    {
        safe = safe.with_outer_detail(status);
    }
    if let Some(sign_in) = sign_in {
        safe = safe.with_cause(sign_in);
    }
    if matches!(
        class,
        ErrorClass::Cancelled | ErrorClass::DeadlineExceeded | ErrorClass::Auth
    ) {
        Error::new(class).with_cause(safe)
    } else {
        safe
    }
}

fn screened_sign_in(error: &Error, evidence: &Evidence) -> Option<Error> {
    let login = error.login_error();
    let service = error.service_account_login_error();
    if login.is_none() && service.is_none() {
        return None;
    }
    let reason = |text: &'static str| {
        if evidence.text_reflects(text) {
            ""
        } else {
            text
        }
    };
    let status =
        |value: Option<u16>| value.filter(|value| !evidence.text_reflects(&value.to_string()));
    let mut screened = Error::new(error.class());
    if let Some(login) = login {
        screened = screened.with_login(LoginError::new(
            reason(login.reason()),
            status(login.status()),
            login.captcha_suspected(),
        ));
    }
    if let Some(service) = service {
        screened = screened.with_service_login(ServiceAccountLoginError::new(
            reason(service.reason()),
            status(service.status()),
        ));
    }
    Some(screened)
}

#[cfg(test)]
mod tests;
