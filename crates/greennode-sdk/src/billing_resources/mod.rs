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
    guard::Guarded,
    operation::{self, Operation, Reflection, RouteAuth},
    options::{AttemptCheck, Options},
    redirect::{Classification, Retry},
    response, retry,
    sender::Response,
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

pub(crate) struct ResourceList {
    projection: ListResourcesOutput,
    renew_periods: Vec<RenewPeriod>,
    receipt: Guarded<()>,
}

crate::error::debug_record!(ResourceList unit);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenewPeriod {
    Omitted,
    Null,
    Value(i64),
}

impl ResourceList {
    pub(crate) fn into_projection(self) -> ListResourcesOutput {
        self.projection
    }

    #[allow(dead_code, reason = "Used by the NAT auto-renew port")]
    pub(crate) fn rows<'a>(
        &'a self,
        evidence: &'a Evidence,
    ) -> Result<impl Iterator<Item = (&'a Resource, RenewPeriod)> + 'a, Box<Error>> {
        if !evidence.can_retain_partial()
            || self.receipt.recheck(evidence).is_err()
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

pub(crate) async fn list(
    session: &Session,
    ctx: &RequestContext,
    evidence: &mut Evidence,
) -> Result<ResourceList, Box<Error>> {
    let op = operation(LIST, "gateway/api/v1/resources", Method::GET);
    let reply = exchange(session, ctx, &op, &Options::DEFAULT, Bytes::new(), evidence)
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
    let receipt = reply
        .clean_receipt()
        .ok_or_else(|| invalid(&op, evidence))?;
    let source = JsonSource::new(receipt.body());
    let data = decode_data(&op, source.text(), evidence)?;
    let (mut projection, renew_periods) =
        decode::list(data).map_err(|()| invalid(&op, evidence))?;
    if source.has_replacements() {
        decode::restore_raw_fields(&source, data, &mut projection)
            .map_err(|()| invalid(&op, evidence))?;
    }
    if credentials::resources_reflect(&projection, evidence) {
        return Err(invalid(&op, evidence));
    }
    let receipt = if evidence.guards_reads() {
        evidence.keep_decoded_operands(op.id, &projection)?;
        Guarded::latest_direct((), evidence, op.id)?
    } else {
        receipt.guard(())
    };
    Ok(ResourceList {
        projection,
        renew_periods,
        receipt,
    })
}

#[allow(dead_code, reason = "Used by the storage and NAT auto-renew ports")]
pub(crate) struct AccountId(Secret);

#[allow(dead_code, reason = "Used by the storage and NAT auto-renew ports")]
pub(crate) async fn account(
    session: &Session,
    ctx: &RequestContext,
    evidence: &mut Evidence,
) -> Result<AccountId, Box<Error>> {
    let mut op = operation(ACCOUNT, "gateway/api/v1/home/user-info", Method::GET);
    op.reflection = Reflection::Guard;
    let reply = exchange(session, ctx, &op, &Options::DEFAULT, Bytes::new(), evidence)
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
    reply.decode_projection(|body| {
        let text = go_json(body);
        let data = decode_data(&op, &text, evidence)?;
        let id = decode::account(data).map_err(|()| invalid(&op, evidence))?;
        if evidence.id_reflects(&id) {
            return Err(invalid(&op, evidence));
        }
        Ok(AccountId(Secret::new(id)))
    })
}

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoRenewInfo {
    pub(crate) is_enable: bool,
    pub(crate) period: i64,
}

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
    let reply = exchange(session, ctx, &op, &options, body, evidence)
        .await
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(
                screen_account_error(error, &account_redaction),
                None,
                dispatch,
            )
        })?;
    let rejected = |error: Error| {
        Failure::new(
            screen_account_error(error, &account_redaction),
            None,
            WriteDispatch::MayHaveBeenSent,
        )
    };
    reply
        .decode_projection(|body| {
            let text = go_json(body);
            let data = decode_data(&op, &text, evidence)?;
            let Some(count) = decode::renewal(data).map_err(|()| invalid(&op, evidence))? else {
                return Ok(());
            };
            let message = format!("server refused auto-renew setting; error count {count}");
            Err(Box::new(api(
                &op,
                Some(StatusCode::OK),
                "AutoRenewRejected",
                &message,
                false,
                evidence,
            )))
        })
        .map_err(|error| rejected(evidence.screen_error(*error)))
}

fn screen_account_error(error: Error, account_redaction: &Evidence) -> Error {
    account_redaction.screen_error(error)
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
    decode::envelope(raw).map_err(|()| invalid(op, evidence))
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
        raw_body: false,
        overflow_overrides_status: false,
        sensitive: true,
        reflection: Reflection::Redact,
        no_redirect: true,
        report_redirect: true,
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
) -> Result<Response, Failure<()>> {
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
    let dispatch = if op.method == Method::PUT {
        WriteDispatch::MayHaveBeenSent
    } else {
        WriteDispatch::NotAWrite
    };
    if reply.status != StatusCode::OK {
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
    let numeric_credential = evidence.credentials().has_numeric_product_credential();
    if op.method == Method::PUT {
        if numeric_credential {
            let error = Error::new(ErrorClass::NotSettled).with_cause(*invalid(op, evidence));
            return Err(Failure::new(evidence.screen_error(error), None, dispatch));
        }
        return Ok(reply);
    }
    if numeric_credential || !reply.reflection.is_clean() {
        let error = if reply.malformed_json() {
            api(
                op,
                Some(StatusCode::OK),
                "InvalidResponse",
                WITHHELD,
                false,
                evidence,
            )
        } else {
            *invalid(op, evidence)
        };
        return Err(Failure::new(error, None, dispatch));
    }
    Ok(reply)
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
    if error.class() == ErrorClass::InvalidInput {
        return evidence.screen_error(error);
    }
    let api_error = error.api_error();
    let sign_in = screened_sign_in(&error, evidence);
    let observed = error.http_status();
    let status = observed.or_else(|| StatusCode::from_u16(api_error?.observed_status()?).ok());
    if status == Some(StatusCode::OK)
        && error.detail::<operation::RefusedGuard>().is_some()
        && error.detail::<operation::MalformedGuard>().is_none()
        && !error.is_body_too_large()
    {
        return *invalid(op, evidence);
    }
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
        safe = safe.with_http_status(status);
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
