mod backup;
mod billing;
mod cdn;
mod containerregistry;
mod database;
mod dns;
mod globalloadbalancer;
mod iam;
mod portal;
mod pricing;
mod project;
mod tagging;
mod vks;
mod volume;

use crate::cli::op::{Handler, SelectedOperation, Service};

pub const GLOBAL_FLAGS: &[&str] = &[
    "profile",
    "region",
    "project-id",
    "output",
    "query",
    "yes",
    "read-only",
    "debug",
];

#[derive(Debug, PartialEq, Eq)]
pub enum RegistryError {
    InvalidErrorRules(&'static str),
    DuplicateService(&'static str),
    DuplicateOperation(&'static str, &'static str),
    GlobalFlagCollision(&'static str, &'static str, &'static str),
    InvalidName(&'static str),
    ReservedName(&'static str),
    DuplicateFlag(&'static str, &'static str, &'static str),
    InvalidInheritedBinding(&'static str),
}

pub struct Registry {
    services: &'static [Service],
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

impl Registry {
    pub fn new(services: &'static [Service]) -> Result<Self, RegistryError> {
        let mut service_names = std::collections::HashSet::new();
        for service in services {
            if [
                "version",
                "help",
                "configure",
                "gen-docs",
                "__command-inventory",
            ]
            .contains(&service.name)
            {
                return Err(RegistryError::ReservedName(service.name));
            }
            if !valid_name(service.name) {
                return Err(RegistryError::InvalidName(service.name));
            }
            if !service_names.insert(service.name) {
                return Err(RegistryError::DuplicateService(service.name));
            }
            if !crate::cli::errors::validate_rules(service.error_rules) {
                return Err(RegistryError::InvalidErrorRules(service.name));
            }
            let mut operation_names = std::collections::HashSet::new();
            for operation in service.operations {
                if operation.name == "help" {
                    return Err(RegistryError::ReservedName(operation.name));
                }
                if !valid_name(operation.name) {
                    return Err(RegistryError::InvalidName(operation.name));
                }
                if !operation_names.insert(operation.name) {
                    return Err(RegistryError::DuplicateOperation(
                        service.name,
                        operation.name,
                    ));
                }
                let mut flag_names = std::collections::HashSet::new();
                for binding in operation.inherited_bindings {
                    if !GLOBAL_FLAGS.contains(&binding.flag.name)
                        || binding.input_field.is_empty()
                        || !flag_names.insert(binding.flag.name)
                    {
                        return Err(RegistryError::InvalidInheritedBinding(binding.flag.name));
                    }
                }
                for flag in operation.flags.iter().chain(operation.extra_flags) {
                    if flag.name == "help" || flag.name == "cli-input-json" {
                        return Err(RegistryError::ReservedName(flag.name));
                    }
                    if !valid_name(flag.name) {
                        return Err(RegistryError::InvalidName(flag.name));
                    }
                    if !flag_names.insert(flag.name) {
                        return Err(RegistryError::DuplicateFlag(
                            service.name,
                            operation.name,
                            flag.name,
                        ));
                    }
                    if GLOBAL_FLAGS.contains(&flag.name) {
                        return Err(RegistryError::GlobalFlagCollision(
                            service.name,
                            operation.name,
                            flag.name,
                        ));
                    }
                }
            }
        }
        Ok(Self { services })
    }

    pub fn services(&self) -> &'static [Service] {
        self.services
    }

    pub(crate) fn select<'a>(
        &self,
        matches: &'a clap::ArgMatches,
    ) -> Option<(SelectedOperation, Handler, &'a clap::ArgMatches)> {
        let (service_name, service_matches) = matches.subcommand()?;
        let (operation_name, operation_matches) = service_matches.subcommand()?;
        let service = self
            .services
            .iter()
            .find(|service| service.name == service_name)?;
        let operation = service
            .operations
            .iter()
            .find(|operation| operation.name == operation_name)?;
        Some((
            SelectedOperation {
                operation,
                error_rules: service.error_rules,
            },
            operation.handler?,
            operation_matches,
        ))
    }
}

pub fn registered() -> Result<Registry, RegistryError> {
    Registry::new(&[
        backup::SERVICE,
        billing::SERVICE,
        cdn::SERVICE,
        containerregistry::SERVICE,
        database::SERVICE,
        dns::SERVICE,
        globalloadbalancer::SERVICE,
        iam::SERVICE,
        portal::SERVICE,
        pricing::SERVICE,
        project::SERVICE,
        tagging::SERVICE,
        vks::SERVICE,
        volume::SERVICE,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::op::{Field, Kind, Operation};

    const OP: Operation = Operation {
        name: "inspect",
        sdk_operation: "Inspect",
        short_help: "Inspect",
        example: None,
        presence_required_fields: &[],
        inherited_bindings: &[],
        kind: Kind::Read,
        flags: &[],
        json_fields: &[],
        extra_flags: &[],
        notes: &[],
        handler: None,
    };
    const TOY: Service = Service {
        name: "toy",
        error_rules: &[],
        operations: &[OP],
    };

    #[test]
    fn duplicate_service_is_rejected() {
        assert!(Registry::new(&[TOY]).is_ok());
        assert!(matches!(
            Registry::new(&[TOY, TOY]),
            Err(RegistryError::DuplicateService("toy"))
        ));
    }

    #[test]
    fn duplicate_operation_is_rejected() {
        assert!(Registry::new(&[TOY]).is_ok());
        const DUP: Service = Service {
            name: "toy",
            error_rules: &[],
            operations: &[OP, OP],
        };
        assert!(matches!(
            Registry::new(&[DUP]),
            Err(RegistryError::DuplicateOperation("toy", "inspect"))
        ));
    }

    #[test]
    fn reserved_service_names_are_rejected() {
        assert!(Registry::new(&[TOY]).is_ok());
        for services in [
            &[Service {
                name: "version",
                ..TOY
            }][..],
            &[Service {
                name: "help",
                ..TOY
            }],
            &[Service {
                name: "configure",
                ..TOY
            }],
            &[Service {
                name: "__command-inventory",
                ..TOY
            }],
        ] {
            assert!(Registry::new(services).is_err());
        }
    }

    #[test]
    fn reserved_leaf_name_is_rejected() {
        assert!(Registry::new(&[TOY]).is_ok());
        const BAD: Operation = Operation { name: "help", ..OP };
        assert!(
            Registry::new(&[Service {
                name: "toy",
                error_rules: &[],
                operations: &[BAD]
            }])
            .is_err()
        );
    }

    #[test]
    fn malformed_names_are_rejected() {
        assert!(Registry::new(&[TOY]).is_ok());
        const BAD_OP: Operation = Operation {
            name: "Bad--Name",
            ..OP
        };
        const BAD_FLAG: Field = Field {
            name: "Bad_Name",
            go_type: "string",
            required: false,
        };
        const BAD_FIELD_OP: Operation = Operation {
            flags: &[BAD_FLAG],
            ..OP
        };
        for services in [
            &[Service {
                name: "Bad-Name",
                ..TOY
            }][..],
            &[Service {
                name: "toy",
                error_rules: &[],
                operations: &[BAD_OP],
            }],
            &[Service {
                name: "toy",
                error_rules: &[],
                operations: &[BAD_FIELD_OP],
            }],
        ] {
            assert!(Registry::new(services).is_err());
        }
    }

    #[test]
    fn inherited_binding_must_name_a_global_flag() {
        use crate::cli::op::InheritedBinding;
        const INVALID: InheritedBinding = InheritedBinding {
            flag: Field {
                name: "unknown",
                go_type: "string",
                required: false,
            },
            input_field: "ProjectID",
        };
        const BAD: Operation = Operation {
            inherited_bindings: &[INVALID],
            ..OP
        };
        assert!(Registry::new(&[TOY]).is_ok());
        assert!(
            Registry::new(&[Service {
                name: "toy",
                error_rules: &[],
                operations: &[BAD]
            }])
            .is_err()
        );
    }

    #[test]
    fn duplicate_flags_are_rejected() {
        assert!(Registry::new(&[TOY]).is_ok());
        const FLAG: Field = Field {
            name: "limit",
            go_type: "int",
            required: false,
        };
        const BAD: Operation = Operation {
            flags: &[FLAG],
            extra_flags: &[FLAG],
            ..OP
        };
        assert!(
            Registry::new(&[Service {
                name: "toy",
                error_rules: &[],
                operations: &[BAD]
            }])
            .is_err()
        );
    }

    #[test]
    fn global_flags_are_rejected_in_both_flag_lists() {
        assert!(Registry::new(&[TOY]).is_ok());
        const BAD: Field = Field {
            name: "profile",
            go_type: "string",
            required: false,
        };
        const BAD_OP: Operation = Operation {
            flags: &[BAD],
            ..OP
        };
        const BAD_EXTRA: Operation = Operation {
            extra_flags: &[BAD],
            ..OP
        };
        assert!(matches!(
            Registry::new(&[Service {
                name: "toy",
                error_rules: &[],
                operations: &[BAD_OP]
            }]),
            Err(RegistryError::GlobalFlagCollision(
                "toy", "inspect", "profile"
            ))
        ));
        assert!(matches!(
            Registry::new(&[Service {
                name: "toy",
                error_rules: &[],
                operations: &[BAD_EXTRA]
            }]),
            Err(RegistryError::GlobalFlagCollision(
                "toy", "inspect", "profile"
            ))
        ));
    }
}
