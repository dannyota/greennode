use zeroize::Zeroizing;

use super::decode;

pub(crate) fn inspect_error_message<T>(body: &[u8], inspect: impl FnOnce(&str) -> T) -> Option<T> {
    full_error(body).map(|(_, message)| inspect(&Zeroizing::new(message)))
}

pub(super) fn full_error(body: &[u8]) -> Option<(String, String)> {
    use crate::wire::strict_json::{Field, validate};

    const fn scalar(name: &str) -> Field<'_> {
        Field {
            name,
            children: &[],
        }
    }

    const SCALARS: &[Field<'_>] = &[
        scalar("message"),
        scalar("error"),
        scalar("detail"),
        scalar("title"),
        scalar("code"),
    ];
    let fields = [
        Field {
            name: "errors",
            children: SCALARS,
        },
        scalar("message"),
        scalar("error"),
        scalar("detail"),
        scalar("title"),
        scalar("code"),
    ];
    let text = crate::model::wire::go_json(body);
    validate(text.as_bytes(), &fields, text.len()).ok()?;
    Some(decode(text.as_bytes(), false))
}
