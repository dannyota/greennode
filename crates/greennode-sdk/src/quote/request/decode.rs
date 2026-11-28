use super::{api, invalid};
use crate::Error;
use crate::model::pricing::DiscoveryPrice;
use crate::quote::catalog::{envelope_fields, exact, record, storage_data, unpaged, wire_field};
use serde::Deserialize;
use serde_json::{Map, Value};

const AMOUNTS: &[&str] = &[
    "optimumPrice",
    "originalPrice",
    "discountPrice",
    "discountPercent",
];
const FIELDS: &[&str] = &[
    "currency",
    "propertiesPrice",
    "success",
    "errorCode",
    "errorMsg",
];
const PROPERTIES: &[&str] = &[
    "name",
    "description",
    "optimumPrice",
    "monthlyPrice",
    "currentPrice",
    "discountPercent",
];

pub(crate) struct PriceLine {
    pub(crate) name: String,
    pub(crate) monthly_price: Option<f64>,
}

pub(crate) struct Quote {
    pub(crate) price: DiscoveryPrice,
    pub(crate) breakdown: Vec<PriceLine>,
    pub(crate) breakdown_mismatch: bool,
}

fn object<'a>(op: &'static str, value: &'a Value) -> Result<&'a Map<String, Value>, Box<Error>> {
    value.as_object().ok_or_else(|| invalid(op, 200))
}

pub(crate) fn numeric(
    op: &'static str,
    obj: &Map<String, Value>,
    field: &str,
) -> Result<Option<f64>, Box<Error>> {
    match obj.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|amount| amount.is_finite())
            .map(Some)
            .ok_or_else(|| invalid(op, 200)),
    }
}

pub(crate) fn string(
    op: &'static str,
    obj: &Map<String, Value>,
    key: &str,
) -> Result<String, Box<Error>> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(invalid(op, 200)),
    }
}

pub(crate) fn properties<'a>(
    op: &'static str,
    obj: &'a Map<String, Value>,
) -> Result<&'a [Value], Box<Error>> {
    match obj.get("propertiesPrice") {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(rows)) => Ok(rows),
        _ => Err(invalid(op, 200)),
    }
}

pub(crate) fn line<'a>(
    op: &'static str,
    row: &'a Value,
) -> Result<&'a Map<String, Value>, Box<Error>> {
    let item = object(op, row)?;
    exact(op, item, PROPERTIES)?;
    Ok(item)
}

pub(crate) fn quote_object<'a>(
    op: &'static str,
    raw: &'a Value,
) -> Result<&'a Map<String, Value>, Box<Error>> {
    let obj = object(op, raw)?;
    exact(op, obj, AMOUNTS)?;
    exact(op, obj, &["propertiesPrice"])?;
    Ok(obj)
}

pub(crate) fn invalid_quote(op: &'static str, status: Option<u16>) -> Error {
    api(
        op,
        status,
        "InvalidResponse",
        "invalid quote response; body withheld",
        false,
    )
}

