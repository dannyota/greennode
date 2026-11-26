use crate::cli::input::InputError;
use serde::de::{DeserializeSeed, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use std::{cell::Cell, collections::HashSet, fmt};

#[derive(Clone, Copy)]
enum Kind {
    Object(&'static [Field]),
    List(&'static Field),
    String,
    Integer,
    Boolean,
}

struct Field {
    name: &'static str,
    error: &'static str,
    kind: Kind,
}

macro_rules! field {
    ($prefix:expr, $name:literal, $kind:expr) => {
        Field {
            name: $name,
            error: concat!(
                "--cli-input-json: invalid estimate field at ",
                $prefix,
                $name
            ),
            kind: $kind,
        }
    };
}

macro_rules! row {
    ($path:literal, $($name:literal => $kind:expr),+ $(,)?) => {
        Field { name: "", error: concat!("--cli-input-json: invalid estimate field at ", $path), kind: Kind::Object(&[ $(field!(concat!($path, "."), $name, $kind)),+ ]) }
    };
}

const SERVER: Field = row!("setup.Servers[]",
    "Key" => Kind::String, "ZoneID" => Kind::String, "FlavorID" => Kind::String, "ImageID" => Kind::String,
    "Quantity" => Kind::Integer, "RootDiskSize" => Kind::Integer, "RootDiskTypeID" => Kind::String,
    "RootDiskEncryptionTypeID" => Kind::String, "DataDiskSize" => Kind::Integer,
    "DataDiskTypeID" => Kind::String, "DataDiskEncryptionTypeID" => Kind::String,
);
const VOLUME: Field = row!("setup.Volumes[]",
    "Key" => Kind::String, "ServerKey" => Kind::String, "ZoneID" => Kind::String,
    "VolumeTypeID" => Kind::String, "Quantity" => Kind::Integer, "Size" => Kind::Integer,
    "EncryptionTypeID" => Kind::String,
);
const IP: Field = row!("setup.PublicIPs[]",
    "Key" => Kind::String, "Kind" => Kind::String, "Type" => Kind::String, "Quantity" => Kind::Integer,
);
const STORAGE: Field = row!("setup.Storage[]",
    "Key" => Kind::String, "Region" => Kind::String, "Type" => Kind::String,
    "Quantity" => Kind::Integer, "QuotaGB" => Kind::Integer,
);
const ROOT: Field = field!(
    "",
    "setup",
    Kind::Object(&[
        field!("setup.", "Servers", Kind::List(&SERVER)),
        field!("setup.", "Volumes", Kind::List(&VOLUME)),
        field!("setup.", "PublicIPs", Kind::List(&IP)),
        field!("setup.", "Storage", Kind::List(&STORAGE)),
        field!("setup.", "MaxRequests", Kind::Integer),
        field!("setup.", "AllowPartial", Kind::Boolean),
    ])
);

pub(super) fn validate(bytes: &[u8]) -> Result<(), InputError> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let error = Cell::new(ROOT.error);
    Check {
        field: &ROOT,
        error: &error,
    }
    .deserialize(&mut decoder)
    .map_err(|_| InputError(error.get()))?;
    decoder.end().map_err(|_| InputError(ROOT.error))
}

struct Check<'a> {
    field: &'static Field,
    error: &'a Cell<&'static str>,
}

impl<'de> DeserializeSeed<'de> for Check<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        self.error.set(self.field.error);
        decoder.deserialize_any(self)
    }
}

fn shape_error<E: Error>() -> E {
    E::custom("invalid estimate shape")
}

impl<'de> Visitor<'de> for Check<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an estimate field")
    }

    fn visit_unit<E: Error>(self) -> Result<(), E> {
        if self.field.name == "setup" {
            Err(shape_error())
        } else {
            Ok(())
        }
    }

    fn visit_str<E: Error>(self, _value: &str) -> Result<(), E> {
        if matches!(self.field.kind, Kind::String) {
            Ok(())
        } else {
            Err(shape_error())
        }
    }

    fn visit_bool<E: Error>(self, _value: bool) -> Result<(), E> {
        if matches!(self.field.kind, Kind::Boolean) {
            Ok(())
        } else {
            Err(shape_error())
        }
    }

    fn visit_i64<E: Error>(self, _value: i64) -> Result<(), E> {
        if matches!(self.field.kind, Kind::Integer) {
            Ok(())
        } else {
            Err(shape_error())
        }
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<(), E> {
        self.visit_i64(i64::try_from(value).map_err(|_| shape_error::<E>())?)
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let Kind::Object(fields) = self.field.kind else {
            return Err(shape_error());
        };
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            let field = fields
                .iter()
                .find(|field| field.name == key)
                .ok_or_else(shape_error)?;
            self.error.set(field.error);
            if !seen.insert(field.name) {
                return Err(shape_error());
            }
            map.next_value_seed(Check {
                field,
                error: self.error,
            })?;
            self.error.set(self.field.error);
        }
        Ok(())
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
        let Kind::List(field) = self.field.kind else {
            return Err(shape_error());
        };
        while seq
            .next_element_seed(Check {
                field,
                error: self.error,
            })?
            .is_some()
        {
            self.error.set(self.field.error);
        }
        Ok(())
    }
}
