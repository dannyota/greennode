mod catalog;
mod notification;
mod quotes;
mod status;
mod wire;

pub(super) use wire::integer;

pub use catalog::*;
pub use notification::*;
pub use quotes::*;
pub use status::*;

use std::collections::HashSet;
use std::hash::Hash;

use super::ResponseField;
use crate::model::wire::WireField;
use crate::wire::product::evidence::Evidence;

trait Screen {
    fn screen(&mut self, evidence: &Evidence) -> bool;
}

impl Screen for String {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        self.redact(evidence)
    }
}

impl<T: WireField> Screen for ResponseField<T> {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        self.redact(evidence)
    }
}

impl<T: Screen> Screen for Vec<T> {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        self.iter_mut().all(|item| item.screen(evidence))
    }
}

impl<T: Screen> Screen for Option<T> {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        self.as_mut().map_or_else(
            || !evidence.text_reflects("null"),
            |value| value.screen(evidence),
        )
    }
}

fn unique<T: Eq + Hash>(values: impl IntoIterator<Item = T>) -> bool {
    let mut seen = HashSet::new();
    values.into_iter().all(|value| seen.insert(value))
}

macro_rules! output {
    ($(#[$meta:meta])* struct $model:ident {
        $($(#[$field_meta:meta])* $field:ident: $ty:ty => $name:literal,)*
    }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, serde::Serialize)]
        pub struct $model {
            $($(#[$field_meta])* #[serde(rename = $name)] pub $field: $ty,)*
        }

        impl super::Screen for $model {
            fn screen(&mut self, evidence: &$crate::wire::product::evidence::Evidence) -> bool {
                $(super::Screen::screen(&mut self.$field, evidence) &&)* true
            }
        }
    };
}

use output;
