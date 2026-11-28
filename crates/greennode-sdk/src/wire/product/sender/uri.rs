use super::SentCredentials;
use crate::wire::product::response::BodyMatcher;
use url::Url;

pub(super) fn uri_contains_credential(url: &Url, credentials: &SentCredentials) -> bool {
    let path = url.path();
    let query = url.query();
    let length = path.len().saturating_add(query.map_or(0, str::len));
    let matcher = BodyMatcher::new(credentials, length);
    matcher.raw_or_percent_decoded(path.as_bytes())
        || query.is_some_and(|part| matcher.raw_or_percent_decoded(part.as_bytes()))
}
pub(in crate::wire::product) fn uri_reflects_credential_forms(
    url: &Url,
    credentials: &SentCredentials,
) -> bool {
    let reflects = |component| {
        crate::wire::product::response::reflects_credential_forms(component, credentials)
    };
    !credentials.values().is_empty() && (reflects(url.path()) || url.query().is_some_and(reflects))
}

pub(super) fn redirect_target(
    current: &Url,
    op: &super::Operation<'_>,
    hops: usize,
    visited: &std::collections::HashSet<Url>,
    location: &http::HeaderValue,
    auth: super::Auth<'_>,
    credentials: &SentCredentials,
) -> Result<Url, super::redirect::Refusal> {
    use super::redirect::{self, Refusal};
    use crate::wire::product::response::{reflects_credential, reflects_identifier};
    let location = location.to_str().map_err(|_| Refusal::Invalid)?;
    let next = redirect::decide(
        current,
        redirect::Operation {
            class: op.class,
            method: &op.method,
            retry: op.retry,
            no_redirect: op.no_redirect,
        },
        hops,
        visited,
        location,
    )
    .map_err(|refusal| {
        if refusal == Refusal::Origin {
            redirect::describe_origin(current, location, credentials)
        } else {
            refusal
        }
    })?;
    let reflected = match auth {
        super::Auth::Bearer(_) => reflects_credential(location, credentials),
        _ => uri_reflects_credential_forms(&next, credentials),
    };
    let reflected_path = next
        .path_segments()
        .into_iter()
        .flatten()
        .any(|segment| reflects_identifier(segment, credentials));
    let reflected_query = next.query().is_some_and(|query| {
        query.split('&').any(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            [key, value]
                .into_iter()
                .any(|part| reflects_identifier(part, credentials))
        })
    });
    if reflected || reflected_path || reflected_query || uri_contains_credential(&next, credentials)
    {
        return Err(Refusal::Invalid);
    }
    Ok(next)
}
