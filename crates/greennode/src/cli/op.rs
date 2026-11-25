#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Read,
    Write,
    DestructiveWrite,
    ReadLocalReplacement,
    WriteReplacement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub go_type: &'static str,
    pub required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedBinding {
    pub flag: Field,
    pub input_field: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operation {
    pub name: &'static str,
    pub sdk_operation: &'static str,
    pub short_help: &'static str,
    pub example: Option<&'static str>,
    pub presence_required_fields: &'static [&'static str],
    pub inherited_bindings: &'static [InheritedBinding],
    pub kind: Kind,
    pub flags: &'static [Field],
    pub json_fields: &'static [Field],
    pub extra_flags: &'static [Field],
    pub notes: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: &'static str,
    pub operations: &'static [Operation],
}
