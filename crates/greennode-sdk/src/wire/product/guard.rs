use super::{
    evidence::Evidence,
    operation::unsafe_guard_response,
    response::{BodyMatcher, Clean},
};
use crate::wire::strict_json::Shape;
use crate::{Error, ErrorClass};
use bytes::Bytes;
use http::StatusCode;

use super::operation::{Operation, Output, Reflection, execute_with_evidence};
use super::{options::Options, redirect, sender};

pub(crate) struct GuardOperation(Operation);

impl GuardOperation {
    pub(crate) fn new(mut operation: Operation) -> Self {
        assert!(matches!(operation.class, redirect::Classification::Read));
        operation.reflection = Reflection::Guard;
        Self(operation)
    }

    pub(super) fn applies(operation: &Operation, evidence: &Evidence) -> bool {
        matches!(operation.class, redirect::Classification::Read)
            && (operation.reflection == Reflection::Guard || evidence.guards_reads())
    }

    #[allow(clippy::result_large_err, clippy::too_many_arguments)]
    pub(crate) async fn execute<T: Output>(
        &self,
        runtime: sender::Runtime<'_>,
        base: &crate::endpoint::Base,
        options: &Options<'_>,
        path: &[&str],
        query: &[(&str, &str)],
        source: &dyn sender::BearerSource,
        evidence: &mut Evidence,
    ) -> Result<(StatusCode, Guarded<T>), crate::Failure<T>> {
        let (status, value) = execute_with_evidence(
            runtime,
            base,
            self,
            options,
            path,
            query,
            source,
            bytes::Bytes::new(),
            evidence,
        )
        .await?;
        let value = Guarded::latest(value, evidence)
            .map_err(|error| crate::Failure::new(*error, None, crate::WriteDispatch::NotAWrite))?;
        Ok((status, value))
    }
}

impl std::ops::Deref for GuardOperation {
    type Target = Operation;

    fn deref(&self) -> &Operation {
        &self.0
    }
}

#[derive(Clone, Copy)]
pub(super) struct DecisionPrice {
    quoted: f64,
    cap: f64,
}

impl DecisionPrice {
    fn reflects(self, matcher: &BodyMatcher<'_>) -> bool {
        let reflects = |text: &str| matcher.reflects(text.as_bytes(), false);
        [self.quoted, self.cap].into_iter().any(|value| {
            crate::output_number::any_spelling(value, true, reflects)
                || reflects(&format!("{value:.0}"))
                || reflects(&crate::output_number::fixed_two(value))
        }) || reflects(&format!("{:.0} {:.0}", self.quoted, self.cap))
    }
}

#[derive(Clone)]
pub(super) struct Proof {
    id: &'static str,
    body: Bytes,
    not_found_stream: bool,
    request: Option<super::absence::CleanRequest>,
    policy: ReceiptPolicy,
    price: Option<DecisionPrice>,
    operands: Option<std::sync::Arc<super::output::Operands>>,
}

impl Proof {
    pub(super) fn new(id: &'static str, body: &[u8], clean: Clean) -> Self {
        Self {
            id,
            body: Bytes::from_owner(zeroize::Zeroizing::new(body.to_vec())),
            not_found_stream: clean.not_found_stream,
            request: None,
            policy: ReceiptPolicy::Guard,
            price: None,
            operands: None,
        }
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.body.len()
            + self
                .operands
                .as_deref()
                .map_or(0, super::output::Operands::bytes)
    }

    pub(super) fn belongs_to(&self, id: &str) -> bool {
        self.id == id && self.operands.is_none()
    }

    pub(super) fn keep_operands(&mut self, operands: super::output::Operands) {
        self.operands = Some(std::sync::Arc::new(operands));
    }

    pub(super) fn attach_price(&mut self, price: DecisionPrice) -> bool {
        if self.price.is_some() {
            return false;
        }
        self.price = Some(price);
        true
    }

