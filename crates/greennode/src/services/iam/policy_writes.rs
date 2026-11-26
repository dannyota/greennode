use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, value::RawValue};

use super::*;
use crate::cli::dispatch::LocalError;
use crate::cli::input::JsonChecks;
use crate::cli::render::decode_string;

const DOCUMENT_FILE: Field = flag("document-file", "string", false);
const MISSING_STATEMENTS: &str = "create-policy needs Statements: pass --document-file <path> or set Statements with --cli-input-json";
const DOCUMENT_SIZE: u64 = 64 * 1024;

struct PolicyInput<T> {
    sdk: T,
    document_file: String,
}

impl<T> PolicyInput<T> {
    fn new(sdk: T) -> Self {
        Self {
            sdk,
            document_file: String::new(),
        }
    }
}

const fn document_file<T>() -> InputField<PolicyInput<T>> {
    InputField {
        json_name: "DocumentFile",
        flag: Some(DOCUMENT_FILE),
        secret: false,
        nullable: false,
        set_json: |_, _| Err(()),
        set_flag: Some(|input, matches| {
            input.document_file = matches
                .get_one::<String>(DOCUMENT_FILE.name)
                .ok_or(())?
                .clone();
            Ok(())
        }),
        requirement: Requirement::Optional,
    }
}

