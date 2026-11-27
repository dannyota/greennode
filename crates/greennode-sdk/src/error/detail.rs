use super::{Diagnostic, HttpStatus, RecoveryAdvice, SystemTagText};
use std::{any::Any, fmt::Debug};
use zeroize::Zeroizing;

type Value = dyn Any + Send + Sync;

trait Erased: Debug + Send + Sync {
    fn value(&self) -> &Value;
    fn value_mut(&mut self) -> &mut Value;
}

impl<T: Any + Send + Sync + Debug> Erased for T {
    fn value(&self) -> &Value {
        self
    }

    fn value_mut(&mut self) -> &mut Value {
        self
    }
}

pub(super) struct Detail {
    value: Box<dyn Erased>,
    withheld: bool,
}

impl Detail {
    pub(super) fn new(value: impl Any + Send + Sync + Debug) -> Self {
        let value: Box<dyn Erased> = match (&value as &dyn Any).downcast_ref::<http::StatusCode>() {
            Some(status) => Box::new(HttpStatus(*status)),
            None => Box::new(value),
        };
        Self {
            value,
            withheld: false,
        }
    }

    pub(super) fn value(&self) -> &Value {
        self.value.as_ref().value()
    }

    pub(super) fn public<T: Any>(&self) -> Option<&T> {
        (!self.withheld)
            .then(|| self.value().downcast_ref())
            .flatten()
    }

    pub(super) fn screen_fields(&mut self, reflects: &impl Fn(&str) -> bool) {
        let value = self.value.as_mut().value_mut();
        if let Some(text) = value.downcast_mut::<&'static str>()
            && reflects(text)
        {
            *text = "";
        }
        if let Some(text) = value.downcast_mut::<String>()
            && reflects(text)
        {
            text.clear();
        }
        if let Some(advice) = value.downcast_mut::<RecoveryAdvice>()
            && reflects(advice.message())
        {
            advice.0.0.clear();
        }
        if let Some(text) = value.downcast_mut::<SystemTagText>()
            && reflects(text.message())
        {
            text.0.clear();
        }
        self.withheld |= value
            .downcast_ref::<Diagnostic>()
            .is_some_and(|value| reflects(value.message()))
            || value
                .downcast_ref::<HttpStatus>()
                .is_some_and(|value| reflects(value.0.as_str()));
    }

    pub(super) fn screen_public(&mut self, reflects: &impl Fn(&str) -> bool) {
        let value = self.value.as_mut().value_mut();
        if let Some(arithmetic) = value.downcast_mut::<crate::pricing::EstimateArithmeticError>() {
            if reflects(arithmetic.stage) {
                arithmetic.stage = "";
            }
            if reflects(&arithmetic.item_key) {
                arithmetic.item_key = "<redacted>".into();
            }
            self.withheld |= reflects(arithmetic.stage) || reflects(&arithmetic.item_key);
        }
        self.withheld |= debug_reflects(self.value.as_ref(), reflects);
    }
}

pub(crate) fn debug_reflects(
    value: &(impl Debug + ?Sized),
    reflects: &impl Fn(&str) -> bool,
) -> bool {
    [false, true]
        .into_iter()
        .any(|alternate| render_debug(value, alternate).is_none_or(|text| reflects(&text)))
}

pub(crate) fn write_debug(
    formatter: &mut std::fmt::Formatter<'_>,
    fields: impl Fn(&mut std::fmt::Formatter<'_>) -> std::fmt::Result,
) -> std::fmt::Result {
    struct Neutral<F>(F);
    impl<F: Fn(&mut std::fmt::Formatter<'_>) -> std::fmt::Result> Debug for Neutral<F> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            (self.0)(formatter)
        }
    }
    if formatter.alternate() {
        write!(formatter, "{:#?}", Neutral(fields))
    } else {
        write!(formatter, "{:?}", Neutral(fields))
    }
}

fn render_debug(value: &(impl Debug + ?Sized), alternate: bool) -> Option<Zeroizing<String>> {
    use std::fmt::{self, Write};
    struct Buffer(Zeroizing<String>);
    impl Write for Buffer {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            if self.0.len().saturating_add(text.len()) > 64 << 20 {
                return Err(fmt::Error);
            }
            self.0.push_str(text);
            Ok(())
        }
    }
    let mut buffer = Buffer(Zeroizing::new(String::new()));
    if alternate {
        write!(buffer, "{value:#?}").ok()?;
    } else {
        write!(buffer, "{value:?}").ok()?;
    }
    Some(buffer.0)
}

macro_rules! debug_record {
    ($name:ident $(<$item:ident>)? { $($field:ident),* $(,)? }) => {
        impl $(<$item: std::fmt::Debug>)? std::fmt::Debug for $name $(<$item>)? {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                crate::error::write_debug(formatter, |f| {
                    let Self { $($field,)* } = self;
                    f.debug_struct(stringify!($name))
                        $(.field(stringify!($field).trim_start_matches("r#"), $field))*
                        .finish()
                })
            }
        }
    };
    ($name:ident; $($variant:ident),* $(,)?) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(match self { $(Self::$variant => stringify!($variant),)* })
            }
        }
    };
    ($name:ident<$item:ident>; $value:ident, $empty:ident) => {
        impl<$item: std::fmt::Debug> std::fmt::Debug for $name<$item> {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                crate::error::write_debug(formatter, |f| match self {
                    Self::$value(value) => f.debug_tuple(stringify!($value)).field(value).finish(),
                    Self::$empty => f.write_str(stringify!($empty)),
                })
            }
        }
    };
    ($name:ident [ $($variant:ident { $($field:ident),* }),* $(,)? ]) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                crate::error::write_debug(formatter, |f| match self {
                    $(Self::$variant { $($field),* } => f.debug_struct(stringify!($variant))
                        $(.field(stringify!($field), $field))*
                        .finish(),)*
                })
            }
        }
    };
    ($name:ident $(<$item:ident>)? => |$value:ident, $formatter:ident| $body:block) => {
        impl $(<$item>)? std::fmt::Debug for $name $(<$item>)? {
            fn fmt(&$value, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                crate::error::write_debug(formatter, |$formatter| $body)
            }
        }
    };
    ($name:ident unit) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(stringify!($name))
            }
        }
    };
    ($name:ident ($field:tt)) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                crate::error::write_debug(formatter, |f| {
                    f.debug_tuple(stringify!($name)).field(&self.$field).finish()
                })
            }
        }
    };
}

pub(crate) use debug_record;
