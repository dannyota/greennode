mod gendocs;
pub mod input;
pub mod op;
mod query;
pub mod render;
mod root;

pub use root::{command_with_registry, run};
