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
    proofs: Vec<Proof>,
}

impl<T> Guarded<T> {
    pub(crate) fn latest(value: T, evidence: &Evidence) -> Result<Self, Box<Error>> {
        let proof = evidence
            .last_guard()
            .ok_or_else(|| Box::new(unsafe_guard_response("")))?;
        Ok(Self {
            value,
            proofs: vec![proof],
        })
    }

    pub(crate) fn from_checkpoint(
        value: T,
        evidence: &Evidence,
        checkpoint: usize,
    ) -> Result<Self, Box<Error>> {
        let proofs = evidence.guards_after(checkpoint).to_vec();
        if proofs.is_empty() {
            return Err(Box::new(unsafe_guard_response("")));
        }
        Ok(Self { value, proofs })
    }

    pub(crate) fn with_proofs_of<U>(mut self, source: &Guarded<U>) -> Self {
        self.proofs.extend_from_slice(&source.proofs);
        self
    }

    pub(crate) fn into_checked(self, evidence: &Evidence) -> Result<T, Box<Error>> {
        self.recheck(evidence)?;
        Ok(self.value)
    }

    pub(crate) fn try_map<U>(
        self,
        evidence: &Evidence,
        map: impl FnOnce(T) -> Result<U, Box<Error>>,
    ) -> Result<Guarded<U>, Box<Error>> {
        self.recheck(evidence)?;
        let value = map(self.value)?;
        Ok(Guarded {
            value,
            proofs: self.proofs,
        })
    }

    pub(crate) fn recheck(&self, evidence: &Evidence) -> Result<&T, Box<Error>> {
        let transport = evidence.credentials().transport_only();
        let matcher = BodyMatcher::with_work(&transport, 192 << 20);
        match self.proofs.iter().find_map(|proof| proof.check(&matcher)) {
            Some(error) => Err(error),
            None => Ok(&self.value),
        }
    }
}
