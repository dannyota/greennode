use super::{parse_date, security::Redactor};
use crate::billing::model::reflects_integer;
use crate::output_number;
use crate::wire::{product::sender::SentCredentials, strict_json};
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
#[derive(Clone, Deserialize, PartialEq, Serialize)]
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

crate::wire::product::output::public_fields!(Transaction {
    id,
    r#type,
    type_code,
    direction,
    amount,
    balance,
    source_reference,
    description,
    user_service_reference,
    invoice_id,
    created_at
});

/// A payment entry as reported by billing.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
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

crate::wire::product::output::public_fields!(Payment {
    id,
    type_code,
    amount,
    transaction_id,
    gateway,
    status,
    created_at,
    description
});

pub(super) trait HistoryRow: DeserializeOwned {
    fn id(&self) -> i64;
    fn reflects_numbers(&self, credentials: &SentCredentials) -> bool;
    fn redact(&mut self, redactor: &Redactor);
}

#[derive(Default)]
struct RowField<T> {
    has_exact_name: bool,
    value: Option<T>,
}

impl<T: DeserializeOwned> RowField<T> {
    fn finish<E: serde::de::Error>(self) -> Result<T, E> {
        if !self.has_exact_name {
            return Err(E::custom("missing exact history row field"));
        }
        self.value
            .map_or_else(|| serde_json::from_str("null").map_err(E::custom), Ok)
    }
}

macro_rules! history_row_wire {
    ($name:ident { $($wire:literal => $field:ident: $ty:ty),+ $(,)? }) => {
        struct $name {
            $($field: $ty,)+
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct WireVisitor;
                impl<'de> serde::de::Visitor<'de> for WireVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str("a billing history row")
                    }

                    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                        $(let mut $field = RowField::<$ty>::default();)+
                        while let Some(key) = map.next_key::<String>()? {
                            match [$($wire),+].into_iter().find(|name| strict_json::field_matches(name, &key)) {
                                $(Some($wire) => {
                                    $field.has_exact_name |= key == $wire;
                                    $field.value = map.next_value()?;
                                })+
                                _ => { map.next_value::<serde::de::IgnoredAny>()?; }
                            }
                        }
                        Ok($name { $($field: $field.finish()?,)+ })
                    }
                }
                deserializer.deserialize_map(WireVisitor)
            }
        }
    };
}

history_row_wire!(TransactionWire {
    "transactionID" => transaction_id: i64,
    "transactionType" => transaction_type: i64,
    "creditStatus" => credit_status: bool,
    "transactionValue" => transaction_value: f64,
    "cumulativeValue" => cumulative_value: f64,
    "sourceReference" => source_reference: Option<String>,
    "description" => description: Option<String>,
    "userServiceReference" => user_service_reference: Option<String>,
    "invoiceId" => invoice_id: Option<i64>,
    "createTimeStamp" => created_at: String,
});

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

history_row_wire!(PaymentWire {
    "paymentID" => payment_id: i64,
    "paymentType" => payment_type: i64,
    "paymentAmount" => payment_amount: f64,
    "transactionID" => transaction_id: String,
    "payGate" => pay_gate: String,
    "paymentStatus" => payment_status: String,
    "createTimeStamp" => created_at: String,
    "description" => description: Option<String>,
});

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
