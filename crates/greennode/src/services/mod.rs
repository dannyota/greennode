use crate::cli::op::Service;

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
    DuplicateService(&'static str),
    DuplicateOperation(&'static str, &'static str),
    GlobalFlagCollision(&'static str, &'static str, &'static str),
}

pub struct Registry {
    services: &'static [Service],
}

impl Registry {
    pub fn new(services: &'static [Service]) -> Result<Self, RegistryError> {
        let mut service_names = std::collections::HashSet::new();
        for service in services {
            if !service_names.insert(service.name) {
                return Err(RegistryError::DuplicateService(service.name));
            }
            let mut operation_names = std::collections::HashSet::new();
            for operation in service.operations {
                if !operation_names.insert(operation.name) {
                    return Err(RegistryError::DuplicateOperation(
                        service.name,
                        operation.name,
                    ));
                }
                for flag in operation.flags.iter().chain(operation.extra_flags) {
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
}

pub fn registered() -> Result<Registry, RegistryError> {
    Registry::new(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::op::{Field, Kind, Operation};

    const OP: Operation = Operation {
        name: "inspect",
        kind: Kind::Read,
        flags: &[],
        json_fields: &[],
        extra_flags: &[],
        notes: &[],
    };
    const TOY: Service = Service {
        name: "toy",
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
            operations: &[OP, OP],
        };
        assert!(matches!(
            Registry::new(&[DUP]),
            Err(RegistryError::DuplicateOperation("toy", "inspect"))
        ));
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
                operations: &[BAD_OP]
            }]),
            Err(RegistryError::GlobalFlagCollision(
                "toy", "inspect", "profile"
            ))
        ));
        assert!(matches!(
            Registry::new(&[Service {
                name: "toy",
                operations: &[BAD_EXTRA]
            }]),
            Err(RegistryError::GlobalFlagCollision(
                "toy", "inspect", "profile"
            ))
        ));
    }
}
