use serde::{Serialize, Serializer};
use zeroize::Zeroizing;

/// A string that is zeroized on drop and redacted in debug and serialized output.
#[derive(Clone)]
pub struct Secret(std::sync::Arc<Zeroizing<String>>);

impl Secret {
    /// Takes ownership of the supplied string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(std::sync::Arc::new(Zeroizing::new(value.into())))
    }

    /// Returns the plaintext; callers must protect it from logs and disclosure.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

crate::error::debug_record!(Secret => |self, f| {
    f.write_str("[redacted]")
});

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_secret_storage() {
        let secret = Secret::new("toy-secret");
        let clone = secret.clone();
        assert!(std::ptr::eq(
            secret.expose_secret().as_ptr(),
            clone.expose_secret().as_ptr()
        ));
    }
}
