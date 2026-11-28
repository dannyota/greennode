use super::{BearerToken, Error, Request, Secret, SentCredentials};
use http::HeaderMap;

pub(super) fn check_history(credentials: &[&SentCredentials]) -> Result<(), Error> {
    if credentials.iter().any(|value| value.history_exhausted()) {
        return Err(Error::Provider(Box::new(crate::Error::new(
            crate::ErrorClass::InvalidInput,
        ))));
    }
    Ok(())
}

pub(super) fn active_request_context(error: Error) -> Error {
    match error {
        Error::Context(end) => Error::ActiveRequestContext(end),
        error => error,
    }
}

fn credential_value(secret: &Secret, bearer: bool) -> Result<http::HeaderValue, Error> {
    let value = secret.expose_secret();
    if value.is_empty()
        || (bearer
            && value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control()))
    {
        return Err(Error::InvalidRequest);
    }
    let text = zeroize::Zeroizing::new(if bearer {
        format!("Bearer {value}")
    } else {
        value.to_owned()
    });
    let mut header = http::HeaderValue::from_str(&text).map_err(|_| Error::InvalidRequest)?;
    header.set_sensitive(true);
    Ok(header)
}

pub(super) fn attempt_headers(
    request: &Request,
    bearer: Option<&BearerToken>,
) -> Result<(HeaderMap, SentCredentials), Error> {
    let mut headers = request.headers.clone();
    let mut credentials = SentCredentials::default();
    let declared = request.credentials.iter().map(|credential| {
        (
            credential.name.clone(),
            &credential.value,
            credential.bearer,
        )
    });
    let authorization = bearer.map(|bearer| {
        (
            http::header::AUTHORIZATION,
            &bearer.token.access_token,
            true,
        )
    });
    for (name, secret, bearer) in declared.chain(authorization) {
        if headers.contains_key(&name) {
            return Err(Error::InvalidRequest);
        }
        let value = credential_value(secret, bearer)?;
        let wire = std::str::from_utf8(value.as_bytes()).map_err(|_| Error::InvalidRequest)?;
        credentials.record_product_credential(secret);
        credentials.record_wire(&Secret::new(wire));
        headers.insert(name, value);
    }
    Ok((headers, credentials))
}
