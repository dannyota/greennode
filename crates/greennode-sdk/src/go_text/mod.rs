mod html;
mod quote;
mod tables;
#[cfg(test)]
mod tests;

pub(crate) use html::unescape_html;
pub(crate) use quote::go_quote;
