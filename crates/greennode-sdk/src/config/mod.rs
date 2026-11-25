#![allow(dead_code)]

mod credentials;
mod file;
mod ini;
mod mode;
mod settings;

#[derive(Clone, Copy)]
enum Slot {
    Config,
    Credentials,
}

impl Slot {
    fn label(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Credentials => "credentials",
        }
    }
}
