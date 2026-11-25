use super::{Error, Value, value::check_depth};

pub fn redact_dynamic(value: &mut Value) -> Result<(), Error> {
    check_depth(value)?;
    redact(value);
    Ok(())
}

fn redact(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                redact(item);
            }
        }
        Value::Object(items) => {
            for (key, value) in items {
                let key: String = key
                    .chars()
                    .flat_map(char::to_lowercase)
                    .filter(char::is_ascii_alphanumeric)
                    .collect();
                if key == "auth"
                    || key == "auths"
                    || [
                        "password",
                        "passwd",
                        "passphrase",
                        "secret",
                        "token",
                        "credential",
                        "privatekey",
                        "apikey",
                        "authorization",
                        "accesskey",
                        "dockerconfig",
                    ]
                    .iter()
                    .any(|s| key.contains(s))
                {
                    *value = Value::String("<redacted>".into());
                } else {
                    redact(value);
                }
            }
        }
        _ => {}
    }
}