fn validate_quote_fields(op: &'static str, raw: &Value) -> Result<(), Box<Error>> {
    let mut next = Some(raw);
    while let Some(value) = next {
        let Some(obj) = value.as_object() else { break };
        exact(op, obj, AMOUNTS)?;
        exact(op, obj, FIELDS)?;
        envelope_fields(op, obj)?;
        if let Some(Value::Array(rows)) = obj.get("propertiesPrice") {
            for row in rows.iter().filter_map(Value::as_object) {
                exact(op, row, PROPERTIES)?;
            }
        }
        next = obj.get("data");
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonthlyAmounts {
    #[serde(default, deserialize_with = "wire_field")]
    optimum_price: Option<f64>,
    #[serde(default, deserialize_with = "wire_field")]
    original_price: Option<f64>,
    #[serde(default, deserialize_with = "wire_field")]
    discount_price: Option<f64>,
    #[serde(default, deserialize_with = "wire_field")]
    discount_percent: Option<f64>,
}

pub(crate) fn monthly(op: &'static str, raw: &Value) -> Result<DiscoveryPrice, Box<Error>> {
    monthly_price(op, raw, true)
}

pub(crate) fn network_monthly(op: &'static str, raw: &Value) -> Result<DiscoveryPrice, Box<Error>> {
    monthly_price(op, raw, false)
}

fn monthly_price(
    op: &'static str,
    raw: &Value,
    read_currency: bool,
) -> Result<DiscoveryPrice, Box<Error>> {
    validate_quote_fields(op, raw)?;
    let obj = object(op, raw)?;
    let MonthlyAmounts {
        optimum_price: monthly_price,
        original_price,
        discount_price,
        discount_percent,
    } = record(op, raw, &[])?;
    if !monthly_price.is_some_and(|value| value > 0.0)
        || [original_price, discount_price, discount_percent]
            .into_iter()
            .flatten()
            .any(|value| value < 0.0)
    {
        return Err(invalid(op, 200));
    }
    let currency = if read_currency {
        string(op, obj, "currency")?
    } else {
        String::new()
    };
    Ok(DiscoveryPrice {
        status: "quoted".into(),
        basis: "monthly-total".into(),
        currency: if currency.is_empty() {
            "VND".into()
        } else {
            currency
        },
        tax: "as-returned".into(),
        source: op.into(),
        monthly_price,
        original_price,
        discount_price,
        discount_percent,
    })
}

fn check_envelope(op: &'static str, obj: &Map<String, Value>) -> Result<(), Box<Error>> {
    unpaged(op, obj, false)?;
    if obj
        .get("success")
        .is_some_and(|value| value != &Value::Bool(true))
        || ["errorCode", "errorMsg"].iter().any(|key| {
            obj.get(*key)
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        })
    {
        return Err(invalid(op, 200));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceLine {
    #[serde(default, deserialize_with = "wire_field")]
    name: String,
    #[serde(default, deserialize_with = "wire_field")]
    monthly_price: Option<f64>,
    #[serde(default, deserialize_with = "wire_field")]
    optimum_price: Option<f64>,
    #[serde(default, deserialize_with = "wire_field")]
    discount_percent: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EstimateLine {
    #[serde(default, deserialize_with = "wire_field")]
    name: String,
    #[serde(default, deserialize_with = "wire_field")]
    monthly_price: Option<f64>,
}

#[allow(dead_code, reason = "Shared quote decoder for compute pricing")]
pub(crate) fn instance(op: &'static str, raw: &Value) -> Result<DiscoveryPrice, Box<Error>> {
    validate_quote_fields(op, raw)?;
    let obj = quote_object(op, raw)?;
    check_envelope(op, obj)?;
    let mut matches = None;
    for row in properties(op, obj)? {
        if row.is_null() {
            continue;
        }
        let InstanceLine {
            name,
            monthly_price: monthly,
            optimum_price: optimum,
            discount_percent: percent,
        } = record(op, row, &[])?;
        if name != "INSTANCE TYPE" {
            continue;
        }
        if matches.is_some()
            || !monthly.is_some_and(|value| value > 0.0)
            || monthly != optimum
            || percent.is_some_and(|value| value < 0.0)
        {
            return Err(invalid(op, 200));
        }
        matches = Some((monthly, percent));
    }
    let (monthly_price, discount_percent) = matches.ok_or_else(|| invalid(op, 200))?;
    Ok(DiscoveryPrice {
        status: "quoted".into(),
        basis: "instance-type-line".into(),
        currency: "VND".into(),
        tax: "as-returned".into(),
        source: op.into(),
        monthly_price,
        discount_percent,
        ..DiscoveryPrice::default()
    })
}

pub(crate) fn estimate(op: &'static str, raw: &Value, storage: bool) -> Result<Quote, Box<Error>> {
    validate_quote_fields(op, raw)?;
    let raw = if storage {
        storage_data(op, raw, false)?
    } else {
        raw
    };
    let obj = quote_object(op, raw)?;
    check_envelope(op, obj)?;
    let price = monthly(op, raw)?;
    if obj
        .get("currency")
        .is_some_and(|value| value.as_str() != Some("VND"))
    {
        return Err(invalid(op, 200));
    }
    let mut breakdown = Vec::new();
    let mut sum = None;
    for row in properties(op, obj)? {
        let EstimateLine {
            name,
            monthly_price,
        } = record(op, row, &[])?;
        if let Some(amount) = monthly_price {
            if amount < 0.0 {
                return Err(invalid(op, 200));
            }
            sum = Some(sum.unwrap_or(0.0) + amount);
        }
        breakdown.push(PriceLine {
            name,
            monthly_price,
        });
    }
    let breakdown_mismatch = sum.is_some() && sum != price.monthly_price;
    Ok(Quote {
        price,
        breakdown,
        breakdown_mismatch,
    })
}
