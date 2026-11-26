use super::{api, invalid};
use crate::Error;
use crate::model::pricing::DiscoveryPrice;
use crate::quote::catalog::{exact, storage_data, unpaged};
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

pub(crate) fn monthly(op: &'static str, raw: &Value) -> Result<DiscoveryPrice, Box<Error>> {
    let obj = quote_object(op, raw)?;
    exact(op, obj, FIELDS)?;
    for row in properties(op, obj)? {
        if !row.is_null() {
            line(op, row)?;
        }
    }
    let monthly_price = numeric(op, obj, "optimumPrice")?;
    let original_price = numeric(op, obj, "originalPrice")?;
    let discount_price = numeric(op, obj, "discountPrice")?;
    let discount_percent = numeric(op, obj, "discountPercent")?;
    if !monthly_price.is_some_and(|value| value > 0.0)
        || [original_price, discount_price, discount_percent]
            .into_iter()
            .flatten()
            .any(|value| value < 0.0)
    {
        return Err(invalid(op, 200));
    }
    let currency = string(op, obj, "currency")?;
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

#[allow(dead_code, reason = "Shared quote decoder for compute pricing")]
pub(crate) fn instance(op: &'static str, raw: &Value) -> Result<DiscoveryPrice, Box<Error>> {
    let obj = quote_object(op, raw)?;
    check_envelope(op, obj)?;
    let mut matches = None;
    for row in properties(op, obj)? {
        if row.is_null() {
            continue;
        }
        let item = line(op, row)?;
        let name = string(op, item, "name")?;
        let monthly = numeric(op, item, "monthlyPrice")?;
        let optimum = numeric(op, item, "optimumPrice")?;
        let percent = numeric(op, item, "discountPercent")?;
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
        let item = line(op, row)?;
        let name = string(op, item, "name")?;
        let monthly_price = numeric(op, item, "monthlyPrice")?;
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
