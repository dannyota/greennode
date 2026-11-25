use greennode::cli::op::{Field, Kind, Operation, Service};
use greennode::services::Registry;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct BaselineField {
    name: String,
    #[serde(rename = "type")]
    go_type: String,
    required: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct BaselineOperation {
    name: String,
    kind: String,
    flags: Vec<BaselineField>,
    #[serde(default)]
    json_fields: Vec<BaselineField>,
}

#[derive(Clone, Debug, Deserialize)]
struct BaselineService {
    name: String,
    operations: Vec<BaselineOperation>,
}

#[derive(Deserialize)]
struct Baseline {
    services: Vec<BaselineService>,
}

fn compare(registry: &Registry, baseline: &Baseline) -> Result<(), String> {
    for service in registry.services() {
        let mut services = baseline.services.iter().filter(|s| s.name == service.name);
        let reference = services
            .next()
            .ok_or_else(|| format!("unknown service {}", service.name))?;
        if services.next().is_some() {
            return Err(format!("duplicate service {}", service.name));
        }
        for operation in service.operations {
            let mut operations = reference
                .operations
                .iter()
                .filter(|o| o.name == operation.name);
            let expected = operations
                .next()
                .ok_or_else(|| format!("unknown operation {} {}", service.name, operation.name))?;
            if operations.next().is_some() {
                return Err(format!(
                    "duplicate operation {} {}",
                    service.name, operation.name
                ));
            }
            let kind = match operation.kind {
                Kind::Read => "Read",
                Kind::Write => "Write",
                Kind::DestructiveWrite => "Write, destructive",
                Kind::ReadLocalReplacement => "Read, local replacement requires --yes",
                Kind::WriteReplacement => "Write, replacement requires --yes",
            };
            if kind != expected.kind {
                return Err(format!("kind mismatch {} {}", service.name, operation.name));
            }
            let actual_flags = operation.flags.iter().chain(operation.extra_flags.iter());
            if !fields_match(actual_flags, &expected.flags)
                || !fields_match(operation.json_fields.iter(), &expected.json_fields)
            {
                return Err(format!(
                    "fields mismatch {} {}",
                    service.name, operation.name
                ));
            }
        }
    }
    Ok(())
}

fn fields_match<'a>(actual: impl Iterator<Item = &'a Field>, expected: &[BaselineField]) -> bool {
    let mut fields = std::collections::BTreeMap::new();
    for field in actual {
        if fields
            .insert(field.name, (field.go_type, field.required))
            .is_some()
        {
            return false;
        }
    }
    if fields.len() != expected.len() {
        return false;
    }
    for field in expected {
        if fields.remove(field.name.as_str()) != Some((field.go_type.as_str(), field.required)) {
            return false;
        }
    }
    fields.is_empty()
}

#[test]
fn registered_operations_match_baseline() {
    let baseline: Baseline = serde_json::from_str(include_str!("parity/commands.json")).unwrap();
    compare(&greennode::services::registered().unwrap(), &baseline).unwrap();
}

const FLAG: Field = Field {
    name: "name",
    go_type: "string",
    required: true,
};
const JSON: Field = Field {
    name: "Items",
    go_type: "[]string",
    required: false,
};
const OP: Operation = Operation {
    name: "inspect",
    kind: Kind::Read,
    flags: &[FLAG],
    json_fields: &[JSON],
    extra_flags: &[],
    notes: &[],
};

fn toy() -> Registry {
    Registry::new(&[Service {
        name: "toy",
        operations: &[OP],
    }])
    .unwrap()
}

fn baseline() -> Baseline {
    serde_json::from_str(r#"{"services":[{"name":"toy","operations":[{"name":"inspect","kind":"Read","flags":[{"name":"name","type":"string","required":true}],"json_fields":[{"name":"Items","type":"[]string","required":false}]}]}]}"#).unwrap()
}

#[test]
fn comparison_rejects_kind_mismatch() {
    assert!(compare(&toy(), &baseline()).is_ok());
    let mut input = baseline();
    input.services[0].operations[0].kind = "Write".into();
    assert!(compare(&toy(), &input).is_err());
}

#[test]
fn comparison_rejects_flag_type_mismatch() {
    assert!(compare(&toy(), &baseline()).is_ok());
    let mut input = baseline();
    input.services[0].operations[0].flags[0].go_type = "int".into();
    assert!(compare(&toy(), &input).is_err());
}

#[test]
fn comparison_rejects_flag_required_mismatch() {
    assert!(compare(&toy(), &baseline()).is_ok());
    let mut input = baseline();
    input.services[0].operations[0].flags[0].required = false;
    assert!(compare(&toy(), &input).is_err());
}

#[test]
fn comparison_rejects_json_field_mismatch() {
    assert!(compare(&toy(), &baseline()).is_ok());
    let mut input = baseline();
    input.services[0].operations[0].json_fields[0].name = "Other".into();
    assert!(compare(&toy(), &input).is_err());
}
