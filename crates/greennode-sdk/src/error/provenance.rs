use super::{ApiError, BodyTooLarge, CauseChain, Detail, Error, ErrorClass, SafeText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiProvenance {
    Response,
    Transport,
    BareOverflow,
    Local,
}

impl ApiProvenance {
    pub(super) fn from_fields(operation: &str, status: Option<u16>) -> Self {
        if operation.is_empty() && status.is_none() {
            Self::Local
        } else {
            Self::Response
        }
    }
}

impl ApiError {
    pub(crate) fn is_invalid_response(&self) -> bool {
        self.invalid_response
    }

    pub(crate) fn provenance(&self) -> ApiProvenance {
        self.provenance
    }

    /// Returns the public status of a classified HTTP response, when available.
    /// An observed status carried only as evidence for a bare body-limit failure is omitted.
    pub fn response_status(&self) -> Option<u16> {
        self.status()
            .filter(|_| self.provenance != ApiProvenance::BareOverflow)
    }

    pub(crate) fn observed_response_status(&self) -> Option<u16> {
        self.observed_status()
            .filter(|_| self.provenance != ApiProvenance::BareOverflow)
    }

    pub(crate) fn http_class(&self) -> Option<ErrorClass> {
        match self.observed_response_status()? {
            401 if !self.public_route => Some(ErrorClass::Auth),
            403 => Some(ErrorClass::PermissionDenied),
            404 => Some(ErrorClass::NotFound),
            429 => Some(ErrorClass::RateLimited),
            _ => None,
        }
    }

    pub(crate) fn fallback_code(status: Option<u16>) -> &'static str {
        match status {
            Some(400) => "BadRequest",
            Some(401) => "Unauthorized",
            Some(403) => "Forbidden",
            Some(404) => "NotFound",
            Some(409) => "Conflict",
            Some(429) => "Throttled",
            Some(500..) => "ServerError",
            Some(400..) => "ClientError",
            _ => "",
        }
    }
}

impl Error {
    pub(crate) fn copy_classified(&self) -> Self {
        let mut head = self.copy_classified_layer();
        let mut tail = &mut head;
        for layer in self.chain().skip(1) {
            tail = tail.cause.0.insert(Box::new(layer.copy_classified_layer()));
        }
        head
    }

    fn copy_classified_layer(&self) -> Self {
        let detail = self
            .detail
            .as_ref()
            .filter(|detail| detail.value().is::<BodyTooLarge>())
            .map(|_| Detail::new(BodyTooLarge));
        Self {
            class: self.class,
            identity: self.identity,
            fallback_message_reflected: self.fallback_message_reflected,
            display_reflected: self.display_reflected,
            debug_reflected: self.debug_reflected,
            api: self.api.clone(),
            login: self.login.clone(),
            service_login: self.service_login.clone(),
            detail,
            cause: CauseChain::default(),
        }
    }

    pub(crate) fn with_api_diagnostic(mut self, code: SafeText, message: SafeText) -> Self {
        let mut layer = Some(&mut self);
        while let Some(error) = layer {
            if let Some(api) = error.api.as_mut() {
                api.code = code.0;
                api.message = message.0;
                break;
            }
            layer = error.cause.0.as_deref_mut();
        }
        self
    }
}
