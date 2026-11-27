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
    pub(crate) async fn read_preflight<E>(
        evidence: &mut Evidence,
        read: impl AsyncFnOnce(&mut Evidence) -> Result<T, E>,
        invalid: impl FnOnce(Box<Error>) -> E,
    ) -> Result<Self, E> {
        let checkpoint = evidence.guard_checkpoint();
        let value = {
            let mut scope = evidence.preflight_reads();
            read(scope.evidence()).await?
        };
        Self::from_checkpoint(value, evidence, checkpoint).map_err(invalid)
    }

    pub(crate) fn acknowledgement(
        value: T,
        evidence: &mut Evidence,
        id: &'static str,
        body: &[u8],
    ) -> Result<Self, Box<Error>> {
        if !evidence.keep_guard(id, body) {
            return Err(Box::new(unsafe_guard_response(id)));
        }
        Self::latest(value, evidence)
    }

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
    fn sdk_write_entry_points_use_guarded_preflight_evidence() {
        use regex_lite::Regex;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let writes = Regex::new(concat!(
            r"(?m)^([ \t]+)pub async fn (\$method|discard_after_delivery_failure|(?:",
            "attach|create|delete|detach|disable|enable|import|purge|reboot|rename|reset|resize|start|stop|tag|untag|update",
            r")_\w+)\(",
        )).unwrap();
        let owners = [
            ("cdn/writes.rs", ".write_evidence(", "cdn/writes.rs"),
            ("cdn/writes.rs", ".toggle(", "cdn/writes.rs"),
            ("cdn/writes/update.rs", ".write_evidence(", "cdn/writes.rs"),
            (
                "compute/server_actions.rs",
                ".toggle(",
                "compute/server_actions.rs",
            ),
            (
                "dns/writes/mod.rs",
                "Workflow::start(",
                "dns/writes/workflow.rs",
            ),
            (
                "globalloadbalancer/child/create.rs",
                ".start_child(",
                "globalloadbalancer/write.rs",
            ),
            (
                "globalloadbalancer/child/delete.rs",
                ".start_child(",
                "globalloadbalancer/write.rs",
            ),
            (
                "globalloadbalancer/load_balancer_writes.rs",
                ".start_write(",
                "globalloadbalancer/write.rs",
            ),
            ("iam/attachments.rs", ".write_evidence(", "iam/writes.rs"),
            ("iam/groups.rs", ".write_evidence(", "iam/writes.rs"),
            ("iam/policy_writes.rs", ".write_evidence(", "iam/writes.rs"),
            (
                "iam/service_account_writes.rs",
                ".write_evidence(",
                "iam/writes.rs",
            ),
            ("iam/user_mfa.rs", ".change_user_mfa(", "iam/writes.rs"),
            ("iam/user_writes.rs", ".write_evidence(", "iam/writes.rs"),
            (
                "loadbalancer/certificate_import.rs",
                ".order_evidence(",
                "loadbalancer/quotes.rs",
            ),
            (
                "loadbalancer/child_deletes.rs",
                ".delete_child(",
                "loadbalancer/child_deletes.rs",
            ),
            (
                "loadbalancer/listener_writes.rs",
                "ChildWriter::new(",
                "loadbalancer/child_writes.rs",
            ),
            (
                "loadbalancer/member_writes.rs",
                ".change_members(",
                "loadbalancer/child_writes.rs",
            ),
            (
                "loadbalancer/paid.rs",
                ".order_evidence(",
                "loadbalancer/quotes.rs",
            ),
            (
                "loadbalancer/policy_writes.rs",
                "ChildWriter::new(",
                "loadbalancer/child_writes.rs",
            ),
            (
                "loadbalancer/pool_writes.rs",
                "ChildWriter::new(",
                "loadbalancer/child_writes.rs",
            ),
            ("tagging/writes.rs", ".replace_tag(", "tagging/writes.rs"),
            ("vks/create.rs", ".create_cluster_inner(", "vks/create.rs"),
            ("vks/delete.rs", ".delete_cluster_inner(", "vks/delete.rs"),
            (
                "volume/attachment.rs",
                ".change_attachment(",
                "volume/attachment.rs",
            ),
        ];
        let mut files = vec![root.clone()];
        let mut checked = 0;
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(path.read_dir().unwrap().map(|entry| entry.unwrap().path()));
                continue;
            }
            let relative = path.strip_prefix(&root).unwrap();
            if path.extension().is_none_or(|ext| ext != "rs")
                || relative.iter().any(|part| part == "tests")
                || path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with("tests.rs"))
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for entry in writes.captures_iter(&text) {
                let from = entry.get(0).unwrap().end();
                let end = format!("\n{}}}", &entry[1]);
                let body = text[from..].split_once(&end).unwrap().0;
                if &entry[2] == "$method" && relative == Path::new("loadbalancer/mod.rs") {
                    assert!(body.contains("let read = Read") && body.contains("self.read("));
                    continue;
                }
                let preflight = body.contains("Evidence::for_write()")
                    || owners.iter().any(|(entry, call, owner)| {
                        relative == Path::new(entry)
                            && body.contains(call)
                            && std::fs::read_to_string(root.join(owner))
                                .unwrap()
                                .contains("Evidence::for_write()")
                    });
                let billing_without_reads = matches!(
                    relative.to_str(),
                    Some("billing/budget_writes.rs" | "billing/threshold_writes.rs")
                ) && (body.contains(".write(")
                    || body.contains(".write::<"))
                    && ![".read(", ".get_", ".list_", "Evidence::new()"]
                        .iter()
                        .any(|call| body.contains(call));
                let cleanup_without_reads = relative == Path::new("compute/ssh_keys.rs")
                    && &entry[2] == "discard_after_delivery_failure"
                    && body.contains(".delete_key(")
                    && body.matches(".await").count() == 1;
                assert!(
                    preflight || billing_without_reads || cleanup_without_reads,
                    "{} {}",
                    relative.display(),
                    &entry[2]
                );
                checked += 1;
            }
        }
        assert!(checked >= 75);
    }

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
