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
pub struct Operation {
    pub name: &'static str,
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
