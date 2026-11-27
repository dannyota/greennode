use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Read;

use clap::{ArgMatches, parser::ValueSource};
use serde_json::Value;

use super::op::Field;
use super::render::decode_string;

pub type FlagSetter<T> = fn(&mut T, &ArgMatches) -> Result<(), ()>;
pub type ProjectSetter<T> = fn(&mut T, &str);
pub type InputGuard<T> = fn(&T, &ArgMatches) -> Result<(), InputError>;
pub(crate) type JsonNormalization = fn(&[u8]) -> Result<Vec<u8>, InputError>;
#[derive(Clone, Copy)]
pub(crate) struct JsonChecks {
    pub(crate) validate: fn(&[u8]) -> Result<(), InputError>,
    pub(crate) read_error: fn(InputError) -> InputError,
    pub(crate) normalize: Option<JsonNormalization>,
}
pub type ArgvGuard = fn(&ArgMatches, Option<&str>) -> Result<(), InputError>;

#[derive(Clone, Copy)]
pub enum Requirement<T> {
    Optional,
    Presence,
    Nonzero(fn(&T) -> bool),
}

pub struct InputField<T> {
    pub json_name: &'static str,
    pub flag: Option<Field>,
    pub secret: bool,
    pub nullable: bool,
    pub set_json: fn(&mut T, Value) -> Result<(), ()>,
    pub set_flag: Option<FlagSetter<T>>,
    pub requirement: Requirement<T>,
}

pub struct InputDescriptor<T: 'static> {
    pub defaults: fn() -> T,
    pub fields: &'static [InputField<T>],
    pub allow_json: bool,
    pub allow_query: bool,
    pub project_binding: Option<(&'static str, ProjectSetter<T>)>,
    pub pre_guard: Option<ArgvGuard>,
    pub guard: Option<InputGuard<T>>,
}

pub struct ParsedOperationInput<T> {
    pub value: T,
    pub present: HashSet<&'static str>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InputError(pub &'static str);

pub fn parse<T: 'static>(
    descriptor: &InputDescriptor<T>,
    matches: &ArgMatches,
    json: Option<&str>,
    project: Option<&str>,
) -> Result<ParsedOperationInput<T>, InputError> {
    before_input(descriptor, matches, json)?;
    let input = merge(descriptor, matches, json, project)?;
    check_guards(descriptor, &input, matches)?;
    Ok(input)
}

pub(crate) fn before_input<T>(
    descriptor: &InputDescriptor<T>,
    matches: &ArgMatches,
    json: Option<&str>,
) -> Result<(), InputError> {
    if let Some(guard) = descriptor.pre_guard {
        guard(matches, json)?;
    }
    Ok(())
}

pub(crate) fn merge<T>(
    descriptor: &InputDescriptor<T>,
    matches: &ArgMatches,
    json: Option<&str>,
    project: Option<&str>,
) -> Result<ParsedOperationInput<T>, InputError> {
    merge_checked(descriptor, matches, json, project, None)
}

pub(crate) fn merge_checked<T>(
    descriptor: &InputDescriptor<T>,
    matches: &ArgMatches,
    json: Option<&str>,
    project: Option<&str>,
    checks: Option<JsonChecks>,
) -> Result<ParsedOperationInput<T>, InputError> {
    let mut value = (descriptor.defaults)();
    let mut present = HashSet::new();
    if let Some(raw) = json.filter(|raw| !raw.is_empty()) {
        if !descriptor.allow_json {
            return Err(InputError("--cli-input-json is disabled"));
        }
        let bytes = match checks {
            Some(checks) => {
                let bytes = read_input(raw).map_err(checks.read_error)?;
                let bytes = match normalized(&bytes) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        (checks.validate)(&bytes)?;
                        return Err(error);
                    }
                };
                (checks.validate)(&bytes)?;
                if let Some(normalize) = checks.normalize {
                    normalize(&bytes)?
                } else {
                    bytes
                }
            }
            None => normalized(&read_input(raw)?)?,
        };
        let input: Option<HashMap<String, &serde_json::value::RawValue>> =
            serde_json::from_slice(&bytes)
                .map_err(|_| InputError("--cli-input-json: invalid JSON object"))?;
        if let Some(input) = input {
            for (name, raw) in input {
                let item =
                    raw_value(raw).map_err(|_| InputError("--cli-input-json: invalid field"))?;
                let field = descriptor
                    .fields
                    .iter()
                    .find(|field| field.json_name == name)
                    .ok_or(InputError("--cli-input-json: unknown field"))?;
                if field.secret {
                    return Err(InputError("--cli-input-json: secret field is forbidden"));
                }
                if item.is_null() {
                    if field.nullable {
                        (field.set_json)(&mut value, item)
                            .map_err(|_| InputError("--cli-input-json: invalid field"))?;
                    }
                } else {
                    (field.set_json)(&mut value, item)
                        .map_err(|_| InputError("--cli-input-json: invalid field"))?;
                    present.insert(field.json_name);
                }
            }
        }
    }
    for field in descriptor.fields {
        if let Some(flag) = field.flag
            && matches.value_source(flag.name) == Some(ValueSource::CommandLine)
        {
            let setter = field.set_flag.ok_or(InputError("invalid flag binding"))?;
            setter(&mut value, matches).map_err(|_| InputError("invalid flag value"))?;
            present.insert(field.json_name);
        }
    }
    if let Some(project) = project.filter(|project| !project.is_empty())
        && let Some((field, setter)) = descriptor.project_binding
    {
        setter(&mut value, project);
        present.insert(field);
    }
    Ok(ParsedOperationInput { value, present })
}