    pub(super) fn check(&self, matcher: &BodyMatcher<'_>) -> Option<Box<Error>> {
        let screen = if self.not_found_stream {
            Evidence::screen_not_found_with(matcher, &self.body)
        } else if self.body.is_empty() {
            super::response::Screen::Clean
        } else {
            Evidence::screen_peer_with(matcher, &self.body, true)
        };
        (!screen.is_clean()
            || self
                .request
                .as_ref()
                .is_some_and(|request| !request.is_direct())
            || self.price.is_some_and(|price| price.reflects(matcher))
            || self
                .operands
                .as_deref()
                .is_some_and(|fields| !fields.screen(matcher).is_clean()))
        .then(|| {
            Box::new(
                Error::new(ErrorClass::InvalidInput).with_cause(unsafe_guard_response(self.id)),
            )
        })
    }
}

pub(super) fn check_proofs(
    proofs: &[Proof],
    credentials: &super::evidence::SentCredentials,
) -> Option<Box<Error>> {
    let transport = credentials.transport_only();
    let guard = credentials.guard_credentials();
    let mut remaining = 192 << 20;
    proofs.iter().find_map(|proof| {
        let credentials = match proof.policy {
            ReceiptPolicy::All => credentials,
            ReceiptPolicy::Guard => &guard,
            ReceiptPolicy::Transport | ReceiptPolicy::PrivateIssuance(_) => &transport,
        };
        let matcher = BodyMatcher::with_work(credentials, remaining);
        let error = proof.check(&matcher);
        remaining = matcher.remaining();
        error
    })
}

pub(crate) struct Decision<'a>(&'a Evidence);

impl<'a> Decision<'a> {
    pub(crate) fn evidence(self) -> &'a Evidence {
        self.0
    }
}