macro_rules! policy_field {
    ($input:ident, $member:ident, $flag:ident, $name:literal, $wrap:path, $requirement:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut PolicyInput<sdk::$input>, value| {
                let Value::String(value) = value else {
                    return Err(());
                };
                input.sdk.$member = $wrap(value);
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.sdk.$member = $wrap(matches.get_one::<String>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}

macro_rules! statements_field {
    ($input:ident, $wrap:path) => {
        InputField {
            json_name: "Statements",
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input: &mut PolicyInput<sdk::$input>, value| {
                let raw = serde_json::value::to_raw_value(&value).map_err(|_| ())?;
                input.sdk.statements = $wrap(statements(&raw)?);
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }
    };
}

const CREATE_INPUT: InputDescriptor<PolicyInput<sdk::CreatePolicyInput>> = InputDescriptor {
    defaults: || PolicyInput::new(sdk::CreatePolicyInput::new("", Vec::new())),
    fields: &[
        policy_field!(
            CreatePolicyInput,
            name,
            REQUIRED_NAME,
            "Name",
            std::convert::identity,
            Requirement::Nonzero(|input| !input.sdk.name.is_empty())
        ),
        policy_field!(
            CreatePolicyInput,
            description,
            DESCRIPTION,
            "Description",
            std::convert::identity,
            Requirement::Optional
        ),
        statements_field!(CreatePolicyInput, std::convert::identity),
        document_file(),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: Some(|input, _| {
        if input.document_file.is_empty() && input.sdk.statements.is_empty() {
            return Err(InputError(MISSING_STATEMENTS));
        }
        Ok(())
    }),
};

const UPDATE_INPUT: InputDescriptor<PolicyInput<sdk::UpdatePolicyInput>> = InputDescriptor {
    defaults: || PolicyInput::new(sdk::UpdatePolicyInput::new("")),
    fields: &[
        policy_field!(
            UpdatePolicyInput,
            policy_id,
            POLICY_ID,
            "PolicyID",
            std::convert::identity,
            Requirement::Nonzero(|input| !input.sdk.policy_id.is_empty())
        ),
        policy_field!(
            UpdatePolicyInput,
            name,
            OPTIONAL_NAME,
            "Name",
            Some,
            Requirement::Optional
        ),
        policy_field!(
            UpdatePolicyInput,
            description,
            OPTIONAL_DESCRIPTION,
            "Description",
            Some,
            Requirement::Optional
        ),
        statements_field!(UpdatePolicyInput, Some),
        document_file(),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};

const JSON_CHECKS: Option<JsonChecks> = Some(JsonChecks {
    validate: check_input_json,
    read_error: std::convert::identity,
});

pub(super) const CREATE: Operation = Operation {
    name: "create-policy",
    sdk_operation: "iam.CreatePolicy",
    short_help: "Create a customer IAM policy",
    example: Some("greennode iam create-policy --name <name> --document-file policy.json"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[REQUIRED_NAME, DESCRIPTION],
    json_fields: &[flag("Statements", "[]iam.Statement", true)],
    extra_flags: &[DOCUMENT_FILE],
    notes: &[
        "Use a JSON document with a `statements` array (up to 64 KiB). `--document-file` takes precedence over `Statements` in `--cli-input-json`. The SDK refuses statements that grant IAM write actions.",
    ],
    handler: Some(Handler::new(
        &CREATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_custom(
                invocation,
                operation,
                matches,
                &CREATE_INPUT,
                |ctx, config, input| {
                    Box::pin(async move {
                        let mut sdk = input.sdk.clone();
                        if let Some(statements) = load_document(&input.document_file).await? {
                            if statements.is_empty() {
                                return Err(LocalError::usage(MISSING_STATEMENTS).into());
                            }
                            sdk.statements = statements;
                        }
                        dispatch::project_result(
                            sdk::Client::new(config).create_policy(ctx, sdk).await,
                            std::convert::identity,
                        )
                    })
                },
                dispatch::OutputView::default(),
                JSON_CHECKS,
            ))
        },
    )),
};

pub(super) const UPDATE: Operation = Operation {
    name: "update-policy",
    sdk_operation: "iam.UpdatePolicy",
    short_help: "Replace a customer IAM policy",
    example: Some(
        "greennode iam update-policy --policy-id <policy-id> --document-file policy.json --yes",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::DestructiveWrite,
    flags: &[POLICY_ID, OPTIONAL_NAME, OPTIONAL_DESCRIPTION],
    json_fields: &[flag("Statements", "*[]iam.Statement", false)],
    extra_flags: &[DOCUMENT_FILE],
    notes: &[
        "Omitted fields retain their current values. The SDK refuses managed policies, privileged statements, and changes that would affect protected principals or groups.",
    ],
    handler: Some(Handler::new(
        &UPDATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_custom(
                invocation,
                operation,
                matches,
                &UPDATE_INPUT,
                |ctx, config, input| {
                    Box::pin(async move {
                        let mut sdk = input.sdk.clone();
                        if let Some(statements) = load_document(&input.document_file).await? {
                            sdk.statements = Some(statements);
                        }
                        dispatch::project_result(
                            sdk::Client::new(config).update_policy(ctx, sdk).await,
                            std::convert::identity,
                        )
                    })
                },
                dispatch::OutputView::default(),
                JSON_CHECKS,
            ))
        },
    )),
};

async fn load_document(path: &str) -> Result<Option<Vec<sdk::Statement>>, LocalError> {
    if path.is_empty() {
        return Ok(None);
    }
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || read_document(&path))
        .await
        .map_err(|_| LocalError::usage("--document-file: cannot read file"))?
        .map(Some)
}

fn read_document(path: &str) -> Result<Vec<sdk::Statement>, LocalError> {
    let file =
        open_document(path).map_err(|_| LocalError::usage("--document-file: cannot open file"))?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(LocalError::usage("--document-file: file must be regular"));
    }
    let mut bytes = Vec::new();
    file.take(DOCUMENT_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalError::usage("--document-file: cannot read file"))?;
    if bytes.len() as u64 > DOCUMENT_SIZE {
        return Err(LocalError::usage("--document-file: file exceeds 64 KiB"));
    }
    let invalid = |_| LocalError::usage("--document-file: invalid policy document");
    let text = document_text(&bytes).map_err(invalid)?;
    let fields = serde_json::from_str::<Option<RawFields<'_>>>(&text)
        .map_err(|_| invalid(()))?
        .unwrap_or_default();
    let fields = fields.named(&["statements"]).map_err(invalid)?;
    fields
        .get("statements")
        .map_or_else(|| Ok(Vec::new()), |raw| statements(raw).map_err(invalid))
}

fn document_text(bytes: &[u8]) -> Result<String, ()> {
    let mut text = String::new();
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        text.extend(std::iter::repeat_n(
            char::REPLACEMENT_CHARACTER,
            chunk.invalid().len(),
        ));
    }
    let mut normalized = String::new();
    let mut copied = 0;
    let mut quoted = None;
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quoted.is_some() && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            if let Some(start) = quoted.take() {
                let raw = text.get(start..=index).ok_or(())?;
                if raw.contains("\\u") {
                    let value = decode_string(raw).map_err(|_| ())?;
                    normalized.push_str(text.get(copied..start).ok_or(())?);
                    normalized.push_str(&serde_json::to_string(&value).map_err(|_| ())?);
                    copied = index + 1;
                }
            } else {
                quoted = Some(index);
            }
        }
    }
    if copied == 0 {
        return Ok(text);
    }
    normalized.push_str(text.get(copied..).ok_or(())?);
    Ok(normalized)
}

#[cfg(unix)]
fn open_document(path: &str) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};
    Ok(open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )?
    .into())
}

#[cfg(not(unix))]
fn open_document(path: &str) -> io::Result<File> {
    File::open(path)
}

#[derive(Default)]
struct RawFields<'a>(BTreeMap<String, &'a RawValue>);

