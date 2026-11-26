use super::{parse_date, security::Redactor};
use crate::billing::model::{field, reflects_integer};
use crate::output_number;
use crate::wire::{product::sender::SentCredentials, strict_json::Shape};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use std::sync::LazyLock;

/// A wallet credit purchase, transaction code 1.
pub const TRANSACTION_PURCHASE: &str = "PURCHASE";
/// A wallet refund, transaction code 2.
pub const TRANSACTION_REFUND: &str = "REFUND";
/// A wallet charge, transaction code 5.
pub const TRANSACTION_CHARGE: &str = "CHARGE";
/// A credit to the wallet.
pub const DIRECTION_CREDIT: &str = "CREDIT";
/// A debit from the wallet.
pub const DIRECTION_DEBIT: &str = "DEBIT";
/// The wallet balance payment gateway.
pub const GATEWAY_BALANCE: &str = "use_balance";

/// A wallet ledger entry with its amount and resulting balance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", try_from = "TransactionWire")]
pub struct Transaction {
    /// Numeric wallet transaction ID.
    #[serde(rename = "ID")]
    pub id: i64,
    /// Name of a known transaction type. Unknown codes have an empty name.
    pub r#type: String,
    /// Transaction type code as reported by billing.
    pub type_code: i64,
    /// Credit or debit as reported by billing.
    pub direction: String,
    /// Transaction amount in the API's monetary basis.
    pub amount: f64,
    /// Resulting wallet balance in the API's monetary basis.
    pub balance: f64,
    /// Source reference when supplied.
    pub source_reference: Option<String>,
    /// Description when supplied.
    pub description: Option<String>,
    /// User service reference when supplied.
    pub user_service_reference: Option<String>,
    /// Invoice ID when supplied.
    #[serde(rename = "InvoiceID")]
    pub invoice_id: Option<i64>,
    /// Creation timestamp in the API's original format.
    pub created_at: String,
}

/// A payment entry as reported by billing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", try_from = "PaymentWire")]
pub struct Payment {
    /// Numeric payment ID.
    #[serde(rename = "ID")]
    pub id: i64,
    /// Payment type code as reported by billing.
    pub type_code: i64,
    /// Payment amount in the API's monetary basis.
    pub amount: f64,
    /// Payment transaction reference.
    #[serde(rename = "TransactionID")]
    pub transaction_id: String,
    /// Gateway as reported by billing.
    pub gateway: String,
    /// Status as reported by billing.
    pub status: String,
    /// Creation timestamp in the API's original format.
    pub created_at: String,
    /// Description when supplied.
    pub description: Option<String>,
}

pub(super) trait HistoryRow: DeserializeOwned {
    const SCHEMA: Shape<'static>;

    fn id(&self) -> i64;
    fn reflects_numbers(&self, credentials: &SentCredentials) -> bool;
    fn redact(&mut self, redactor: &Redactor);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransactionWire {
    #[serde(rename = "transactionID")]
    transaction_id: i64,
    transaction_type: i64,
    credit_status: bool,
    transaction_value: f64,
    cumulative_value: f64,
    #[serde(deserialize_with = "nullable")]
    source_reference: Option<String>,
    #[serde(deserialize_with = "nullable")]
    description: Option<String>,
    #[serde(deserialize_with = "nullable")]
    user_service_reference: Option<String>,
    #[serde(deserialize_with = "nullable")]
    invoice_id: Option<i64>,
    #[serde(rename = "createTimeStamp")]
    created_at: String,
}

impl TryFrom<TransactionWire> for Transaction {
    type Error = &'static str;

