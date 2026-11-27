use super::{evidence::Evidence, operation::unsafe_guard_response, response::BodyMatcher};
use crate::{Error, ErrorClass};
use bytes::Bytes;

#[derive(Clone)]
pub(super) struct Proof {
    pub(super) id: &'static str,
    pub(super) body: Bytes,
}

impl Proof {
    pub(super) fn check(&self, matcher: &BodyMatcher<'_>) -> Option<Box<Error>> {
        (!self.body.is_empty()
            && Evidence::screen_peer_with(matcher, &self.body, true) != Some(false))
        .then(|| {
            Box::new(
                Error::new(ErrorClass::InvalidInput).with_cause(unsafe_guard_response(self.id)),
            )
        })
    }
}

pub(crate) struct Guarded<T> {
    value: T,
    proof: Proof,
}

impl<T> Guarded<T> {
    pub(crate) fn latest(value: T, evidence: &Evidence) -> Result<Self, Box<Error>> {
        let proof = evidence
            .last_guard()
            .ok_or_else(|| Box::new(unsafe_guard_response("")))?;
        Ok(Self { value, proof })
    }

    pub(crate) fn into_checked(self, evidence: &Evidence) -> Result<T, Box<Error>> {
        self.recheck(evidence)?;
        Ok(self.value)
    }

    pub(crate) fn recheck(&self, evidence: &Evidence) -> Result<&T, Box<Error>> {
        let transport = evidence.credentials().transport_only();
        let matcher = BodyMatcher::new(&transport, self.proof.body.len());
        match self.proof.check(&matcher) {
            Some(error) => Err(error),
            None => Ok(&self.value),
        }
    }
}
