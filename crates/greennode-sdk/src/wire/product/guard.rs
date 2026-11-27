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

macro_rules! guarded_poll {
    ($evidence:expr, $body:block) => {{
        let checkpoint = $evidence.guard_checkpoint();
        loop {
            $evidence.discard_guards_after(checkpoint);
            $body
        }
    }};
    ($evidence:expr, for $index:ident in $steps:expr, $body:block) => {{
        let checkpoint = $evidence.guard_checkpoint();
        for $index in $steps {
            $evidence.discard_guards_after(checkpoint);
            $body
        }
    }};
}

pub(crate) use guarded_poll;

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn settlement_loops_use_shared_guarded_polling() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let ordinary_loop =
            regex_lite::Regex::new(r"(?m)^\s*(?:let [^=\n]+ = )?(?:loop|while)\b.*\{").unwrap();
        for (path, count) in [
            ("cdn/writes/settle.rs", 2),
            ("compute/mutation.rs", 1),
            ("containerregistry/repository_writes.rs", 1),
            ("database/write.rs", 1),
            ("dns/writes/workflow.rs", 1),
            ("globalloadbalancer/child/create.rs", 2),
            ("globalloadbalancer/child/delete.rs", 1),
            ("globalloadbalancer/load_balancer_writes.rs", 2),
            ("loadbalancer/child_deletes.rs", 2),
            ("loadbalancer/child_writes.rs", 2),
            ("loadbalancer/deletes.rs", 1),
            ("loadbalancer/member_writes.rs", 1),
            ("loadbalancer/paid.rs", 2),
            ("vks/write.rs", 1),
            ("volume/write.rs", 1),
        ] {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            let source = source.split("#[cfg(test)]\nmod ").next().unwrap();
            assert_eq!(source.matches("guarded_poll!(").count(), count, "{path}");
            assert!(!ordinary_loop.is_match(source), "{path}");
        }
        for path in ["listener_writes.rs", "pool_writes.rs", "policy_writes.rs"] {
            let source = std::fs::read_to_string(root.join("loadbalancer").join(path)).unwrap();
            assert_eq!(source.matches(".settle_child(").count(), 1, "{path}");
            assert!(!ordinary_loop.is_match(&source), "{path}");
        }
    }
}