impl<'de> Deserialize<'de> for RawFields<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = RawFields<'de>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut fields = BTreeMap::new();
                while let Some((name, value)) = map.next_entry()? {
                    if fields.insert(name, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate field"));
                    }
                }
                Ok(RawFields(fields))
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

impl<'a> RawFields<'a> {
    fn named(self, names: &[&'static str]) -> Result<BTreeMap<&'static str, &'a RawValue>, ()> {
        let mut fields = BTreeMap::new();
        for (name, value) in self.0 {
            let name = names
                .iter()
                .copied()
                .find(|field| field.eq_ignore_ascii_case(&name))
                .ok_or(())?;
            if fields.insert(name, value).is_some() {
                return Err(());
            }
        }
        Ok(fields)
    }
}

fn check_input_json(bytes: &[u8]) -> Result<(), InputError> {
    let invalid = InputError("--cli-input-json: invalid policy input");
    let fields: Option<RawFields<'_>> = serde_json::from_slice(bytes).map_err(|_| invalid)?;
    if let Some(fields) = fields {
        let names = ["Name", "Description", "PolicyID", "Statements"];
        let fields = fields
            .named(&names)
            .map_err(|_| InputError("--cli-input-json: invalid policy input"))?;
        if let Some(raw) = fields.get("Statements") {
            statements(raw)
                .map_err(|_| InputError("--cli-input-json: invalid policy statements"))?;
        }
    }
    Ok(())
}

fn statements(raw: &RawValue) -> Result<Vec<sdk::Statement>, ()> {
    let rows: Option<Vec<&RawValue>> = serde_json::from_str(raw.get()).map_err(|_| ())?;
    rows.unwrap_or_default()
        .into_iter()
        .map(statement)
        .collect()
}

fn statement(raw: &RawValue) -> Result<sdk::Statement, ()> {
    let fields: Option<RawFields<'_>> = serde_json::from_str(raw.get()).map_err(|_| ())?;
    let mut fields =
        fields
            .unwrap_or_default()
            .named(&["effect", "actions", "resources", "condition"])?;
    let mut statement = sdk::Statement::default();
    statement.effect = fields
        .remove("effect")
        .map_or_else(|| Ok(String::new()), string)?;
    statement.actions = fields
        .remove("actions")
        .map_or_else(|| Ok(Vec::new()), strings)?;
    statement.resources = fields
        .remove("resources")
        .map_or_else(|| Ok(Vec::new()), strings)?;
    if let Some(raw) = fields.remove("condition").filter(|raw| raw.get() != "null") {
        let fields: RawFields<'_> = serde_json::from_str(raw.get()).map_err(|_| ())?;
        let condition = fields
            .0
            .into_iter()
            .map(|(key, raw)| {
                if !matches!(raw.get().as_bytes().first(), Some(b'{' | b'n')) {
                    return Err(());
                }
                Ok((key, condition_value(raw, 0)?))
            })
            .collect::<Result<_, ()>>()?;
        statement.condition = Some(condition);
    }
    Ok(statement)
}

fn strings(raw: &RawValue) -> Result<Vec<String>, ()> {
    let rows: Option<Vec<&RawValue>> = serde_json::from_str(raw.get()).map_err(|_| ())?;
    rows.unwrap_or_default().into_iter().map(string).collect()
}

fn string(raw: &RawValue) -> Result<String, ()> {
    if raw.get() == "null" {
        Ok(String::new())
    } else {
        decode_string(raw.get()).map_err(|_| ())
    }
}

fn condition_value(raw: &RawValue, depth: u8) -> Result<Value, ()> {
    if depth == 128 {
        return Err(());
    }
    match raw.get().as_bytes().first() {
        Some(b'{') => {
            let fields: RawFields<'_> = serde_json::from_str(raw.get()).map_err(|_| ())?;
            fields
                .0
                .into_iter()
                .map(|(key, raw)| {
                    if key == "$serde_json::private::Number" {
                        return Err(());
                    }
                    Ok((key, condition_value(raw, depth + 1)?))
                })
                .collect::<Result<_, ()>>()
                .map(Value::Object)
        }
        Some(b'[') => {
            let rows: Vec<&RawValue> = serde_json::from_str(raw.get()).map_err(|_| ())?;
            rows.into_iter()
                .map(|raw| condition_value(raw, depth + 1))
                .collect::<Result<_, _>>()
                .map(Value::Array)
        }
        Some(b'"') => string(raw).map(Value::String),
        Some(b'-' | b'0'..=b'9') => serde_json::from_str::<f64>(raw.get())
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or(()),
        _ => serde_json::from_str(raw.get()).map_err(|_| ()),
    }
}
