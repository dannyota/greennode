#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PathIds {
    #[default]
    Strict,
    WithUnderscore,
}

impl PathIds {
    pub(crate) fn allows(self, id: &str) -> bool {
        !id.is_empty()
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || byte == b'-'
                    || (byte == b'_' && self == Self::WithUnderscore)
            })
    }
}

const ALLOWED_HEADERS: &[&str] = &["user-type"];

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub(crate) path_ids: PathIds,
    pub(crate) headers: &'static [(&'static str, &'static str)],
}

impl Options {
    pub(crate) const DEFAULT: Self = Self {
        path_ids: PathIds::Strict,
        headers: &[],
    };

    pub(crate) fn apply_headers(&self, headers: &mut http::HeaderMap) -> Result<(), ()> {
        for (name, value) in self.headers {
            if !ALLOWED_HEADERS.contains(name) || value.is_empty() {
                return Err(());
            }
            let name = http::HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?;
            let value = http::HeaderValue::from_str(value).map_err(|_| ())?;
            if headers.contains_key(&name) {
                return Err(());
            }
            headers.insert(name, value);
        }
        Ok(())
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::DEFAULT
    }
}
