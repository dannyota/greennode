#[allow(dead_code)]
pub(crate) mod connection;
#[allow(dead_code)]
pub(crate) mod credential;
#[allow(dead_code)]
pub(crate) mod product;
#[allow(dead_code)]
mod send;
#[cfg(test)]
mod send_tests;
#[allow(dead_code)]
pub(crate) mod strict_json;
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)]
pub(crate) mod test_server;
#[cfg(test)]
mod tests;
