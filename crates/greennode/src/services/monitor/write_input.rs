use std::{collections::HashMap, fmt};

use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::{Value, value::RawValue};

use crate::cli::input::{InputError, JsonChecks, JsonNormalization};

const INVALID: InputError = InputError("--cli-input-json: invalid nested Monitor input");

enum Field {
    Text,
    Bool,
    Integer,
    TextList,
}

const HEADERS: &[(&str, Field)] = &[("Key", Field::Text), ("Value", Field::Text)];
const ASSERTIONS: &[(&str, Field)] = &[
    ("type", Field::Text),
    ("operator", Field::Text),
    ("target", Field::Text),
];
const NOTIFICATIONS: &[(&str, Field)] = &[
    ("In-alarm", Field::TextList),
    ("Up", Field::TextList),
    ("Undetermined", Field::TextList),
];
const RESEND: &[(&str, Field)] = &[
    ("Enabled", Field::Bool),
    ("Statuses", Field::TextList),
    ("Period", Field::Integer),
    ("Times", Field::Integer),
];

pub(super) const CHECK: JsonChecks = checks(|bytes| {
    normalize(
        bytes,
        &[
            ("Assertions", ASSERTIONS, true),
            ("Notifications", NOTIFICATIONS, false),
        ],
    )
});
pub(super) const CHANNEL: JsonChecks =
    checks(|bytes| normalize(bytes, &[("Headers", HEADERS, true)]));
pub(super) const ALARM: JsonChecks = checks(|bytes| normalize(bytes, &[("Resend", RESEND, false)]));

const fn checks(normalize: JsonNormalization) -> JsonChecks {
    JsonChecks {
        validate: |_| Ok(()),
        read_error: |error| error,
        normalize: Some(normalize),
    }
}

type FieldShape = (&'static str, &'static [(&'static str, Field)], bool);

fn normalize(bytes: &[u8], shapes: &[FieldShape]) -> Result<Vec<u8>, InputError> {
    let mut root: Option<HashMap<String, Box<RawValue>>> =
        serde_json::from_slice(bytes).map_err(|_| INVALID)?;
    if let Some(root) = &mut root {
        for &(name, fields, list) in shapes {
            if let Some(raw) = root.get_mut(name).filter(|raw| raw.get() != "null") {
                let decoded = if list {
                    let items: Vec<&RawValue> =
                        serde_json::from_str(raw.get()).map_err(|_| INVALID)?;
                    Value::Array(
                        items
                            .into_iter()
                            .map(|item| object(item, fields))
                            .collect::<Result<_, _>>()?,
                    )
                } else {
                    object(raw, fields)?
                };
                *raw = serde_json::value::to_raw_value(&decoded).map_err(|_| INVALID)?;
            }
        }
    }
    serde_json::to_vec(&root).map_err(|_| INVALID)
}

fn object(raw: &RawValue, fields: &[(&str, Field)]) -> Result<Value, InputError> {
    let mut output = serde_json::Map::new();
    if raw.get() == "null" {
        return Ok(Value::Object(output));
    }
    let ordered = serde_json::Deserializer::from_str(raw.get())
        .deserialize_map(Ordered)
        .map_err(|_| INVALID)?;
    for (key, raw) in ordered {
        let (name, kind) = fields
            .iter()
            .find(|(name, _)| go_field_eq(&key, name))
            .ok_or(INVALID)?;
        let text = raw.get();
        if text == "null" {
            if matches!(kind, Field::TextList) {
                output.remove(*name);
            }
            continue;
        }
        let value = match kind {
            Field::Text => Value::String(serde_json::from_str(text).map_err(|_| INVALID)?),
            Field::Bool => Value::Bool(serde_json::from_str(text).map_err(|_| INVALID)?),
            Field::Integer => Value::from(serde_json::from_str::<i64>(text).map_err(|_| INVALID)?),
            Field::TextList => Value::Array(
                serde_json::from_str::<Vec<Option<String>>>(text)
                    .map_err(|_| INVALID)?
                    .into_iter()
                    .map(|value| Value::String(value.unwrap_or_default()))
                    .collect(),
            ),
        };
        output.insert((*name).to_owned(), value);
    }
    Ok(Value::Object(output))
}

fn go_field_eq(key: &str, ascii_name: &str) -> bool {
    fn fold(character: char) -> char {
        match character {
            'ſ' => 'S',
            'K' => 'K',
            character => character.to_ascii_uppercase(),
        }
    }
    key.chars().map(fold).eq(ascii_name.chars().map(fold))
}

struct Ordered;

impl<'de> Visitor<'de> for Ordered {
    type Value = Vec<(String, &'de RawValue)>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a Monitor input object")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut object: M) -> Result<Self::Value, M::Error> {
        let mut fields = Vec::new();
        while let Some(field) = object.next_entry()? {
            fields.push(field);
        }
        Ok(fields)
    }
}
