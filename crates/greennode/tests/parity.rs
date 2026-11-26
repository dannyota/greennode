use greennode::cli::op::{Field, InheritedBinding, Kind, Operation, Service};
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
            let actual_flags = operation
                .flags
                .iter()
                .chain(operation.extra_flags.iter())
                .chain(
                    operation
                        .inherited_bindings
                        .iter()
                        .map(|binding| &binding.flag),
                );
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
fn inherited_project_binding_matches_storage_baseline() {
    const USER: Field = Field {
        name: "user-key-id",
        go_type: "string",
        required: true,
    };
    const ACCOUNT: Field = Field {
        name: "service-account-id",
        go_type: "string",
        required: true,
    };
    const REGION: Field = Field {
        name: "Region",
        go_type: "string",
        required: false,
    };
    const PROJECT: InheritedBinding = InheritedBinding {
        flag: Field {
            name: "project-id",
            go_type: "string",
            required: true,
        },
        input_field: "ProjectID",
    };
    const ATTACH: Operation = Operation {
        name: "attach-s3-key",
        sdk_operation: "AttachS3Key",
        short_help: "Attach an S3 key",
        example: Some("storage attach-s3-key"),
        presence_required_fields: &["ProjectID"],
        inherited_bindings: &[PROJECT],
        kind: Kind::DestructiveWrite,
        flags: &[USER, ACCOUNT],
        json_fields: &[REGION],
        extra_flags: &[],
        notes: &[],
        handler: None,
    };
    let registry = Registry::new(&[Service {
        name: "storage",
        operations: &[ATTACH],
    }])
    .unwrap();
    let baseline: Baseline = serde_json::from_str(include_str!("parity/commands.json")).unwrap();
    assert!(compare(&registry, &baseline).is_ok());
    let tree = greennode::cli::command_with_registry(&registry);
    assert!(compare_tree(&registry, &baseline, &tree).is_ok());
}

fn compare_tree(
    registry: &Registry,
    baseline: &Baseline,
    tree: &clap::Command,
) -> Result<(), String> {
    compare(registry, baseline)?;
    for service in tree.get_subcommands() {
        for leaf in service.get_subcommands() {
            for arg in leaf.get_arguments() {
                if tree
                    .get_arguments()
                    .any(|global| global.is_global_set() && global.get_id() == arg.get_id())
                {
                    return Err(format!(
                        "redeclared global {} {}",
                        service.get_name(),
                        leaf.get_name()
                    ));
                }
            }
        }
    }
    let mut built = tree.clone();
    built.build();
    let tree = &built;
    let mut seen_services = std::collections::BTreeSet::new();
    for service_command in tree
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && !["version", "help", "configure"].contains(&c.get_name()))
    {
        let name = service_command.get_name();
        if !seen_services.insert(name) {
            return Err(format!("duplicate command {name}"));
        }
        let service = registry
            .services()
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("extra service {name}"))?;
        if service.operations.is_empty() {
            return Err(format!("empty service {name}"));
        }
        let mut seen_operations = std::collections::BTreeSet::new();
        for child in service_command
            .get_subcommands()
            .filter(|c| c.get_name() != "help" && !c.is_hide_set())
        {
            let op_name = child.get_name();
            if !seen_operations.insert(op_name) {
                return Err(format!("duplicate command {name} {op_name}"));
            }
            let operation = service
                .operations
                .iter()
                .find(|o| o.name == op_name)
                .ok_or_else(|| format!("extra operation {name} {op_name}"))?;
            let expected: std::collections::BTreeSet<_> = operation
                .flags
                .iter()
                .chain(operation.extra_flags)
                .map(|f| f.name)
                .collect();
            let mut actual = std::collections::BTreeSet::new();
            for arg in child.get_arguments() {
                let flag = arg
                    .get_long()
                    .ok_or_else(|| format!("unnamed or short-only argument {name} {op_name}"))?;
                if flag == "cli-input-json" {
                    continue;
                }
                if flag == "help" || greennode::services::GLOBAL_FLAGS.contains(&flag) {
                    let inherited = arg.is_global_set()
                        && tree.get_arguments().any(|root_arg| {
                            root_arg.get_long() == Some(flag)
                                && root_arg.is_global_set()
                                && root_arg.get_id() == arg.get_id()
                        });
                    let implicit_help = flag == "help"
                        && matches!(
                            arg.get_action(),
                            clap::ArgAction::Help
                                | clap::ArgAction::HelpLong
                                | clap::ArgAction::HelpShort
                        );
                    if inherited || implicit_help {
                        continue;
                    }
                    return Err(format!(
                        "non-inherited global or help flag {name} {op_name}"
                    ));
                }
                if !actual.insert(flag) {
                    return Err(format!("duplicate flag {name} {op_name}"));
                }
            }
            if actual != expected {
                return Err(format!("clap flags mismatch {name} {op_name}"));
            }
            for binding in operation.inherited_bindings {
                if !tree
                    .get_arguments()
                    .any(|arg| arg.get_long() == Some(binding.flag.name) && arg.is_global_set())
                {
                    return Err(format!("missing inherited flag {name} {op_name}"));
                }
            }
        }
        if seen_operations.len() != service.operations.len() {
            return Err(format!("missing operation in {name}"));
        }
    }
    if seen_services.len() != registry.services().len() {
        return Err("missing service".into());
    }
    Ok(())
}

