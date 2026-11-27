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

pub(crate) type Execute = for<'a> fn(
    SelectedOperation,
    &'a clap::ArgMatches,
    &'a mut super::Invocation<'_>,
) -> super::dispatch::FutureResult<'a, u8>;

#[derive(Clone, Copy, Debug)]
pub struct Handler {
    pub(crate) execute: Execute,
    pub(crate) allow_json: bool,
}

impl Handler {
    pub(crate) const fn new<T>(input: &super::input::InputDescriptor<T>, execute: Execute) -> Self {
        Self {
            execute,
            allow_json: input.allow_json,
        }
    }
}

#[derive(Clone, Copy, Debug)]
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
    pub handler: Option<Handler>,
}

#[derive(Clone, Copy, Debug)]
pub struct Service {
    pub name: &'static str,
    pub operations: &'static [Operation],
    pub error_rules: &'static [super::errors::Rule],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SelectedOperation {
    pub operation: &'static Operation,
    pub error_rules: &'static [super::errors::Rule],
}

pub(crate) fn argument(field: &Field) -> clap::Arg {
    use clap::{Arg, ArgAction};
    let arg = Arg::new(field.name).long(field.name);
    if field.name == "password-file" {
        return arg
            .value_parser(clap::builder::OsStringValueParser::new())
            .help(super::password::HELP);
    }
    match field.go_type.trim_start_matches('*') {
        "bool" => arg
            .action(ArgAction::Set)
            .num_args(0..=1)
            .require_equals(true)
            .default_missing_value("true")
            .value_parser(parse_bool),
        "[]string" => arg.action(ArgAction::Append),
        "stringArray" => arg.action(ArgAction::Append).value_name("stringArray"),
        "int" | "int64" => arg
            .action(ArgAction::Set)
            .allow_hyphen_values(true)
            .value_parser(parse_int),
        "float64" => arg
            .action(ArgAction::Set)
            .allow_hyphen_values(true)
            .value_parser(parse_float),
        _ => arg.action(ArgAction::Set),
    }
}

fn parse_bool(value: &str) -> Result<bool, &'static str> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err("invalid boolean"),
    }
}

fn parse_int(value: &str) -> Result<i64, &'static str> {
    let (negative, digits) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    let (radix, digits, prefixed) = if let Some(rest) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, rest, true)
    } else if let Some(rest) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        (8, rest, true)
    } else if let Some(rest) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        (2, rest, true)
    } else if digits.len() > 1 && digits.starts_with('0') {
        (8, digits, false)
    } else {
        (10, digits, false)
    };
    let mut previous_digit = prefixed;
    let mut any_digit = false;
    let mut number = 0_i128;
    for ch in digits.chars() {
        if ch == '_' {
            if !previous_digit {
                return Err("invalid integer");
            }
            previous_digit = false;
        } else {
            let digit = ch.to_digit(radix).ok_or("invalid integer")?;
            number = number
                .checked_mul(i128::from(radix))
                .and_then(|n| n.checked_add(i128::from(digit)))
                .ok_or("invalid integer")?;
            previous_digit = true;
            any_digit = true;
        }
    }
    if !previous_digit || !any_digit {
        return Err("invalid integer");
    }
    i64::try_from(if negative { -number } else { number }).map_err(|_| "invalid integer")
}

fn parse_float(value: &str) -> Result<f64, &'static str> {
    let number: f64 = value.parse().map_err(|_| "invalid number")?;
    let spelling = value.strip_prefix(['+', '-']).unwrap_or(value);
    if number.is_infinite()
        && !spelling.eq_ignore_ascii_case("inf")
        && !spelling.eq_ignore_ascii_case("infinity")
    {
        return Err("invalid number");
    }
    Ok(number)
}

#[cfg(test)]
mod tests {
    use super::{parse_float, parse_int};

    #[test]
    fn repeatable_string_array_preserves_occurrences_and_has_its_own_help_type() {
        use super::{Field, argument};
        let list = Field {
            name: "locations",
            go_type: "[]string",
            required: false,
        };
        let array = Field {
            name: "channel-id",
            go_type: "stringArray",
            required: false,
        };
        let mut cmd = clap::Command::new("check")
            .arg(argument(&list))
            .arg(argument(&array));
        let host_args = [
            "check",
            "--locations",
            "a,b",
            "--locations",
            "c",
            "--channel-id",
            "x,y",
            "--channel-id",
            "z",
        ];
        let values = cmd.clone().try_get_matches_from(host_args).unwrap();
        assert_eq!(
            values
                .get_many::<String>("locations")
                .unwrap()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["a,b", "c"]
        );
        assert_eq!(
            values
                .get_many::<String>("channel-id")
                .unwrap()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["x,y", "z"]
        );
        assert!(
            cmd.render_long_help()
                .to_string()
                .contains("--channel-id <stringArray>")
        );
        assert!(
            cmd.render_long_help()
                .to_string()
                .contains("--locations <locations>")
        );
    }

    #[test]
    fn floating_point_overflow_is_rejected() {
        assert!(parse_float("1e309").is_err());
        assert!(parse_float("-1e309").is_err());
        assert_eq!(parse_float("1e308"), Ok(1e308));
        assert_eq!(parse_float("Inf"), Ok(f64::INFINITY));
    }

    #[test]
    fn integers_follow_go_base_zero_rules() {
        for (text, expected) in [
            ("0xff", 255),
            ("-0o10", -8),
            ("0b_10", 2),
            ("012", 10),
            ("1_000", 1000),
            ("-9223372036854775808", i64::MIN),
        ] {
            assert_eq!(parse_int(text), Ok(expected));
        }
        for text in ["0x", "09", "1__0", "_1", "1_", "9223372036854775808"] {
            assert!(parse_int(text).is_err());
        }
    }
}
