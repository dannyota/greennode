pub(crate) fn has_credential_marker(text: &str) -> bool {
    let normalized: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .filter(char::is_ascii_alphanumeric)
        .collect();
    normalized.contains("clientsecret")
}