fn toy_tree_with(arg: clap::Arg) -> clap::Command {
    clap::Command::new("greennode").subcommand(
        clap::Command::new("toy").subcommand(
            clap::Command::new("inspect")
                .arg(clap::Arg::new("name").long("name"))
                .arg(arg),
        ),
    )
}

#[test]
fn comparison_rejects_undeclared_positional() {
    assert!(
        compare_tree(
            &toy(),
            &baseline(),
            &toy_tree_with(clap::Arg::new("surprise"))
        )
        .is_err()
    );
}

#[test]
fn comparison_rejects_short_only_flag() {
    assert!(
        compare_tree(
            &toy(),
            &baseline(),
            &toy_tree_with(clap::Arg::new("surprise").short('x'))
        )
        .is_err()
    );
}

#[test]
fn comparison_rejects_global_id_redeclared_at_leaf() {
    let root = clap::Command::new("greennode").arg(
        clap::Arg::new("read-only")
            .long("read-only")
            .global(true)
            .action(clap::ArgAction::SetTrue),
    );
    let leaf = clap::Command::new("inspect").arg(clap::Arg::new("name").long("name"));
    let clean = root
        .clone()
        .subcommand(clap::Command::new("toy").subcommand(leaf.clone()));
    assert!(compare_tree(&toy(), &baseline(), &clean).is_ok());
    let bad = root.subcommand(
        clap::Command::new("toy").subcommand(
            leaf.arg(
                clap::Arg::new("read-only")
                    .long("read-only")
                    .global(true)
                    .value_parser(clap::value_parser!(String)),
            ),
        ),
    );
    assert!(compare_tree(&toy(), &baseline(), &bad).is_err());
}

#[test]
fn comparison_rejects_local_global_copy() {
    assert!(
        compare_tree(
            &toy(),
            &baseline(),
            &toy_tree_with(clap::Arg::new("profile").long("profile"))
        )
        .is_err()
    );
}

#[test]
fn comparison_rejects_missing_clap_flag() {
    let tree = clap::Command::new("greennode")
        .subcommand(clap::Command::new("toy").subcommand(clap::Command::new("inspect")));
    assert!(compare_tree(&toy(), &baseline(), &tree).is_err());
}

#[test]
fn comparison_rejects_extra_clap_flag() {
    let tree = clap::Command::new("greennode").subcommand(
        clap::Command::new("toy").subcommand(
            clap::Command::new("inspect")
                .arg(clap::Arg::new("name").long("name"))
                .arg(clap::Arg::new("extra").long("extra")),
        ),
    );
    assert!(compare_tree(&toy(), &baseline(), &tree).is_err());
}

#[test]
fn comparison_rejects_empty_registered_service() {
    let registry = Registry::new(&[Service {
        name: "toy",
        operations: &[],
    }])
    .unwrap();
    let tree = clap::Command::new("greennode").subcommand(clap::Command::new("toy"));
    assert!(compare_tree(&registry, &baseline(), &tree).is_err());
}

#[test]
fn registered_operations_match_baseline() {
    let baseline: Baseline = serde_json::from_str(include_str!("parity/commands.json")).unwrap();
    let registry = greennode::services::registered().unwrap();
    let tree = greennode::cli::command_with_registry(&registry);
    compare_tree(&registry, &baseline, &tree).unwrap();
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
    sdk_operation: "Inspect",
    short_help: "Inspect",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[FLAG],
    json_fields: &[JSON],
    extra_flags: &[],
    notes: &[],
    handler: None,
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
fn comparison_rejects_duplicate_baseline_operation() {
    let mut input = baseline();
    assert!(compare(&toy(), &input).is_ok());
    let op = input.services[0].operations[0].clone();
    input.services[0].operations.push(op);
    assert!(compare(&toy(), &input).is_err());
}

#[test]
fn comparison_rejects_unknown_registered_operation() {
    let mut input = baseline();
    assert!(compare(&toy(), &input).is_ok());
    input.services[0].operations.clear();
    assert!(compare(&toy(), &input).is_err());
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