pub(crate) fn check_guards<T>(
    descriptor: &InputDescriptor<T>,
    input: &ParsedOperationInput<T>,
    matches: &ArgMatches,
) -> Result<(), InputError> {
    if !descriptor.allow_query
        && matches.try_contains_id("query").unwrap_or(false)
        && matches.value_source("query") == Some(ValueSource::CommandLine)
    {
        return Err(InputError("--query is disabled"));
    }
    if let Some(guard) = descriptor.guard {
        guard(&input.value, matches)?;
    }
    Ok(())
}

pub fn validate_descriptor<T: 'static>(descriptor: &InputDescriptor<T>) -> Result<(), InputError> {
    let mut names = HashSet::new();
    let mut flags = HashSet::new();
    for field in descriptor.fields {
        if field.json_name.is_empty() || !names.insert(field.json_name) {
            return Err(InputError("duplicate or empty input field"));
        }
        if field.secret && (field.flag.is_some() || field.set_flag.is_some()) {
            return Err(InputError("secret has ordinary flag"));
        }
        if field.flag.is_some() != field.set_flag.is_some() {
            return Err(InputError("invalid flag binding"));
        }
        if let Some(flag) = field.flag
            && (!flags.insert(flag.name)
                || flag.name == "cli-input-json"
                || crate::services::GLOBAL_FLAGS.contains(&flag.name))
        {
            return Err(InputError("duplicate or reserved flag"));
        }
    }
    if let Some((field, _)) = descriptor.project_binding
        && !descriptor
            .fields
            .iter()
            .any(|item| item.json_name == field && !item.secret)
    {
        return Err(InputError("invalid project binding"));
    }
    Ok(())
}

pub fn check_required<T: 'static>(
    descriptor: &InputDescriptor<T>,
    input: &ParsedOperationInput<T>,
) -> Result<(), InputError> {
    for field in descriptor.fields {
        let valid = match field.requirement {
            Requirement::Optional => true,
            Requirement::Presence => input.present.contains(field.json_name),
            Requirement::Nonzero(check) => check(&input.value),
        };
        if !valid {
            return Err(InputError(
                field.flag.map_or(field.json_name, |flag| flag.name),
            ));
        }
    }
    Ok(())
}

fn raw_value(raw: &serde_json::value::RawValue) -> Result<Value, serde_json::Error> {
    raw_value_at(raw, 128)
}

fn raw_value_at(
    raw: &serde_json::value::RawValue,
    depth: usize,
) -> Result<Value, serde_json::Error> {
    if depth == 0 {
        return Err(serde::de::Error::custom("JSON nesting limit exceeded"));
    }
    match raw.get().as_bytes().first() {
        Some(b'{') => {
            serde_json::from_str::<HashMap<String, &serde_json::value::RawValue>>(raw.get())?
                .into_iter()
                .map(|(key, value)| Ok((key, raw_value_at(value, depth - 1)?)))
                .collect::<Result<_, _>>()
                .map(Value::Object)
        }
        Some(b'[') => serde_json::from_str::<Vec<&serde_json::value::RawValue>>(raw.get())?
            .into_iter()
            .map(|value| raw_value_at(value, depth - 1))
            .collect::<Result<_, _>>()
            .map(Value::Array),
        _ => serde_json::from_str(raw.get()),
    }
}

fn normalized(bytes: &[u8]) -> Result<Vec<u8>, InputError> {
    document_text(bytes)
        .map(String::into_bytes)
        .map_err(|()| InputError("--cli-input-json: invalid JSON object"))
}

