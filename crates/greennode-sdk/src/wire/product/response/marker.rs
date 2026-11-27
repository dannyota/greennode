use zeroize::Zeroizing;

pub(crate) fn has_credential_marker(text: &str) -> bool {
    has_credential_marker_bytes(text.as_bytes())
}

pub(super) fn has_credential_marker_bytes(bytes: &[u8]) -> bool {
    let mut normalized = Zeroizing::new(String::with_capacity(bytes.len()));
    for chunk in bytes.utf8_chunks() {
        normalized.extend(
            chunk
                .valid()
                .chars()
                .flat_map(char::to_lowercase)
                .filter(char::is_ascii_alphanumeric),
        );
    }
    normalized.contains("clientsecret")
}