    fn try_from(wire: TransactionWire) -> Result<Self, Self::Error> {
        if wire.transaction_id <= 0
            || !valid_amount(wire.transaction_value)
            || !wire.cumulative_value.is_finite()
            || !valid_timestamp(&wire.created_at)
        {
            return Err("invalid transaction");
        }
        let direction = if wire.credit_status {
            DIRECTION_CREDIT
        } else {
            DIRECTION_DEBIT
        };
        let kind = match (wire.transaction_type, wire.credit_status) {
            (1, true) => TRANSACTION_PURCHASE,
            (2, true) => TRANSACTION_REFUND,
            (5, false) => TRANSACTION_CHARGE,
            (1 | 2 | 5, _) => return Err("invalid transaction"),
            _ => "",
        };
        Ok(Self {
            id: wire.transaction_id,
            r#type: kind.into(),
            type_code: wire.transaction_type,
            direction: direction.into(),
            amount: wire.transaction_value,
            balance: wire.cumulative_value,
            source_reference: wire.source_reference,
            description: wire.description,
            user_service_reference: wire.user_service_reference,
            invoice_id: wire.invoice_id,
            created_at: wire.created_at,
        })
    }
}

impl HistoryRow for Transaction {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("transactionID"),
        field("transactionType"),
        field("creditStatus"),
        field("transactionValue"),
        field("cumulativeValue"),
        field("sourceReference"),
        field("description"),
        field("userServiceReference"),
        field("invoiceId"),
        field("createTimeStamp"),
    ]);

    fn id(&self) -> i64 {
        self.id
    }

    fn reflects_numbers(&self, credentials: &SentCredentials) -> bool {
        [Some(self.id), Some(self.type_code), self.invoice_id]
            .into_iter()
            .flatten()
            .any(|value| reflects_integer(value, credentials))
            || output_number::reflects(self.amount, credentials, true)
            || output_number::reflects(self.balance, credentials, true)
    }

    fn redact(&mut self, redactor: &Redactor) {
        for text in [&mut self.r#type, &mut self.direction, &mut self.created_at]
            .into_iter()
            .chain(self.source_reference.as_mut())
            .chain(self.description.as_mut())
            .chain(self.user_service_reference.as_mut())
        {
            redactor.redact(text);
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaymentWire {
    #[serde(rename = "paymentID")]
    payment_id: i64,
    payment_type: i64,
    payment_amount: f64,
    #[serde(rename = "transactionID")]
    transaction_id: String,
    pay_gate: String,
    payment_status: String,
    #[serde(rename = "createTimeStamp")]
    created_at: String,
    #[serde(deserialize_with = "nullable")]
    description: Option<String>,
}

impl TryFrom<PaymentWire> for Payment {
    type Error = &'static str;

    fn try_from(wire: PaymentWire) -> Result<Self, Self::Error> {
        if wire.payment_id <= 0
            || !valid_amount(wire.payment_amount)
            || !valid_timestamp(&wire.created_at)
        {
            return Err("invalid payment");
        }
        Ok(Self {
            id: wire.payment_id,
            type_code: wire.payment_type,
            amount: wire.payment_amount,
            transaction_id: wire.transaction_id,
            gateway: wire.pay_gate,
            status: wire.payment_status,
            created_at: wire.created_at,
            description: wire.description,
        })
    }
}

impl HistoryRow for Payment {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("paymentID"),
        field("paymentType"),
        field("paymentAmount"),
        field("transactionID"),
        field("payGate"),
        field("paymentStatus"),
        field("createTimeStamp"),
        field("description"),
    ]);

    fn id(&self) -> i64 {
        self.id
    }

    fn reflects_numbers(&self, credentials: &SentCredentials) -> bool {
        [self.id, self.type_code]
            .into_iter()
            .any(|value| reflects_integer(value, credentials))
            || output_number::reflects(self.amount, credentials, true)
    }

    fn redact(&mut self, redactor: &Redactor) {
        for text in [
            &mut self.transaction_id,
            &mut self.gateway,
            &mut self.status,
            &mut self.created_at,
        ]
        .into_iter()
        .chain(self.description.as_mut())
        {
            redactor.redact(text);
        }
    }
}

fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(decoder)
}

fn valid_amount(amount: f64) -> bool {
    amount.is_finite() && amount >= 0.0
}

fn valid_timestamp(text: &str) -> bool {
    static TIME: LazyLock<Result<regex_lite::Regex, regex_lite::Error>> = LazyLock::new(|| {
        regex_lite::Regex::new(concat!(
            r"^(?:[01]?[0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](?:[.,][0-9]+)?",
            r"(?:Z|[+-](?:[01][0-9]|2[0-4]):(?:[0-5][0-9]|60))$",
        ))
    });
    text.split_once('T').is_some_and(|(date, time)| {
        parse_date(date).is_some() && TIME.as_ref().is_ok_and(|pattern| pattern.is_match(time))
    })
}