pub(crate) fn document_text(bytes: &[u8]) -> Result<String, ()> {
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

fn read_input(raw: &str) -> Result<Vec<u8>, InputError> {
    let Some(path) = raw.strip_prefix("file://") else {
        return Ok(raw.as_bytes().to_vec());
    };
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "linux")]
        const NONBLOCK: i32 = 0x800;
        #[cfg(not(target_os = "linux"))]
        const NONBLOCK: i32 = 0x4;
        options.custom_flags(NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| InputError("--cli-input-json: cannot open file"))?;
    if !file
        .metadata()
        .map_err(|_| InputError("--cli-input-json: cannot inspect file"))?
        .is_file()
    {
        return Err(InputError("--cli-input-json: file must be regular"));
    }
    let mut bytes = Vec::new();
    file.take((1 << 20) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| InputError("--cli-input-json: cannot read file"))?;
    if bytes.len() > 1 << 20 {
        return Err(InputError("--cli-input-json: file too large"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, Command};

    #[derive(Default)]
    struct Staged {
        name: String,
    }
    fn json_name(input: &mut Staged, value: Value) -> Result<(), ()> {
        input.name = value.as_str().ok_or(())?.to_owned();
        Ok(())
    }
    fn flag_name(input: &mut Staged, matches: &ArgMatches) -> Result<(), ()> {
        input.name = matches.get_one::<String>("name").ok_or(())?.clone();
        Ok(())
    }
    const FIELDS: &[InputField<Staged>] = &[
        InputField {
            json_name: "Name",
            flag: Some(Field {
                name: "name",
                go_type: "string",
                required: false,
            }),
            secret: false,
            nullable: false,
            set_json: json_name,
            set_flag: Some(flag_name),
            requirement: Requirement::Optional,
        },
        InputField {
            json_name: "Secret",
            flag: None,
            secret: true,
            nullable: false,
            set_json: json_name,
            set_flag: None,
            requirement: Requirement::Optional,
        },
    ];
    fn descriptor() -> InputDescriptor<Staged> {
        InputDescriptor {
            defaults: Staged::default,
            fields: FIELDS,
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: None,
            guard: None,
        }
    }
    fn matches(args: &[&str]) -> ArgMatches {
        Command::new("toy")
            .arg(Arg::new("name").long("name"))
            .try_get_matches_from(args)
            .unwrap()
    }
    #[test]
    fn raw_json_conversion_rejects_excessive_nesting() {
        let nested = format!("{}0{}", "[".repeat(256), "]".repeat(256));
        let raw: &serde_json::value::RawValue = serde_json::from_str(&nested).unwrap();
        assert!(raw_value(raw).is_err());
        let shallow: &serde_json::value::RawValue = serde_json::from_str("[[0]]").unwrap();
        assert!(raw_value(shallow).is_ok());
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let suffix = getrandom::u64().unwrap();
            let path = std::env::temp_dir().join(format!("greennode-input-{suffix:016x}"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unpaired_surrogates_and_invalid_utf8_become_replacement_characters() {
        let args = matches(&["toy"]);
        let parsed = parse(
            &descriptor(),
            &args,
            Some(r#"{"Name":"a\ud800b\udc00c"}"#),
            None,
        )
        .unwrap();
        assert_eq!(parsed.value.name, "a\u{fffd}b\u{fffd}c");
        let dir = TempDir::new();
        let path = dir.0.join("input.json");
        std::fs::write(&path, b"{\"Name\":\"a\xffb\\ud800\"}").unwrap();
        let raw = format!("file://{}", path.display());
        let parsed = parse(&descriptor(), &args, Some(&raw), None).unwrap();
        assert_eq!(parsed.value.name, "a\u{fffd}b\u{fffd}");
    }

    #[test]
    fn malformed_escape_reports_the_operation_message() {
        fn validate(bytes: &[u8]) -> Result<(), InputError> {
            serde_json::from_slice::<Value>(bytes)
                .map(|_| ())
                .map_err(|_| InputError("invalid toy input"))
        }
        let checks = JsonChecks {
            validate,
            read_error: |error| error,
            normalize: None,
        };
        let error = merge_checked(
            &descriptor(),
            &matches(&["toy"]),
            Some(r#"{"Name":"a\u12"}"#),
            None,
            Some(checks),
        )
        .err()
        .unwrap();
        assert_eq!(error, InputError("invalid toy input"));
    }

    #[test]
    fn input_number_rejects_private_number_object() {
        #[derive(Default)]
        struct NumberInput {
            page: i64,
        }
        const FIELDS: &[InputField<NumberInput>] = &[InputField {
            json_name: "Page",
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.page = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }];
        let desc = InputDescriptor {
            defaults: NumberInput::default,
            fields: FIELDS,
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: None,
            guard: None,
        };
        let args = matches(&["toy"]);
        assert!(
            parse(
                &desc,
                &args,
                Some(r#"{"Page":{"$serde_json::private::Number":"1"}}"#),
                None
            )
            .is_err()
        );
        assert_eq!(
            parse(&desc, &args, Some(r#"{"Page":1}"#), None)
                .unwrap()
                .value
                .page,
            1
        );
    }

    #[test]
    fn flags_override_json_and_track_presence() {
        let parsed = parse(
            &descriptor(),
            &matches(&["toy", "--name", "flag"]),
            Some(r#"{"Name":"json"}"#),
            None,
        )
        .unwrap();
        assert_eq!(parsed.value.name, "flag");
        assert!(parsed.present.contains("Name"));
        let parsed = parse(
            &descriptor(),
            &matches(&["toy"]),
            Some(r#"{"Name":null}"#),
            None,
        )
        .unwrap();
        assert!(parsed.present.is_empty());
    }
    #[test]
    fn secrets_and_invalid_json_are_refused() {
        let args = matches(&["toy"]);
        for raw in [
            r#"{"Secret":null}"#,
            r#"{"name":"x"}"#,
            r#"{"Name":"x"} {}"#,
        ] {
            assert!(parse(&descriptor(), &args, Some(raw), None).is_err());
        }
        assert!(parse(&descriptor(), &args, Some(r#"{"Name":"ok"}"#), None).is_ok());
    }
    #[test]
    fn argv_guard_precedes_opening_json_file() {
        fn guard(args: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
            if args
                .get_one::<String>("password-file")
                .is_some_and(|v| v == "-")
                && json.is_some_and(|v| v.starts_with("file://"))
            {
                return Err(InputError("conflicting input sources"));
            }
            Ok(())
        }
        let args = Command::new("toy")
            .arg(Arg::new("name").long("name"))
            .arg(Arg::new("password-file").long("password-file"))
            .try_get_matches_from(["toy", "--password-file", "-"])
            .unwrap();
        let mut desc = descriptor();
        desc.pre_guard = Some(guard);
        assert!(matches!(
            parse(&desc, &args, Some("file:///missing-input"), None),
            Err(InputError("conflicting input sources"))
        ));
        assert!(parse(&desc, &args, Some(r#"{"Name":"ok"}"#), None).is_ok());
    }

    #[test]
    fn project_binding_cannot_target_secret() {
        fn project(input: &mut Staged, value: &str) {
            input.name = value.to_owned();
        }
        let mut desc = descriptor();
        desc.project_binding = Some(("Secret", project));
        assert_eq!(
            validate_descriptor(&desc),
            Err(InputError("invalid project binding"))
        );
        desc.project_binding = Some(("Name", project));
        assert!(validate_descriptor(&desc).is_ok());
    }

    #[test]
    fn required_presence_and_descriptor_validation() {
        const REQUIRED: &[InputField<Staged>] = &[InputField {
            json_name: "Name",
            flag: Some(Field {
                name: "name",
                go_type: "string",
                required: true,
            }),
            secret: false,
            nullable: false,
            set_json: json_name,
            set_flag: Some(flag_name),
            requirement: Requirement::Presence,
        }];
        let mut desc = descriptor();
        desc.fields = REQUIRED;
        assert!(validate_descriptor(&desc).is_ok());
        let args = matches(&["toy"]);
        let missing = parse(&desc, &args, Some(r#"{"Name":null}"#), None).unwrap();
        assert!(check_required(&desc, &missing).is_err());
        let present = parse(&desc, &args, Some(r#"{"Name":""}"#), None).unwrap();
        assert!(check_required(&desc, &present).is_ok());
    }

    #[test]
    fn input_file_is_bounded_and_regular() {
        let path = std::env::temp_dir().join(format!("greennode-input-{}", std::process::id()));
        std::fs::write(&path, br#"{"Name":"ok"}"#).unwrap();
        let raw = format!("file://{}", path.display());
        assert!(parse(&descriptor(), &matches(&["toy"]), Some(&raw), None).is_ok());
        std::fs::write(&path, vec![b' '; (1 << 20) + 1]).unwrap();
        assert!(parse(&descriptor(), &matches(&["toy"]), Some(&raw), None).is_err());
        std::fs::remove_file(&path).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn fifo_input_is_refused_without_waiting_for_a_writer() {
        let path = std::env::temp_dir().join(format!("greennode-fifo-{}", std::process::id()));
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let raw = format!("file://{}", path.display());
        let result = parse(&descriptor(), &matches(&["toy"]), Some(&raw), None);
        std::fs::remove_file(path).unwrap();
        assert!(result.is_err());
    }
}