pub(crate) fn decide<T>(
    proofs: &Evidence,
    evaluate: impl FnOnce(Decision<'_>) -> T,
) -> Result<T, Box<Error>> {
    proofs.recheck_read_proofs()?;
    Ok(evaluate(Decision(proofs)))
}

#[derive(Clone)]
pub(crate) struct Guarded<T> {
    value: T,
    proofs: Vec<Proof>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum ReceiptPolicy {
    #[default]
    All,
    Transport,
    Guard,
    PrivateIssuance(&'static str),
}

pub(crate) struct CleanReceipt<'a> {
    body: &'a [u8],
    clean: Clean,
    request: super::absence::CleanRequest,
    policy: ReceiptPolicy,
}

impl<'a> CleanReceipt<'a> {
    pub(super) fn new(
        body: &'a [u8],
        clean: Clean,
        request: super::absence::CleanRequest,
        policy: ReceiptPolicy,
    ) -> Self {
        Self {
            body,
            clean,
            request,
            policy,
        }
    }

    pub(crate) fn body(&self) -> &[u8] {
        self.body
    }

    pub(super) fn into_proof(self) -> Proof {
        let mut proof = Proof::new(self.request.operation(), self.body, self.clean);
        proof.request = Some(self.request);
        proof.policy = self.policy;
        proof
    }

    pub(crate) fn guard<T>(self, value: T) -> Guarded<T> {
        Guarded {
            value,
            proofs: vec![self.into_proof()],
        }
    }
}

#[derive(Clone)]
pub(crate) struct AcknowledgedId {
    id: String,
    proof: Proof,
    revoked: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

crate::error::debug_record!(AcknowledgedId unit);

impl CleanReceipt<'_> {
    pub(crate) fn bind_id(self, id: String) -> AcknowledgedId {
        AcknowledgedId::new(id, self.into_proof())
    }
}

impl AcknowledgedId {
    fn new(id: String, proof: Proof) -> Self {
        Self {
            id,
            proof,
            revoked: std::sync::Arc::default(),
        }
    }

    pub(crate) fn recheck<'a>(&'a self, evidence: &Evidence) -> Result<&'a str, Box<Error>> {
        match self.refusal(evidence.credentials()) {
            Some(error) => Err(error),
            None => Ok(&self.id),
        }
    }

    pub(super) fn refusal(
        &self,
        credentials: &super::evidence::SentCredentials,
    ) -> Option<Box<Error>> {
        use std::sync::atomic::Ordering;
        if self.revoked.load(Ordering::Relaxed) {
            return Some(Box::new(unsafe_guard_response(self.proof.id)));
        }
        let refusal = check_proofs(std::slice::from_ref(&self.proof), credentials).or_else(|| {
            credentials
                .id_reflects(&self.id)
                .then(|| Box::new(unsafe_guard_response(self.proof.id)))
        });
        if refusal.is_some() {
            self.revoked.store(true, Ordering::Relaxed);
        }
        refusal
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ConfirmationTarget<'a> {
    Explicit(&'a str),
    Acknowledged(&'a AcknowledgedId),
}

impl<'a> ConfirmationTarget<'a> {
    pub(crate) fn checked(self, evidence: &Evidence) -> Result<&'a str, Box<Error>> {
        match self {
            Self::Explicit(id) => Ok(id),
            Self::Acknowledged(id) => id.recheck(evidence),
        }
    }
}

#[derive(Default, serde::Deserialize)]
pub(crate) struct Acknowledgement<T> {
    #[serde(skip)]
    received: Option<Guarded<T>>,
}

impl<T: Output> Output for Acknowledgement<T> {
    const SCHEMA: Shape<'static> = T::SCHEMA;
    const RESPONSE_GUARD: Option<super::sender::ResponseGuard> = T::RESPONSE_GUARD;

    fn decode_response(
        operation: &Operation,
        response: &super::sender::Response,
    ) -> Result<Self, Box<Error>> {
        let value = T::decode_response(operation, response)?;
        let received = response
            .clean_receipt()
            .ok_or_else(|| {
                Box::new(
                    Error::new(ErrorClass::NotSettled)
                        .with_cause(unsafe_guard_response(operation.id)),
                )
            })?
            .guard(value);
        Ok(Self {
            received: Some(received),
        })
    }
}

impl<T> Acknowledgement<T> {
    pub(crate) fn into_id(
        self,
        select: impl FnOnce(&T) -> Option<&str>,
    ) -> Option<(T, AcknowledgedId)> {
        let Guarded { value, mut proofs } = self.received?;
        let id = select(&value)?.to_owned();
        Some((value, AcknowledgedId::new(id, proofs.pop()?)))
    }
}

impl<T> Guarded<T> {
    pub(crate) fn acknowledged_id(
        &self,
        select: impl FnOnce(&T) -> Option<&str>,
    ) -> Option<AcknowledgedId> {
        let [proof] = self.proofs.as_slice() else {
            return None;
        };
        proof.request.as_ref()?;
        Some(AcknowledgedId::new(
            select(&self.value)?.to_owned(),
            proof.clone(),
        ))
    }

    pub(crate) fn register_acknowledgement(
        &self,
        evidence: &mut Evidence,
        select: impl FnOnce(&T) -> Option<&str>,
    ) -> Result<(), Box<Error>> {
        let id = self
            .acknowledged_id(select)
            .ok_or_else(|| Box::new(unsafe_guard_response("")))?;
        evidence.keep_acknowledgement(&id)
    }

    pub(crate) async fn read_preflight<E>(
        evidence: &mut Evidence,
        read: impl AsyncFnOnce(&mut Evidence) -> Result<T, E>,
        invalid: impl FnOnce(Box<Error>) -> E,
    ) -> Result<Self, E> {
        let checkpoint = evidence.guard_checkpoint();
        let scope = evidence.guard_reads();
        let result = read(evidence).await;
        drop(scope);
        if result.is_err()
            && let Some(error) = evidence.recheck_guards(evidence.credentials())
        {
            return Err(invalid(error));
        }
        Self::from_checkpoint(result?, evidence, checkpoint).map_err(invalid)
    }

    pub(crate) fn retain(&self, evidence: &mut Evidence) -> Result<(), Box<Error>> {
        self.recheck(evidence)?;
        for proof in &self.proofs {
            if !evidence.keep_proof(proof.clone()) {
                return Err(Box::new(unsafe_guard_response(proof.id)));
            }
        }
        Ok(())
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

    pub(crate) fn latest_direct(
        value: T,
        evidence: &Evidence,
        id: &'static str,
    ) -> Result<Self, Box<Error>> {
        let proof = evidence
            .last_guard()
            .filter(|proof| proof.id == id && proof.request.is_some())
            .ok_or_else(|| Box::new(unsafe_guard_response(id)))?;
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
        match check_proofs(&self.proofs, evidence.credentials()) {
            Some(error) => Err(error),
            None => Ok(&self.value),
        }
    }
}

impl Guarded<(f64, f64)> {
    pub(crate) fn price(
        evidence: &mut Evidence,
        quoted: f64,
        cap: f64,
    ) -> Result<Self, Box<Error>> {
        let proof = evidence
            .price_proof(DecisionPrice { quoted, cap })
            .ok_or_else(|| Box::new(unsafe_guard_response("pricing.GetQuote")))?;
        Ok(Self {
            value: (quoted, cap),
            proofs: vec![proof],
        })
    }
}

macro_rules! guarded_poll {
    ($evidence:expr, $body:block) => {{
        let _guard_reads = $evidence.guard_reads();
        let checkpoint = $evidence.guard_checkpoint();
        loop {
            $evidence.discard_guards_after(checkpoint);
            $body
        }
    }};
    ($evidence:expr, for $index:ident in $steps:expr, $body:block) => {{
        let _guard_reads = $evidence.guard_reads();
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

    fn production_source(source: &str) -> String {
        use regex_lite::Regex;
        if !source.contains("#[cfg(test)]") {
            return source.to_owned();
        }
        let declaration = Regex::new(
            r"\A#\[cfg\(test\)\]\s*(?:#\[[^\n]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*[;{]",
        )
        .unwrap();
        let literal = Regex::new(r#"(?s)\A(?:"(?:\\.|[^"\\])*"|'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^'\\\r\n])')"#).unwrap();
        let mut output = String::with_capacity(source.len());
        let mut cursor = 0;
        while cursor < source.len() {
            let remaining = &source[cursor..];
            if remaining.starts_with("#[cfg(test)]")
                && let Some(module) = declaration.find(remaining)
            {
                let mut end = cursor + module.end();
                if module.as_str().ends_with('{') {
                    let mut depth = 1;
                    while depth != 0 {
                        let token = source_token(&source[end..], &literal);
                        match token {
                            "{" => depth += 1,
                            "}" => depth -= 1,
                            _ => {}
                        }
                        end += token.len();
                    }
                }
                output.extend(
                    source[cursor..end]
                        .chars()
                        .map(|character| if character == '\n' { '\n' } else { ' ' }),
                );
                cursor = end;
                continue;
            }
            let token = source_token(remaining, &literal);
            output.push_str(token);
            cursor += token.len();
        }
        output
    }

    fn source_token<'a>(source: &'a str, literal: &regex_lite::Regex) -> &'a str {
        if source.starts_with("//") {
            return source
                .split_once('\n')
                .map_or(source, |(comment, _)| comment);
        }
        if source.starts_with("/*") {
            let mut depth = 1;
            let mut end = 2;
            while depth != 0 {
                match source.as_bytes().get(end..end + 2) {
                    Some(b"/*") => {
                        depth += 1;
                        end += 2;
                    }
                    Some(b"*/") => {
                        depth -= 1;
                        end += 2;
                    }
                    Some(_) => end += 1,
                    None => panic!("unterminated source comment"),
                }
            }
            return &source[..end];
        }
        if let Some(raw) = source.strip_prefix('r') {
            let hashes = raw.bytes().take_while(|byte| *byte == b'#').count();
            if let Some(contents) = raw[hashes..].strip_prefix('"') {
                let closing = format!("\"{}", &raw[..hashes]);
                let end = contents
                    .find(&closing)
                    .expect("terminated raw source literal");
                return &source[..2 + hashes + end + closing.len()];
            }
        }
        if source.starts_with(['"', '\''])
            && let Some(value) = literal.find(source)
        {
            return value.as_str();
        }
        &source[..source
            .chars()
            .next()
            .expect("terminated source module")
            .len_utf8()]
    }

    #[test]
    fn scan_resumes_after_external_and_inline_test_modules() {
        let source = r###"
fn first() {}
#[cfg(test)]
mod external;
fn second() {}
#[cfg(test)]
#[allow(dead_code)]
mod inline {
    fn hidden() {
        let raw = r#"
}
"#;
        let quoted = "escaped \"} strike";
        let closing = '}';
        /* outer /* } */ } */
    }
}
fn third() {}
"###;
        let visible = production_source(source);
        for name in ["first", "second", "third"] {
            assert!(visible.contains(&format!("fn {name}()")));
        }
        for hidden in ["external", "inline", "hidden", "strike"] {
            assert!(!visible.contains(hidden));
        }
    }

    fn production_files(root: &Path, mut inspect: impl FnMut(&Path, &str)) {
        let mut files = vec![root.to_owned()];
        while let Some(file) = files.pop() {
            if file.is_dir() {
                files.extend(file.read_dir().unwrap().map(|entry| entry.unwrap().path()));
                continue;
            }
            let path = file.strip_prefix(root).unwrap();
            if file.extension().is_none_or(|ext| ext != "rs")
                || path.iter().any(|part| part == "tests")
                || path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with("tests.rs"))
            {
                continue;
            }
            let source = std::fs::read_to_string(&file).unwrap();
            inspect(path, &production_source(&source));
        }
    }

    #[test]
    fn project_discovery_paths_use_the_same_pre_retry_policy() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for (path, function, policy) in [
            (
                "discovery/mod.rs",
                "read_projects_with_evidence",
                "project_discovery: true",
            ),
            (
                "network/wire.rs",
                "exchange",
                "project_discovery: read.style == Style::BandwidthProject",
            ),
        ] {
            let source = production_source(&std::fs::read_to_string(root.join(path)).unwrap());
            assert!(
                definition(&source, function).is_some_and(|body| body.contains(policy)),
                "{path}"
            );
        }
    }

    #[test]
    fn screen_decisions_do_not_reject_only_reflected_responses() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let reflected_only = regex_lite::Regex::new(concat!(
            r"(?s)(?:==|!=)\s*(?:[\w:]+::)?Screen::Reflected\b",
            r"|Screen::Reflected\s*(?:==|!=|=>)",
            r"|matches!\s*\([^;]{0,160},\s*(?:[\w:]+::)?Screen::Reflected\s*\)",
        ))
        .unwrap();
        production_files(&root, |path, source| {
            if source.contains("Screen::Reflected") {
                assert!(!reflected_only.is_match(source), "{}", path.display());
            }
        });
    }

    #[test]
    fn typed_read_outcomes_require_a_current_decision_witness() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        production_files(&root, |path, source| {
            let path_name = path.to_str().unwrap();
            let factory = match path_name {
                "iam/writes.rs" => Some("decided_refusal"),
                "loadbalancer/deletes.rs" => Some("resource_in_use"),
                "compute/server_writes.rs" => Some("delete_output"),
                _ => None,
            };
            let mut rest = source.to_owned();
            if let Some(factory) = factory {
                let body = definition(source, factory).unwrap();
                assert!(body.contains("Decision<'_>"), "{path_name} {factory}");
                rest = rest.replacen(body, "", 1);
            }
            for marker in ["WriteError::ResourceInUse", "WriteError::PrivilegedChange"] {
                assert!(
                    !rest
                        .replace(&format!("Some(&{marker})"), "")
                        .contains(marker),
                    "{path_name} {marker}"
                );
            }
            assert!(
                !rest
                    .replace("pub struct DeleteServerOutput {", "")
                    .replace("public_fields!(DeleteServerOutput {", "")
                    .contains("DeleteServerOutput {"),
                "{path_name}"
            );
        });
    }

    fn definition<'a>(text: &'a str, name: &str) -> Option<&'a str> {
        let declaration = regex_lite::Regex::new(&format!(
            r"(?m)^([ \t]*)(?:pub(?:\([^)]*\))? )?(?:async )?fn {name}\b",
        ))
        .unwrap();
        let found = declaration.captures(text)?;
        let end = format!("\n{}}}", &found[1]);
        text[found.get(0)?.end()..]
            .split_once(&end)
            .map(|(body, _)| body)
    }

    fn uses_write_evidence(
        root: &Path,
        links: &[(&str, &str, &str)],
        file: &Path,
        body: &str,
        depth: usize,
    ) -> bool {
        if body.contains("Evidence::new()") {
            return false;
        }
        if body.contains("Evidence::for_write()") {
            return true;
        }
        depth > 0
            && links.iter().any(|(caller, call, callee)| {
                if file != Path::new(caller) || !body.contains(call) {
                    return false;
                }
                let name = call
                    .trim_end_matches('(')
                    .rsplit(['.', ':'])
                    .next()
                    .unwrap();
                let source =
                    production_source(&std::fs::read_to_string(root.join(callee)).unwrap());
                let body = definition(&source, name).unwrap_or_else(|| panic!("{callee} {name}"));
                uses_write_evidence(root, links, Path::new(callee), body, depth - 1)
            })
    }

    #[test]
    fn paid_price_decisions_keep_typed_proofs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for (path, helper) in [
            ("compute/server_writes.rs", "check_price"),
            ("loadbalancer/paid.rs", "guard_price"),
            ("volume/paid.rs", "check_price"),
        ] {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            assert!(
                definition(&source, helper).is_some_and(|body| body.contains("Guarded::price(")),
                "{path} {helper}"
            );
        }
    }

    #[test]
    fn guard_decoders_use_canonical_field_names() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for (path, decoder, policy) in [
            (
                "wire/product/operation.rs",
                "execute_with_evidence",
                "has_case_variant",
            ),
            ("volume/mod.rs", "read_value", "decode(&document, names,"),
            ("volume/wire.rs", "entry", ".check(name, key)"),
            ("volume/wire.rs", "optional", "T::from_wire(raw, self.1)"),
            (
                "cdn/wire.rs",
                "call_with_policy",
                ".validate(text.as_bytes(), &ENVELOPE",
            ),
            ("cdn/reads.rs", "read_guard_detail", "FieldNames::Canonical"),
            (
                "cdn/reads.rs",
                "decode_web_detail",
                ".validate(text.as_bytes(), &STATE",
            ),
            (
                "globalloadbalancer/child/mod.rs",
                "decode_canonical",
                "validate_canonical_shape",
            ),
            (
                "globalloadbalancer/load_balancer_writes.rs",
                "load_balancer_detail",
                "validate_canonical_shape",
            ),
            (
                "loadbalancer/guard.rs",
                "canonical_json",
                "validate_canonical_shape",
            ),
            (
                "loadbalancer/wire.rs",
                "canonical_raw",
                "FieldNames::Canonical",
            ),
            ("loadbalancer/wire.rs", "field_raw", ".check(name, key)"),
            ("compute/wire.rs", "field", "has_case_variant"),
            (
                "quote/request/decode.rs",
                "quote_object",
                "exact(op, obj, AMOUNTS)",
            ),
        ] {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            assert!(
                definition(&source, decoder).is_some_and(|body| body.contains(policy)),
                "{path} {decoder}"
            );
        }
    }

    #[test]
    fn typed_role_survives_a_wire_name_without_identifier_metadata() {
        use super::Evidence;
        use crate::Secret;
        use crate::wire::product::response::Screen;

        #[derive(Debug)]
        struct Operands {
            destination_id: String,
        }
        impl super::super::output::PublicOutput for Operands {
            fn public_reflects(
                &self,
                scan: &super::super::output::FieldScan<'_>,
                identifier: bool,
            ) -> bool {
                scan.field(
                    &self.destination_id,
                    identifier,
                    "destination_id",
                    "DestinationID",
                )
            }
        }
        let body = br#"{"opaque":"pre6A6B6Cpost"}"#;
        let value = Operands {
            destination_id: "pre6A6B6Cpost".into(),
        };
        let mut evidence = Evidence::for_write();
        assert!(evidence.keep_guard_for_test("test", body));
        evidence.keep_decoded_operands("test", &value).unwrap();
        let before = super::Guarded::latest((), &evidence).unwrap();
        assert!(before.recheck(&evidence).is_ok());
        evidence.add_bearer(&Secret::new("jkl"));
        assert_eq!(evidence.peer_body_screen(body), Screen::Clean);
        assert!(before.recheck(&evidence).is_err());
        assert!(evidence.recheck_read_proofs().is_err());
    }

    #[test]
    fn depth_and_budget_exhaustion_are_indeterminate() {
        use super::{BodyMatcher, Evidence};
        use crate::Secret;
        use crate::wire::product::{evidence::SentCredentials, response::Screen};
        let credentials = SentCredentials::for_test(vec![Secret::new("toy-guard")]);
        let evidence = Evidence::from_credentials(&credentials);
        let deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
        assert_eq!(
            evidence.peer_body_screen(deep.as_bytes()),
            Screen::Indeterminate
        );
        assert_eq!(
            evidence.peer_body_screen(br#"{"safe":true}"#),
            Screen::Clean
        );
        assert_eq!(
            evidence.peer_body_screen(br#"{"value":"toy-guard"}"#),
            Screen::Reflected
        );
        let matcher = BodyMatcher::with_work(&credentials, 1);
        assert_eq!(matcher.scan(b"public text", false), Screen::Indeterminate);
    }

    #[test]
    fn mutation_confirmation_reads_reuse_guarded_write_evidence() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for (path, writer, reader) in [
            (
                "compute/server_writes.rs",
                "delete_server",
                ".get_volume_with_evidence(ctx, &volume_id, &mut evidence)",
            ),
            (
                "compute/server_groups.rs",
                "update_server_group",
                ".group_detail::<()>(ctx, id, &mut evidence)",
            ),
            (
                "iam/groups.rs",
                "create_group",
                ".fetch_group(ctx, id, false, &mut evidence)",
            ),
            (
                "iam/groups.rs",
                "update_group",
                ".fetch_group(ctx, id, false, &mut evidence)",
            ),
        ] {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            let body = definition(&source, writer).unwrap();
            assert!(
                body.contains(reader) && !body.contains("Evidence::new()"),
                "{path} {writer}"
            );
        }
    }

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
            ("cdn/writes.rs", ".toggle(", "cdn/writes/settle.rs"),
            ("cdn/writes/settle.rs", ".write_evidence(", "cdn/writes.rs"),
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
                "globalloadbalancer/child/mod.rs",
            ),
            (
                "globalloadbalancer/child/delete.rs",
                ".start_child(",
                "globalloadbalancer/child/mod.rs",
            ),
            (
                "globalloadbalancer/child/mod.rs",
                ".start_write(",
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
            ("iam/user_mfa.rs", ".change_user_mfa(", "iam/user_mfa.rs"),
            ("iam/user_mfa.rs", ".write_evidence(", "iam/writes.rs"),
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
                "loadbalancer/member_writes.rs",
            ),
            (
                "loadbalancer/member_writes.rs",
                "ChildWriter::new(",
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
        let mut checked = 0;
        production_files(&root, |relative, text| {
            if !text.contains("pub async fn ") {
                return;
            }
            for entry in writes.captures_iter(text) {
                let from = entry.get(0).unwrap().end();
                let end = format!("\n{}}}", &entry[1]);
                let body = text[from..].split_once(&end).unwrap().0;
                if &entry[2] == "$method" && relative == Path::new("loadbalancer/mod.rs") {
                    assert!(body.contains("let read = Read") && body.contains("self.read("));
                    continue;
                }
                let preflight = uses_write_evidence(&root, &owners, relative, body, 8);
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
        });
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
            ("loadbalancer/paid.rs", 1),
            ("loadbalancer/paid/settlement.rs", 1),
            ("vks/write.rs", 1),
            ("volume/write.rs", 1),
        ] {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            let source = production_source(&source);
            assert_eq!(source.matches("guarded_poll!(").count(), count, "{path}");
            assert!(!ordinary_loop.is_match(&source), "{path}");
        }
        for path in ["listener_writes.rs", "pool_writes.rs", "policy_writes.rs"] {
            let source = std::fs::read_to_string(root.join("loadbalancer").join(path)).unwrap();
            assert_eq!(source.matches(".settle_child(").count(), 1, "{path}");
            assert!(!ordinary_loop.is_match(&source), "{path}");
        }
    }
}
