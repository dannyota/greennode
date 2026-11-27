use super::super::super::Screen;
use super::super::super::output;
use super::super::state::{Attempt, Goal, order_display, screen_quota};
use super::super::synthetic_settlement::Action;
use crate::monitor::{Quota, QuotaTimestamp, ResponseField};
use crate::wire::product::evidence::Evidence;
use crate::{ApiError, Error, ErrorClass};
use serde::{Serialize, Serializer, ser::SerializeStruct};

output! {
    /// Fixed, structured error information for a quota write.
    struct QuotaOutcomeError {
        /// Outcome code.
        code:String => "Code",
        /// SDK operation that handled the write.
        operation:String => "Operation",
        /// Observed HTTP status. Zero indicates no observed status.
        status:ResponseField<i64> => "Status",
    }
}
output! {
    /// Transmission and settlement evidence from a synthetic quota write.
    struct QuotaMutationOutcome {
        /// Whether the request was sent: no, yes, or unknown.
        sent:String => "Sent",
        /// Stage reached by the attempt.
        stage:String => "Stage",
        /// Fixed error information when the attempt failed.
        error:Option<QuotaOutcomeError> => "Error",
    }
}

macro_rules! synthetic_output {
    ($(#[$meta:meta])* $name:ident { $($(#[$field_meta:meta])* $field:ident : $type:ty => $json:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone,Debug)]
        pub struct $name {
            /// Transmission and settlement evidence.
            pub outcome:QuotaMutationOutcome,
            $($(#[$field_meta])* pub $field:$type,)*
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut output = serializer.serialize_struct(stringify!($name), 3 $(+ { let _ = $json; 1 })*)?;
                output.serialize_field("Sent", &self.outcome.sent)?;
                output.serialize_field("Stage", &self.outcome.stage)?;
                output.serialize_field("Error", &self.outcome.error)?;
                $(output.serialize_field($json, &self.$field)?;)*
                output.end()
            }
        }
        impl Screen for $name {
            fn screen(&mut self,evidence:&Evidence)->bool {
                self.outcome.screen(evidence) && $(self.$field.screen(evidence) &&)* true
            }
        }
    };
}

synthetic_output!(
    /// Outcome of ordering a new synthetic quota.
    CreateSyntheticQuotaOutput {
        /// Quota confirmed by a read.
        quota:Option<Quota> => "Quota",
        /// Whether the read matched the new quota.
        created:bool => "Created",
        /// Order reference when supplied by the service.
        order_id:String => "OrderID",
        /// Whether billing reports a pending payment.
        payment_pending:bool => "PaymentPending",
        /// When the write may have been sent.
        sent_at:Option<ResponseField<QuotaTimestamp>> => "SentAt",
    }
);
synthetic_output!(
    /// Outcome of renewing an active synthetic quota.
    RenewSyntheticQuotaOutput {
        /// Expiry observed before renewal.
        previous_end_time:Option<ResponseField<QuotaTimestamp>> => "PreviousEndTime",
        /// Quota confirmed by a read.
        quota:Option<Quota> => "Quota",
        /// Order reference when supplied by the service.
        order_id:String => "OrderID",
        /// Whether billing reports a pending payment.
        payment_pending:bool => "PaymentPending",
        /// Whether a read confirmed a later expiry.
        renewed:bool => "Renewed",
    }
);
synthetic_output!(
    /// Outcome of recovering an expired synthetic quota from trash.
    RecoverSyntheticQuotaOutput {
        /// Expiry observed before recovery.
        previous_end_time:Option<ResponseField<QuotaTimestamp>> => "PreviousEndTime",
        /// Quota confirmed by a read.
        quota:Option<Quota> => "Quota",
        /// Order reference when supplied by the service.
        order_id:String => "OrderID",
        /// Whether billing reports a pending payment.
        payment_pending:bool => "PaymentPending",
        /// Whether a read confirmed the recovery.
        recovered:bool => "Recovered",
    }
);
synthetic_output!(
    /// Outcome of selecting and performing synthetic renewal or recovery.
    EnsureSyntheticQuotaOutput {
        /// Selected action, or none when no action was taken.
        action:String => "Action",
        /// Quota confirmed by a read.
        quota:Option<Quota> => "Quota",
        /// Expiry observed before the attempt.
        previous_end_time:Option<ResponseField<QuotaTimestamp>> => "PreviousEndTime",
        /// Whether a read confirmed renewal.
        renewed:bool => "Renewed",
        /// Whether a read confirmed recovery.
        recovered:bool => "Recovered",
        /// Order reference when supplied by the service.
        order_id:String => "OrderID",
        /// Whether billing reports a pending payment.
        payment_pending:bool => "PaymentPending",
    }
);

/// Identity guard and price cap for synthetic quota recovery.
#[derive(Clone, Debug)]
pub struct RecoverSyntheticQuotaInput {
    /// Quota ID that must match the read.
    pub expected_quota_id: String,
    /// Maximum quote in VND.
    pub max_price: f64,
    /// Skips settlement after acceptance.
    pub no_wait: bool,
}
impl RecoverSyntheticQuotaInput {
    /// Supplies the required quota identity guard.
    pub fn new(expected_quota_id: impl Into<String>) -> Self {
        Self {
            expected_quota_id: expected_quota_id.into(),
            max_price: 0.0,
            no_wait: false,
        }
    }
}

/// Options for automatically choosing renewal or recovery.
#[derive(Clone, Debug, Default)]
pub struct EnsureSyntheticQuotaInput {
    /// Optional quota ID that must match the read.
    pub expected_quota_id: String,
    /// Maximum quote in VND.
    pub max_price: f64,
    /// Days remaining to renew. Zero allows early renewal.
    pub max_days_left: i64,
    /// Skips settlement after acceptance.
    pub no_wait: bool,
}

pub(super) struct Draft {
    pub(super) sent: &'static str,
    pub(super) stage: &'static str,
    pub(super) status: u16,
    pub(super) action: Action,
    pub(super) quota: Option<Quota>,
    pub(super) order_id: String,
    pub(super) payment_pending: bool,
    pub(super) sent_at: Option<QuotaTimestamp>,
    pub(super) previous: Option<QuotaTimestamp>,
}
impl Default for Draft {
    fn default() -> Self {
        Self {
            sent: "no",
            stage: "preflight",
            status: 0,
            action: Action::None,
            quota: None,
            order_id: String::new(),
            payment_pending: false,
            sent_at: None,
            previous: None,
        }
    }
}
impl Draft {
    pub(super) fn retained(attempt: &Attempt) -> Self {
        let (sent_at, action) = match &attempt.goal {
            Goal::SyntheticCreate { sent_at, .. } => (Some(*sent_at), Action::None),
            Goal::SyntheticChange { action, .. } if attempt.op == super::ENSURE => (None, *action),
            _ => (None, Action::None),
        };
        let previous = attempt
            .goal
            .before()
            .and_then(super::super::state::Snapshot::end)
            .map(QuotaTimestamp)
            .or_else(|| {
                matches!(attempt.goal, Goal::SyntheticCreate { .. }).then(QuotaTimestamp::default)
            });
        let (sent, status) = match attempt.goal {
            Goal::RenewSetting { .. } => ("unknown", 0),
            _ => (attempt.sent(), attempt.observed_status),
        };
        Self {
            sent,
            stage: "unconfirmed",
            status,
            action,
            quota: None,
            order_id: attempt.order_id.clone(),
            payment_pending: attempt.payment_pending,
            sent_at: sent_at.filter(|_| attempt.evidence.can_retain_partial()),
            previous: previous.filter(|_| attempt.evidence.can_retain_partial()),
        }
    }
    pub(super) fn project<T>(
        &mut self,
        op: &'static str,
        error: Option<&Error>,
        evidence: &Evidence,
        make: fn(Draft, QuotaMutationOutcome) -> T,
    ) -> T {
        screen_quota(&mut self.quota, evidence);
        self.order_id = order_display(&self.order_id, evidence);
        let outcome = QuotaMutationOutcome {
            sent: self.sent.into(),
            stage: self.stage.into(),
            error: error.map(|error| outcome_error(op, error, self.status)),
        };
        make(std::mem::take(self), outcome)
    }
    pub(super) fn create(self, outcome: QuotaMutationOutcome) -> CreateSyntheticQuotaOutput {
        CreateSyntheticQuotaOutput {
            outcome,
            quota: self.quota,
            created: self.stage == "confirmed",
            order_id: self.order_id,
            payment_pending: self.payment_pending,
            sent_at: self.sent_at.map(ResponseField::Value),
        }
    }
    pub(super) fn renew(self, outcome: QuotaMutationOutcome) -> RenewSyntheticQuotaOutput {
        RenewSyntheticQuotaOutput {
            outcome,
            previous_end_time: self.previous.map(ResponseField::Value),
            quota: self.quota,
            order_id: self.order_id,
            payment_pending: self.payment_pending,
            renewed: self.stage == "confirmed",
        }
    }
    pub(super) fn recover(self, outcome: QuotaMutationOutcome) -> RecoverSyntheticQuotaOutput {
        RecoverSyntheticQuotaOutput {
            outcome,
            previous_end_time: self.previous.map(ResponseField::Value),
            quota: self.quota,
            order_id: self.order_id,
            payment_pending: self.payment_pending,
            recovered: self.stage == "confirmed",
        }
    }
    pub(super) fn ensure(self, outcome: QuotaMutationOutcome) -> EnsureSyntheticQuotaOutput {
        EnsureSyntheticQuotaOutput {
            outcome,
            action: self.action.name().into(),
            quota: self.quota,
            previous_end_time: self.previous.map(ResponseField::Value),
            renewed: self.stage == "confirmed" && self.action == Action::Renew,
            recovered: self.stage == "confirmed" && self.action == Action::Recover,
            order_id: self.order_id,
            payment_pending: self.payment_pending,
        }
    }
}
fn outcome_error(op: &'static str, error: &Error, observed: u16) -> QuotaOutcomeError {
    let api = error.api_error();
    let status = if observed == 0 {
        api.and_then(ApiError::status).unwrap_or_default()
    } else {
        observed
    };
    let has = |class| error.has_class(class);
    let (code, status) = if has(ErrorClass::NotSettled) {
        ("NotSettled", status)
    } else if error.detail::<crate::monitor::ErrorDetail>()
        == Some(&crate::monitor::ErrorDetail::QuotaChanged)
    {
        ("QuotaChanged", 0)
    } else if has(ErrorClass::Unpriced) {
        ("Unpriced", 0)
    } else if has(ErrorClass::PriceAboveMax) {
        ("PriceAboveMax", 0)
    } else if has(ErrorClass::NoCredentials) || has(ErrorClass::CredentialsFile) {
        ("NoCredentials", status)
    } else if has(ErrorClass::InvalidConfig) {
        ("InvalidConfig", status)
    } else if error.detail::<crate::monitor::ErrorDetail>()
        == Some(&crate::monitor::ErrorDetail::UnexpectedStatus)
    {
        ("UnexpectedStatus", status)
    } else if has(ErrorClass::InvalidInput) && api.is_none() {
        ("InvalidUsage", status)
    } else if has(ErrorClass::TokenCache) {
        ("TokenCacheFailed", status)
    } else if let Some(login) = error.service_account_login_error() {
        (
            "ServiceAccountLoginFailed",
            if status == 0 {
                login.status().unwrap_or_default()
            } else {
                status
            },
        )
    } else {
        (
            api.map(ApiError::code)
                .filter(|code| !code.is_empty())
                .unwrap_or("RequestFailed"),
            status,
        )
    };
    QuotaOutcomeError {
        code: code.into(),
        operation: op.into(),
        status: ResponseField::Value(i64::from(status)),
    }
}
