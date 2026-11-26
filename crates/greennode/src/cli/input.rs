use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Read;

use clap::{ArgMatches, parser::ValueSource};
use serde_json::Value;

use super::op::Field;

pub type FlagSetter<T> = fn(&mut T, &ArgMatches) -> Result<(), ()>;
pub type ProjectSetter<T> = fn(&mut T, &str);
pub type InputGuard<T> = fn(&T, &ArgMatches) -> Result<(), InputError>;

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
    let mut value = (descriptor.defaults)();
    let mut present = HashSet::new();
    if let Some(raw) = json.filter(|raw| !raw.is_empty()) {
        if !descriptor.allow_json {
            return Err(InputError("--cli-input-json is disabled"));
        }
        let bytes = read_input(raw)?;
        let input: Option<HashMap<String, Value>> = serde_json::from_slice(&bytes)
            .map_err(|_| InputError("--cli-input-json: invalid JSON object"))?;
        if let Some(input) = input {
            for (name, item) in input {
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
    if !descriptor.allow_query
        && matches.try_contains_id("query").unwrap_or(false)
        && matches.value_source("query") == Some(ValueSource::CommandLine)
    {
        return Err(InputError("--query is disabled"));
    }
    if let Some(guard) = descriptor.guard {
        guard(&value, matches)?;
    }
    Ok(ParsedOperationInput { value, present })
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
        && !names.contains(field)
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
